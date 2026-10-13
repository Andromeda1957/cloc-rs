use crate::{
    cli::Options,
    counter,
    languages::Registry,
    model::{Report, Source},
    progress::Progress,
};
use anyhow::{Context, Result, bail};
use fancy_regex::Regex;
use std::{
    borrow::Cow,
    collections::HashSet,
    fs,
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use walkdir::WalkDir;
mod counting;
const VCS_DIRS: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".cvs", ".snapshot"];
// Verified against the behavioral reference: these suffixes are ignored even
// when their contents have a shebang. Explicit language mappings override this.
const NON_SOURCE_EXTENSIONS: [&str; 8] = ["jpg", "png", "gif", "bmp", "tiff", "pdf", "mp3", "wav"];
pub const GENERATED: &str = "aclocal.m4,autom4te.cache,config.guess,config.sub,configure,depcomp,install-sh,ltmain.sh,missing,Makefile.in";
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
struct Selection {
    excluded: Vec<PathBuf>,
    cwd: PathBuf,
    excluded_dirs: HashSet<String>,
    included_ext: HashSet<String>,
    excluded_ext: HashSet<String>,
    match_files: Vec<Regex>,
    skip_files: Vec<Regex>,
    match_dirs: Vec<Regex>,
    skip_dirs: Vec<Regex>,
    fullpath: bool,
    no_autogen: bool,
    skip_hidden: bool,
}
impl Selection {
    fn new(o: &Options) -> Result<Self> {
        let mut excluded = Vec::new();
        for p in o.all("exclude-list-file") {
            excluded.extend(list(p)?.into_iter().map(|p| absolute(Path::new(&p))))
        }
        Ok(Self {
            excluded,
            cwd: std::env::current_dir()?,
            excluded_dirs: o.list("exclude-dir").into_iter().collect(),
            included_ext: o.list("include-ext").into_iter().collect(),
            excluded_ext: o.list("exclude-ext").into_iter().collect(),
            match_files: patterns(o, "match-f")?,
            skip_files: patterns(o, "not-match-f")?,
            match_dirs: patterns(o, "match-d")?,
            skip_dirs: patterns(o, "not-match-d")?,
            fullpath: o.flag("fullpath"),
            no_autogen: o.has("no-autogen"),
            skip_hidden: o.flag("skip-win-hidden"),
        })
    }
    fn excluded(&self, p: &Path) -> bool {
        if self.excluded.is_empty() {
            return false;
        }
        let a = if p.is_absolute() {
            Cow::Borrowed(p)
        } else {
            Cow::Owned(self.cwd.join(p))
        };
        self.excluded
            .iter()
            .any(|e| a.as_ref() == e || a.starts_with(e))
    }
    fn dir(&self, p: &Path) -> Result<bool> {
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if VCS_DIRS.contains(&n) || self.excluded_dirs.contains(n) || self.excluded(p) {
            return Ok(false);
        };
        if self.skip_dirs.is_empty() {
            return Ok(true);
        }
        let s = if self.fullpath {
            p.to_string_lossy()
        } else {
            Cow::Borrowed(n)
        };
        for r in &self.skip_dirs {
            if r.is_match(&s)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn file(&self, p: &Path) -> Result<bool> {
        if self.excluded(p) {
            return Ok(false);
        }
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if (!self.included_ext.is_empty() && !self.included_ext.contains(ext))
            || self.excluded_ext.contains(ext)
        {
            return Ok(false);
        }
        if self.no_autogen && GENERATED.split(',').any(|s| s == n) {
            return Ok(false);
        }
        if self.skip_hidden && n.starts_with('.') {
            return Ok(false);
        }
        let s = if self.fullpath {
            p.to_string_lossy()
        } else {
            Cow::Borrowed(n)
        };
        for r in &self.match_files {
            if !r.is_match(&s)? {
                return Ok(false);
            }
        }
        for r in &self.skip_files {
            if r.is_match(&s)? {
                return Ok(false);
            }
        }
        for r in &self.match_dirs {
            if !r.is_match(&p.parent().unwrap_or(Path::new("")).to_string_lossy())? {
                return Ok(false);
            }
        }
        // Apply directory exclusions to git and archive member lists too.
        for ancestor in p.ancestors().skip(1) {
            if !self.dir(ancestor)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
fn patterns(o: &Options, key: &str) -> Result<Vec<Regex>> {
    o.all(key)
        .into_iter()
        .map(|p| Regex::new(p).with_context(|| format!("invalid regular expression {p}")))
        .collect()
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
fn ignored_media(p: &Path, registry: &Registry, o: &Options) -> bool {
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    NON_SOURCE_EXTENSIONS.contains(&ext) && registry.detect(&p.to_string_lossy(), "", o).is_none()
}
trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}
fn archive(
    name: &str,
    reader: Box<dyn ReadSeek>,
    o: &Options,
    sel: &Selection,
    emit: &mut dyn FnMut(Source) -> Result<()>,
    level: usize,
) -> Result<()> {
    if level > 8 {
        bail!("archive nesting exceeds 8 levels: {name}")
    };
    let max = o.number("max-file-size", 100.0)? as u64 * 1024 * 1024;
    if [".zip", ".jar", ".war", ".ear"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        let mut z = zip::ZipArchive::new(reader)?;
        for i in 0..z.len() {
            let mut entry = z.by_index(i)?;
            if entry.is_dir() || entry.size() > max {
                continue;
            };
            let p = entry
                .enclosed_name()
                .context("unsafe archive member name")?;
            let n = p.to_string_lossy().into_owned();
            if !sel.file(&p)? {
                continue;
            };
            let mut b = Vec::new();
            entry.by_ref().take(max + 1).read_to_end(&mut b)?;
            if b.len() as u64 <= max {
                archive_member(name, n, b, o, sel, emit, level)?;
            }
        }
    } else {
        let reader: Box<dyn Read> = if name.ends_with(".gz") || name.ends_with(".tgz") {
            Box::new(flate2::read::GzDecoder::new(reader))
        } else if name.ends_with(".bz2") || name.ends_with(".tbz") || name.ends_with(".tbz2") {
            Box::new(bzip2::read::BzDecoder::new(reader))
        } else if name.ends_with(".xz") || name.ends_with(".txz") {
            Box::new(liblzma::read::XzDecoder::new(reader))
        } else if name.ends_with(".zst") || name.ends_with(".zstd") {
            Box::new(zstd::stream::read::Decoder::new(reader)?)
        } else {
            Box::new(reader)
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
            if !sel.file(&p)? {
                continue;
            };
            let n = p.to_string_lossy().into_owned();
            let mut b = Vec::new();
            e.by_ref().take(max + 1).read_to_end(&mut b)?;
            if b.len() as u64 <= max {
                archive_member(name, n, b, o, sel, emit, level)?;
            };
        }
    }
    Ok(())
}
fn archive_member(
    name: &str,
    n: String,
    bytes: Vec<u8>,
    o: &Options,
    sel: &Selection,
    emit: &mut dyn FnMut(Source) -> Result<()>,
    level: usize,
) -> Result<()> {
    if archive_kind(&n) {
        archive(&n, Box::new(Cursor::new(bytes)), o, sel, emit, level + 1)
    } else {
        emit(Source {
            name: format!("{name}/{n}"),
            key: n,
            bytes,
            explicit: false,
        })
    }
}
pub fn collect(o: &Options, inputs: &[String]) -> Result<Vec<Source>> {
    let progress = Progress::new(o)?;
    progress.begin("Scanning", None);
    let mut sources = Vec::new();
    visit(o, inputs, &progress, &mut |source| {
        sources.push(source);
        Ok(())
    })?;
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    progress.finish(sources.len());
    Ok(sources)
}
fn visit(
    o: &Options,
    inputs: &[String],
    progress: &Progress,
    emit: &mut dyn FnMut(Source) -> Result<()>,
) -> Result<()> {
    let sel = Selection::new(o)?;
    let registry = Registry::new(o)?;
    let included_languages = o.list("include-lang");
    let excluded_languages = o.list("exclude-lang");
    let skip_archive = o.value("skip-archive").map(Regex::new).transpose()?;
    let max_file_size = o.number("max-file-size", 100.0)? * 1024.0 * 1024.0;
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
    let mut discovered = 0;
    for arg in paths {
        if arg == "-" {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes)?;
            let n = o
                .value("stdin-name")
                .context("standard input requires --stdin-name")?;
            emit(Source {
                name: n.into(),
                key: n.into(),
                bytes,
                explicit: true,
            })?;
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
                    discovered += 1;
                    progress.update("Scanning", discovered, None);
                    let n = String::from_utf8(n.to_vec()).context("git path is not UTF-8")?;
                    if !sel.file(Path::new(&n))? {
                        continue;
                    };
                    let spec = format!("{hash}:{n}");
                    if let Ok(bytes) = git(Path::new("."), &["show", &spec]) {
                        emit(Source {
                            name: n.clone(),
                            key: n,
                            bytes,
                            explicit: false,
                        })?;
                    }
                }
                continue;
            }
            bail!("input does not exist and is not a git revision: {arg}")
        }
        let directory = path.is_dir();
        let explicit = path.is_file();
        let mut files: Vec<PathBuf> = if directory {
            let depth = if o.flag("no-recurse") { 1 } else { usize::MAX };
            let mut files = Vec::new();
            for entry in WalkDir::new(path)
                .max_depth(depth)
                .follow_links(o.flag("follow-links"))
                .into_iter()
                .filter_entry(|e| !e.file_type().is_dir() || sel.dir(e.path()).unwrap_or(false))
            {
                let entry = entry.with_context(|| format!("traversing {}", path.display()))?;
                if entry.file_type().is_file()
                    || (entry.file_type().is_symlink() && entry.path().is_file())
                {
                    discovered += 1;
                    progress.update("Scanning", discovered, None);
                    if !ignored_media(entry.path(), &registry, o) {
                        files.push(entry.path().into());
                    }
                }
            }
            files
        } else {
            vec![path.into()]
        };
        files.sort();
        for p in files {
            if !sel.file(&p)? {
                continue;
            };
            if !directory && ignored_media(&p, &registry, o) {
                // Prune known non-source files before filesystem resolution,
                // metadata, opening, and probing. Large image corpora should
                // cost a directory walk, not hundreds of thousands of reads.
                continue;
            }
            let canonical = fs::canonicalize(&p)?;
            if !seen.insert(canonical) {
                continue;
            };
            // cloc expands archive arguments, but treats archives encountered
            // inside a directory as ordinary ignored files.
            if !explicit && archive_kind(&p.to_string_lossy()) {
                continue;
            }
            let len = fs::metadata(&p)?.len();
            if !explicit && len as f64 > max_file_size {
                continue;
            };
            let n = p.to_string_lossy().into_owned();
            if let Some(pattern) = &skip_archive {
                if pattern.is_match(&n)? {
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
                visit(
                    &extracted,
                    &[scratch.path.to_string_lossy().into_owned()],
                    progress,
                    &mut |mut item| {
                        item.name = format!("{n}/{}", item.key);
                        emit(item)
                    },
                )?;
                continue;
            }
            if archive_kind(&n) {
                archive(&n, Box::new(fs::File::open(&p)?), o, &sel, emit, 0)?;
                continue;
            }
            let bytes = {
                // Inspect a small prefix before reading an entire build artifact.
                // Keeping that prefix preserves found/ignored diagnostics while
                // avoiding full allocations for binary and unrecognized files.
                let mut file = fs::File::open(&p).with_context(|| format!("reading {n}"))?;
                let mut prefix = vec![0; 4096];
                let read = file.read(&mut prefix)?;
                prefix.truncate(read);
                let binary =
                    !o.flag("read-binary-files") && !o.flag("unicode") && prefix.contains(&0);
                let text = counter::decode(&prefix, o.flag("unicode"));
                let language = text
                    .as_deref()
                    .and_then(|text| registry.detect(&n, text, o));
                let ambiguous = ["m", "pl", "v", "ts", "inc", "d", "cs"]
                    .contains(&p.extension().and_then(|s| s.to_str()).unwrap_or(""));
                let excluded = !ambiguous
                    && language.is_some_and(|l| {
                        (!included_languages.is_empty()
                            && !included_languages
                                .iter()
                                .any(|n| n.eq_ignore_ascii_case(&l.name)))
                            || excluded_languages
                                .iter()
                                .any(|n| n.eq_ignore_ascii_case(&l.name))
                            || crate::basic::excluded_language(o, &l.name, &included_languages)
                    });
                let unknown =
                    language.is_none() && !o.flag("unicode") && !o.flag("read-binary-files");
                if binary {
                    // No consumer decodes a rejected binary. Retain only its
                    // classification marker, not a prefix per image/artifact.
                    vec![0]
                } else if unknown || excluded {
                    prefix
                } else {
                    file.read_to_end(&mut prefix)?;
                    prefix
                }
            };
            let key = if directory {
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
            emit(Source {
                name: n,
                key,
                bytes,
                explicit,
            })?;
        }
    }
    Ok(())
}
pub fn report(o: &Options, registry: &Registry, inputs: &[String]) -> Result<Report> {
    let start = Instant::now();
    let progress = Progress::new(o)?;
    progress.begin("Scanning", None);
    let mut pipeline = counting::Pipeline::new(o, registry, &progress)?;
    visit(o, inputs, &progress, &mut |source| pipeline.accept(source))?;
    let mut report = pipeline.finish()?;
    progress.finish(report.files.len());
    crate::basic::filter(&mut report, o)?;
    report.elapsed = start.elapsed().as_secs_f64();
    write_diagnostics(&report, o)?;
    Ok(report)
}
fn store_file(report: &mut Report, mut file: crate::model::FileCount, o: &Options) -> Result<()> {
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

    if !o.flag("retain-detail") {
        file.lines = Vec::new();
        file.kinds = Vec::new();
        file.stripped = Vec::new();
    }
    report.files.push(file);
    Ok(())
}
pub(crate) fn write_diagnostics(report: &Report, o: &Options) -> Result<()> {
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
                    .map(|f| format!("{} {} {}", f.original_bytes, f.language, f.name))
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
    Ok(())
}
pub fn list_inputs(p: &str) -> Result<Vec<String>> {
    list(p)
}
