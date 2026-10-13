use crate::{
    cli::Options,
    counter,
    languages::Registry,
    model::{Report, Source},
};
use anyhow::{Context, Result, bail};
use fancy_regex::Regex;
use rayon::prelude::*;
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use walkdir::WalkDir;
const VCS_DIRS: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".cvs", ".snapshot"];
pub const GENERATED: &str = "aclocal.m4,autom4te.cache,config.guess,config.sub,configure,depcomp,install-sh,ltmain.sh,missing,Makefile.in";
fn matches(p: &str, text: &str) -> Result<bool> {
    Ok(Regex::new(p)
        .with_context(|| format!("invalid regular expression {p}"))?
        .is_match(text)?)
}
fn list(p: &str) -> Result<Vec<String>> {
    let s = if p == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        fs::read_to_string(p).with_context(|| format!("reading list {p}"))?
    };
    Ok(s.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(str::to_owned)
        .collect())
}
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.into()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}
#[derive(Default)]
struct Selection {
    excluded: Vec<PathBuf>,
}
impl Selection {
    fn new(o: &Options) -> Result<Self> {
        let mut excluded = Vec::new();
        for p in o.all("exclude-list-file") {
            excluded.extend(list(p)?.into_iter().map(|p| absolute(Path::new(&p))))
        }
        Ok(Self { excluded })
    }
    fn excluded(&self, p: &Path) -> bool {
        let a = absolute(p);
        self.excluded.iter().any(|e| a == *e || a.starts_with(e))
    }
    fn dir(&self, p: &Path, o: &Options) -> Result<bool> {
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if VCS_DIRS.contains(&n) || o.list("exclude-dir").iter().any(|d| d == n) || self.excluded(p)
        {
            return Ok(false);
        };
        let s = if o.flag("fullpath") {
            p.to_string_lossy().into_owned()
        } else {
            n.into()
        };
        for r in o.all("not-match-d") {
            if matches(r, &s)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn file(&self, p: &Path, o: &Options) -> Result<bool> {
        if self.excluded(p) {
            return Ok(false);
        }
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        let inc = o.list("include-ext");
        let exc = o.list("exclude-ext");
        if (!inc.is_empty() && !inc.iter().any(|e| e == ext)) || exc.iter().any(|e| e == ext) {
            return Ok(false);
        }
        if o.has("no-autogen") && GENERATED.split(',').any(|s| s == n) {
            return Ok(false);
        }
        if o.flag("skip-win-hidden") && n.starts_with('.') {
            return Ok(false);
        }
        let s = if o.flag("fullpath") {
            p.to_string_lossy().into_owned()
        } else {
            n.into()
        };
        for r in o.all("match-f") {
            if !matches(r, &s)? {
                return Ok(false);
            }
        }
        for r in o.all("not-match-f") {
            if matches(r, &s)? {
                return Ok(false);
            }
        }
        for r in o.all("match-d") {
            if !matches(r, &p.parent().unwrap_or(Path::new("")).to_string_lossy())? {
                return Ok(false);
            }
        }
        // Apply directory exclusions to git and archive member lists too.
        for ancestor in p.ancestors().skip(1) {
            if !self.dir(ancestor, o)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("starting git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )
    };
    Ok(out.stdout)
}
fn archive_kind(n: &str) -> bool {
    [
        ".zip",
        ".jar",
        ".war",
        ".ear",
        ".tar",
        ".tar.gz",
        ".tgz",
        ".tar.bz2",
        ".tbz",
        ".tbz2",
        ".tar.xz",
        ".txz",
        ".tar.zst",
        ".tar.zstd",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}
fn archive(
    name: &str,
    bytes: Vec<u8>,
    o: &Options,
    sel: &Selection,
    sources: &mut Vec<Source>,
    level: usize,
) -> Result<()> {
    if level > 8 {
        bail!("archive nesting exceeds 8 levels: {name}")
    };
    let max = o.number("max-file-size", 100.0)? as u64 * 1024 * 1024;
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    if [".zip", ".jar", ".war", ".ear"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes))?;
        for i in 0..z.len() {
            let mut entry = z.by_index(i)?;
            if entry.is_dir() || entry.size() > max {
                continue;
            };
            let p = entry
                .enclosed_name()
                .context("unsafe archive member name")?;
            let n = p.to_string_lossy().into_owned();
            if !sel.file(&p, o)? {
                continue;
            };
            let mut b = Vec::new();
            entry.by_ref().take(max + 1).read_to_end(&mut b)?;
            if b.len() as u64 <= max {
                members.push((n, b));
            }
        }
    } else {
        let cursor = Cursor::new(bytes);
        let reader: Box<dyn Read> = if name.ends_with(".gz") || name.ends_with(".tgz") {
            Box::new(flate2::read::GzDecoder::new(cursor))
        } else if name.ends_with(".bz2") || name.ends_with(".tbz") || name.ends_with(".tbz2") {
            Box::new(bzip2::read::BzDecoder::new(cursor))
        } else if name.ends_with(".xz") || name.ends_with(".txz") {
            Box::new(liblzma::read::XzDecoder::new(cursor))
        } else if name.ends_with(".zst") || name.ends_with(".zstd") {
            Box::new(zstd::stream::read::Decoder::new(cursor)?)
        } else {
            Box::new(cursor)
        };
        let mut a = tar::Archive::new(reader);
        for e in a.entries()? {
            let mut e = e?;
            if !e.header().entry_type().is_file() || e.size() > max {
                continue;
            };
            let p = e.path()?.into_owned();
            if p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!("unsafe archive member name")
            };
            if !sel.file(&p, o)? {
                continue;
            };
            let n = p.to_string_lossy().into_owned();
            let mut b = Vec::new();
            e.by_ref().take(max + 1).read_to_end(&mut b)?;
            if b.len() as u64 <= max {
                members.push((n, b))
            };
        }
    }
    for (n, b) in members {
        if archive_kind(&n) {
            archive(&n, b, o, sel, sources, level + 1)?
        } else {
            sources.push(Source {
                name: format!("{name}/{n}"),
                key: n,
                bytes: b,
                explicit: false,
            })
        }
    }
    Ok(())
}
pub fn collect(o: &Options, inputs: &[String]) -> Result<Vec<Source>> {
    let sel = Selection::new(o)?;
    let mut sources = Vec::new();
    let mut paths = inputs.to_vec();
    for p in o.all("list-file") {
        paths.extend(list(p)?)
    }
    if let Some(vcs) = o
        .value("vcs")
        .filter(|_| !o.flag("git") || paths.is_empty())
    {
        let roots = if paths.is_empty() {
            vec![".".to_owned()]
        } else {
            paths.clone()
        };
        paths.clear();
        for root in roots {
            let p = Path::new(&root);
            let vcs = if vcs == "auto" {
                if p.join(".git").exists() {
                    "git"
                } else if p.join(".svn").exists() {
                    "svn"
                } else {
                    ""
                }
            } else {
                vcs
            };
            if vcs.is_empty() {
                paths.push(root);
                continue;
            }
            if vcs == "git" {
                let mut args = vec!["ls-files", "-z"];
                if o.flag("include-submodules") {
                    args.push("--recurse-submodules")
                };
                let out = git(p, &args)?;
                for n in out.split(|b| *b == 0).filter(|n| !n.is_empty()) {
                    paths.push(
                        p.join(String::from_utf8_lossy(n).as_ref())
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            } else {
                let cmd = if vcs == "svn" { "svn list -R" } else { vcs };
                let out = Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(p)
                    .output()?;
                if !out.status.success() {
                    bail!("file listing command failed: {cmd}")
                };
                paths.extend(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter(|s| !s.ends_with('/'))
                        .map(|s| p.join(s).to_string_lossy().into_owned()),
                );
            }
        }
    }
    let mut seen = HashSet::new();
    for arg in paths {
        if arg == "-" {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes)?;
            let n = o
                .value("stdin-name")
                .context("standard input requires --stdin-name")?;
            sources.push(Source {
                name: n.into(),
                key: n.into(),
                bytes,
                explicit: true,
            });
            continue;
        }
        let path = Path::new(&arg);
        if !path.exists() {
            // Resolve a revision safely as a commit before passing it to other git commands.
            let revision = format!("{arg}^{{commit}}");
            if let Ok(hash) = git(
                Path::new("."),
                &["rev-parse", "--verify", "--end-of-options", &revision],
            ) {
                let hash = String::from_utf8(hash)?.trim().to_owned();
                let tree = git(
                    Path::new("."),
                    &["ls-tree", "-r", "-z", "--name-only", &hash],
                )?;
                for n in tree.split(|b| *b == 0).filter(|n| !n.is_empty()) {
                    let n = String::from_utf8(n.to_vec()).context("git path is not UTF-8")?;
                    if !sel.file(Path::new(&n), o)? {
                        continue;
                    };
                    let spec = format!("{hash}:{n}");
                    if let Ok(bytes) = git(Path::new("."), &["show", &spec]) {
                        sources.push(Source {
                            name: n.clone(),
                            key: n,
                            bytes,
                            explicit: false,
                        })
                    }
                }
                continue;
            }
            bail!("input does not exist and is not a git revision: {arg}")
        }
        let files: Vec<PathBuf> = if path.is_dir() {
            let depth = if o.flag("no-recurse") { 1 } else { usize::MAX };
            let mut files = Vec::new();
            for entry in WalkDir::new(path)
                .max_depth(depth)
                .follow_links(o.flag("follow-links"))
                .into_iter()
                .filter_entry(|e| !e.file_type().is_dir() || sel.dir(e.path(), o).unwrap_or(false))
            {
                let entry = entry.with_context(|| format!("traversing {}", path.display()))?;
                if entry.file_type().is_file()
                    || (entry.file_type().is_symlink() && entry.path().is_file())
                {
                    files.push(entry.path().into())
                }
            }
            files
        } else {
            vec![path.into()]
        };
        for p in files {
            if !sel.file(&p, o)? {
                continue;
            };
            let canonical = fs::canonicalize(&p)?;
            if !seen.insert(canonical) {
                continue;
            };
            let explicit = path.is_file();
            let len = fs::metadata(&p)?.len();
            if !explicit && len as f64 > o.number("max-file-size", 100.0)? * 1024.0 * 1024.0 {
                continue;
            };
            let n = p.to_string_lossy().into_owned();
            if let Some(pattern) = o.value("skip-archive") {
                if matches(pattern, &n)? {
                    continue;
                }
            }
            if let Some(cmd) = o.value("extract-with") {
                let scratch = crate::scratch::Scratch::new(o.value("sdir"))?;
                scratch.extract(cmd, &p)?;
                let mut extracted = o.clone();
                extracted.values.remove("extract-with");
                extracted.values.remove("list-file");
                extracted.values.remove("vcs");
                let mut items =
                    collect(&extracted, &[scratch.path.to_string_lossy().into_owned()])?;
                for item in &mut items {
                    item.name = format!("{n}/{}", item.key);
                }
                sources.extend(items);
                continue;
            }
            let bytes = fs::read(&p).with_context(|| format!("reading {n}"))?;
            if archive_kind(&n) {
                archive(&n, bytes, o, &sel, &mut sources, 0)?
            } else {
                let key = if path.is_dir() {
                    p.strip_prefix(path)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                sources.push(Source {
                    name: n,
                    key,
                    bytes,
                    explicit,
                });
            }
        }
    }
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sources)
}
pub fn report(o: &Options, registry: &Registry, inputs: &[String]) -> Result<Report> {
    let start = Instant::now();
    let sources = collect(o, inputs)?;
    let mut report = Report {
        found: sources.iter().map(|s| s.name.clone()).collect(),
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::new();
    for s in sources {
        let result = (|| -> Result<Option<(Source, String, String)>> {
            let Some(text) = counter::decode(&s.bytes, o.flag("unicode")).or_else(|| {
                o.flag("read-binary-files")
                    .then(|| String::from_utf8_lossy(&s.bytes).into_owned())
            }) else {
                report.ignored.push((s.name.clone(), "binary file".into()));
                return Ok(None);
            };
            let Some(lang) = registry.detect(&s.name, &text, o) else {
                report
                    .ignored
                    .push((s.name.clone(), "language not recognized".into()));
                return Ok(None);
            };
            let include = o.list("include-lang");
            let exclude = o.list("exclude-lang");
            if (!include.is_empty() && !include.iter().any(|n| n.eq_ignore_ascii_case(&lang.name)))
                || exclude.iter().any(|n| n.eq_ignore_ascii_case(&lang.name))
            {
                report
                    .ignored
                    .push((s.name.clone(), "language excluded".into()));
                return Ok(None);
            }
            for p in o.all("include-content") {
                if !matches(p, &text)? {
                    report
                        .ignored
                        .push((s.name.clone(), "content excluded".into()));
                    return Ok(None);
                }
            }
            for p in o.all("exclude-content") {
                if matches(p, &text)? {
                    report
                        .ignored
                        .push((s.name.clone(), "content excluded".into()));
                    return Ok(None);
                }
            }
            if !o.flag("skip-uniqueness") && !seen.insert(*blake3::hash(&s.bytes).as_bytes()) {
                report
                    .ignored
                    .push((s.name.clone(), "duplicate file".into()));
                return Ok(None);
            }
            if s.bytes.is_empty() {
                report.ignored.push((s.name.clone(), "empty file".into()));
                return Ok(None);
            };
            Ok(Some((s, text, lang.name.clone())))
        })()?;
        if let Some(v) = result {
            prepared.push(v)
        }
    }
    let count = |(s, text, name): &(Source, String, String)| {
        counter::count(s, text, registry.get(name).unwrap(), o)
    };
    let n = o.number("processes", 0.0)? as usize;
    let files: Vec<Result<_>> = if n > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()?
            .install(|| prepared.par_iter().map(count).collect::<Vec<_>>())
    } else {
        prepared.iter().map(count).collect::<Vec<_>>()
    };
    for ((source, _, _), result) in prepared.iter().zip(files) {
        let file = match result {
            Ok(f) => f,
            Err(e) if e.downcast_ref::<counter::TimedOut>().is_some() => {
                report.ignored.push((source.name.clone(), e.to_string()));
                if !o.flag("quiet") || o.flag("show-errors") {
                    eprintln!("{}: {e}", source.name)
                };
                continue;
            }
            Err(e) => return Err(e),
        };
        if !o.flag("quiet") && (o.has("v") || o.has("verbose")) {
            eprintln!("{}: {} code lines", file.name, file.counts.code)
        }

        if o.flag("print-filter-stages") {
            eprintln!(
                "{}\nOriginal:\n{}\nComments removed:\n{}",
                file.name,
                file.lines.join("\n"),
                file.stripped.join("\n")
            );
        }
        for (option, kind) in [("strip-comments", false), ("strip-code", true)] {
            if let Some(ext) = o.value(option) {
                let source = Path::new(&file.name);
                let target = if o.flag("original-dir") {
                    source.with_file_name(format!(
                        "{}.{ext}",
                        source.file_name().unwrap_or_default().to_string_lossy()
                    ))
                } else {
                    PathBuf::from(format!(
                        "{}.{ext}",
                        source.file_name().unwrap_or_default().to_string_lossy()
                    ))
                };
                if target.exists() {
                    bail!(
                        "refusing to overwrite existing stripped file {}",
                        target.display()
                    )
                };
                let lines: Vec<_> = file
                    .lines
                    .iter()
                    .zip(&file.kinds)
                    .filter_map(|(l, k)| {
                        if (!kind && *k == crate::model::Kind::Code)
                            || (kind && *k == crate::model::Kind::Comment)
                        {
                            Some(l.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                fs::write(
                    target,
                    if lines.is_empty() {
                        String::new()
                    } else {
                        lines.join("\n") + "\n"
                    },
                )?;
            }
        }
        report
            .languages
            .entry(file.language.clone())
            .or_default()
            .add(file.counts);
        report.total.add(file.counts);
        report.files.push(file);
        let progress = o.number("progress-rate", 100.0)? as usize;
        if !o.flag("quiet") && progress > 0 && report.files.len() % progress == 0 {
            eprintln!("{} files counted", report.files.len());
        }
    }
    report.elapsed = start.elapsed().as_secs_f64();
    for key in ["found", "counted", "unique", "ignored", "categorized"] {
        if let Some(p) = o.value(key) {
            let data = match key {
                "found" => report.found.join("\n"),
                "ignored" => report
                    .ignored
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                "categorized" => report
                    .files
                    .iter()
                    .map(|f| {
                        format!(
                            "{} {} {}",
                            f.lines.iter().map(String::len).sum::<usize>(),
                            f.language,
                            f.name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => report
                    .files
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            fs::write(p, data + "\n")?;
        }
    }
    Ok(report)
}
pub fn list_inputs(p: &str) -> Result<Vec<String>> {
    list(p)
}
