use super::{patterns, store_file};
use crate::{
    cli::Options,
    counter,
    languages::Registry,
    model::{Report, Source},
    progress::Progress,
};
use anyhow::Result;
use fancy_regex::Regex;
use rayon::prelude::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};

const BATCH_BYTES: usize = 16 * 1024 * 1024;
type Prepared = (Source, String, String);

/// Retains metadata and hashes; source text lives only in the current bounded batch.
pub(super) struct Pipeline<'a> {
    o: &'a Options,
    registry: &'a Registry,
    progress: &'a Progress,
    report: Report,
    seen: HashMap<[u8; 32], String>,
    include: Vec<String>,
    exclude: Vec<String>,
    include_content: Vec<Regex>,
    exclude_content: Vec<Regex>,
    pool: Option<rayon::ThreadPool>,
    pending: Vec<Prepared>,
    pending_bytes: usize,
    completed: AtomicUsize,
    detail: bool,
    started: bool,
}
impl<'a> Pipeline<'a> {
    pub(super) fn new(
        o: &'a Options,
        registry: &'a Registry,
        progress: &'a Progress,
    ) -> Result<Self> {
        let workers = o.number("processes", 0.0)? as usize;
        Ok(Self {
            o,
            registry,
            progress,
            report: Report::default(),
            seen: HashMap::new(),
            include: o.list("include-lang"),
            exclude: o.list("exclude-lang"),
            include_content: patterns(o, "include-content")?,
            exclude_content: patterns(o, "exclude-content")?,
            pool: if workers > 0 {
                Some(
                    rayon::ThreadPoolBuilder::new()
                        .num_threads(workers)
                        .build()?,
                )
            } else {
                None
            },
            pending: Vec::new(),
            pending_bytes: 0,
            completed: AtomicUsize::new(0),
            detail: o.flag("retain-detail")
                || o.has("strip-comments")
                || o.has("strip-code")
                || o.flag("print-filter-stages"),
            started: false,
        })
    }
    fn ignored(&mut self, name: String, reason: &str) {
        self.report.ignored.push((name, reason.into()));
    }
    pub(super) fn accept(&mut self, mut source: Source) -> Result<()> {
        if !self.started {
            self.progress.begin("Counting", None);
            self.started = true;
        }
        self.report.found.push(source.name.clone());
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
        let Some(text) = text else {
            self.ignored(source.name, "binary file");
            return Ok(());
        };
        let Some(lang) = self.registry.detect(&source.name, &text, self.o) else {
            self.ignored(source.name, "language not recognized");
            return Ok(());
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
            self.ignored(source.name, "language excluded");
            return Ok(());
        }
        for pattern in &self.include_content {
            if !pattern.is_match(&text)? {
                self.ignored(source.name, "content excluded");
                return Ok(());
            }
        }
        for pattern in &self.exclude_content {
            if pattern.is_match(&text)? {
                self.ignored(source.name, "content excluded");
                return Ok(());
            }
        }
        if let Some(hash) = digest {
            if let Some(prior) = self.seen.get(&hash).cloned() {
                if prior <= source.name {
                    self.ignored(source.name, "duplicate file");
                    return Ok(());
                }
                // Preserve alphabetical duplicate selection even when separate
                // arguments or archive members arrive in a different order.
                self.flush()?;
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
            self.seen.insert(hash, source.name.clone());
        }
        if empty {
            self.ignored(source.name, "empty file");
            return Ok(());
        }
        let prepared = (source, text, lang.name.clone());
        if self.pool.is_none() {
            let result = self.count(&prepared);
            self.store(&prepared.0.name, result)?;
        } else {
            if self.pending_bytes + prepared.1.len() > BATCH_BYTES {
                self.flush()?;
            }
            self.pending_bytes += prepared.1.len();
            self.pending.push(prepared);
            if self.pending_bytes >= BATCH_BYTES
                || self.pending.len() >= self.pool.as_ref().unwrap().current_num_threads() * 4
            {
                self.flush()?;
            }
        }
        Ok(())
    }
    fn count(&self, item: &Prepared) -> Result<crate::model::FileCount> {
        let language = self.registry.get(&item.2).unwrap();
        let result = if self.detail {
            counter::count(&item.0, &item.1, language, self.o)
        } else {
            counter::tally(&item.0, &item.1, language, self.o)
        };
        self.progress.update(
            "Counting",
            self.completed.fetch_add(1, Ordering::Relaxed) + 1,
            None,
        );
        result
    }
    fn store(&mut self, name: &str, result: Result<crate::model::FileCount>) -> Result<()> {
        match result {
            Ok(file) => store_file(&mut self.report, file, self.o),
            Err(error) if error.downcast_ref::<counter::TimedOut>().is_some() => {
                self.report.ignored.push((name.into(), error.to_string()));
                if !self.o.flag("quiet") || self.o.flag("show-errors") {
                    eprintln!("{name}: {error}");
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending_bytes = 0;
        let results: Vec<_> = self
            .pool
            .as_ref()
            .unwrap()
            .install(|| pending.par_iter().map(|item| self.count(item)).collect());
        for (item, result) in pending.into_iter().zip(results) {
            self.store(&item.0.name, result)?;
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<Report> {
        self.flush()?;
        self.report.files.sort_by(|a, b| a.name.cmp(&b.name));
        self.report.found.sort();
        self.report.ignored.sort();
        Ok(self.report)
    }
}
