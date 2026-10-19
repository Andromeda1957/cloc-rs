use super::{patterns, store_file};
use crate::{
    cli::Options,
    counter,
    languages::Registry,
    model::{FileCount, Report, Source},
    progress::Progress,
};
use anyhow::Result;
use fancy_regex::Regex;
use std::{
    collections::HashMap,
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Work that depends only on one file: decoding, language selection, content
/// filters, hashing and counting. It runs on worker threads in any order.
pub(super) struct Preparer<'a> {
    o: &'a Options,
    registry: &'a Registry,
    progress: &'a Progress,
    include: Vec<String>,
    exclude: Vec<String>,
    include_content: Vec<Regex>,
    exclude_content: Vec<Regex>,
    completed: AtomicUsize,
    detail: bool,
    started: Once,
}

/// One file's independent result, applied to the report in input order.
pub(super) struct Prepared {
    name: String,
    outcome: Outcome,
}

enum Outcome {
    Ignored(&'static str),
    Failed(anyhow::Error),
    /// Passed every per-file filter; duplicate selection still depends on order.
    Selected {
        digest: Option<[u8; 32]>,
        empty: bool,
        count: Option<Box<Result<FileCount>>>,
    },
}

impl<'a> Preparer<'a> {
    pub(super) fn new(
        o: &'a Options,
        registry: &'a Registry,
        progress: &'a Progress,
    ) -> Result<Self> {
        Ok(Self {
            o,
            registry,
            progress,
            include: o.list("include-lang"),
            exclude: o.list("exclude-lang"),
            include_content: patterns(o, "include-content")?,
            exclude_content: patterns(o, "exclude-content")?,
            completed: AtomicUsize::new(0),
            detail: o.flag("retain-detail")
                || o.has("strip-comments")
                || o.has("strip-code")
                || o.flag("print-filter-stages"),
            started: Once::new(),
        })
    }

    pub(super) fn prepare(&self, mut source: Source) -> Prepared {
        // Concurrent workers wait here, so no count can precede the phase line.
        self.started
            .call_once(|| self.progress.begin("Counting", None));
        let empty = source.bytes.is_empty();
        let digest =
            (!self.o.flag("skip-uniqueness")).then(|| *blake3::hash(&source.bytes).as_bytes());
        let bytes = std::mem::take(&mut source.bytes);
        // Ordinary UTF-8 text takes ownership of the read buffer, with no second full copy.
        let text = if self.o.flag("unicode") {
            counter::decode(&bytes, true).or_else(|| {
                self.o
                    .flag("read-binary-files")
                    .then(|| String::from_utf8_lossy(&bytes).into_owned())
            })
        } else if bytes.contains(&0) && !self.o.flag("read-binary-files") {
            None
        } else {
            Some(match String::from_utf8(bytes) {
                Ok(mut text) => {
                    if text.starts_with('\u{feff}') {
                        text.drain(..3);
                    }
                    text
                }
                Err(error) => {
                    let bytes = error.as_bytes();
                    String::from_utf8_lossy(
                        bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
                    )
                    .into_owned()
                }
            })
        };
        let outcome = match text {
            None => Outcome::Ignored("binary file"),
            Some(text) => self.select(&source, &text, digest, empty),
        };
        Prepared {
            name: source.name,
            outcome,
        }
    }

    fn select(
        &self,
        source: &Source,
        text: &str,
        digest: Option<[u8; 32]>,
        empty: bool,
    ) -> Outcome {
        let Some(lang) = self.registry.detect(&source.name, text, self.o) else {
            return Outcome::Ignored("language not recognized");
        };
        if (!self.include.is_empty()
            && !self
                .include
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&lang.name)))
            || self
                .exclude
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&lang.name))
            || crate::basic::excluded_language(self.o, &lang.name, &self.include)
        {
            return Outcome::Ignored("language excluded");
        }
        for pattern in &self.include_content {
            match pattern.is_match(text) {
                Err(error) => return Outcome::Failed(error.into()),
                Ok(false) => return Outcome::Ignored("content excluded"),
                Ok(true) => (),
            }
        }
        for pattern in &self.exclude_content {
            match pattern.is_match(text) {
                Err(error) => return Outcome::Failed(error.into()),
                Ok(true) => return Outcome::Ignored("content excluded"),
                Ok(false) => (),
            }
        }
        // Counting a file that later proves to be a duplicate only wastes work;
        // its result is discarded in order, exactly as if it was never counted.
        let count = (!empty).then(|| {
            let result = if self.detail {
                counter::count(source, text, lang, self.o)
            } else {
                counter::tally(source, text, lang, self.o)
            };
            self.progress.update(
                "Counting",
                self.completed.fetch_add(1, Ordering::Relaxed) + 1,
                None,
            );
            Box::new(result)
        });
        Outcome::Selected {
            digest,
            empty,
            count,
        }
    }
}

/// Applies prepared files in input order. Retains metadata and hashes, never source text.
pub(super) struct Pipeline<'a> {
    o: &'a Options,
    report: Report,
    seen: HashMap<[u8; 32], String>,
}

impl<'a> Pipeline<'a> {
    pub(super) fn new(o: &'a Options) -> Self {
        Self {
            o,
            report: Report::default(),
            seen: HashMap::new(),
        }
    }

    fn ignored(&mut self, name: String, reason: &str) {
        self.report.ignored.push((name, reason.into()));
    }

    pub(super) fn commit(&mut self, prepared: Prepared) -> Result<()> {
        let Prepared { name, outcome } = prepared;
        self.report.found.push(name.clone());
        let (digest, empty, count) = match outcome {
            Outcome::Ignored(reason) => {
                self.ignored(name, reason);
                return Ok(());
            }
            Outcome::Failed(error) => return Err(error),
            Outcome::Selected {
                digest,
                empty,
                count,
            } => (digest, empty, count),
        };
        if let Some(hash) = digest {
            if let Some(prior) = self.seen.get(&hash).cloned() {
                if prior <= name {
                    self.ignored(name, "duplicate file");
                    return Ok(());
                }
                // Preserve alphabetical duplicate selection even when separate
                // arguments or archive members arrive in a different order.
                if let Some(index) = self.report.files.iter().position(|file| file.name == prior) {
                    let old = self.report.files.remove(index);
                    let language = self.report.languages.get_mut(&old.language).unwrap();
                    for counts in [language, &mut self.report.total] {
                        counts.files -= old.counts.files;
                        counts.blank -= old.counts.blank;
                        counts.comment -= old.counts.comment;
                        counts.code -= old.counts.code;
                    }
                    self.report.languages.retain(|_, counts| counts.files > 0);
                }
                self.report.ignored.retain(|(name, _)| name != &prior);
                self.ignored(prior, "duplicate file");
            }
            self.seen.insert(hash, name.clone());
        }
        if empty {
            self.ignored(name, "empty file");
            return Ok(());
        }
        match *count.expect("every nonempty selected file is counted") {
            Ok(file) => store_file(&mut self.report, file, self.o),
            Err(error) if error.downcast_ref::<counter::TimedOut>().is_some() => {
                if !self.o.flag("quiet") || self.o.flag("show-errors") {
                    eprintln!("{name}: {error}");
                }
                self.report.ignored.push((name, error.to_string()));
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn finish(mut self) -> Report {
        self.report.files.sort_by(|a, b| a.name.cmp(&b.name));
        self.report.found.sort();
        self.report.ignored.sort();
        self.report
    }
}
