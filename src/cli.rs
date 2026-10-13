use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub flags: BTreeSet<String>,
    pub values: BTreeMap<String, Vec<String>>,
    pub inputs: Vec<String>,
}
const FLAGS: &str = "only-count-files read-binary-files help version unicode autoconf by-file by-file-by-lang count-and-diff diff docstring-as-code follow-links git git-diff-rel git-diff-all ignore-whitespace ignore-case ignore-case-ext no-recurse original-dir skip-uniqueness stat strip-str-comments sum-reports unix windows include-submodules fullpath skip-win-hidden print-filter-stages show-os 3 csv hide-rate json md percent quiet show-errors sql-append sum-one xml yaml use-sloccount";
const VALUES: &str = "skip-leading stdin-name extract-with list-file diff-list-file files-from vcs config diff-timeout force-lang force-lang-def ignore-regex lang-no-ext max-file-size read-lang-def script-lang sdir strip-code strip-comments timeout processes include-content exclude-content exclude-dir exclude-ext exclude-lang exclude-list-file include-ext include-lang match-d not-match-d match-f not-match-f skip-archive categorized counted diff-alignment explain found ignored unique write-lang-def write-lang-def-incl-dup by-percent csv-delimiter file-encoding fmt out progress-rate report-file sql sql-project sql-style summary-cutoff thousands-delimiter ksep xsl";
const OPTIONAL: &str = "show-lang show-ext no-autogen verbose v";
impl Options {
    pub fn flag(&self, k: &str) -> bool {
        self.flags.contains(k)
    }
    pub fn value(&self, k: &str) -> Option<&str> {
        self.values
            .get(k)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
    pub fn all(&self, k: &str) -> Vec<&str> {
        self.values
            .get(k)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }
    pub fn has(&self, k: &str) -> bool {
        self.flag(k) || self.values.contains_key(k)
    }
    pub fn number(&self, k: &str, default: f64) -> Result<f64> {
        match self.value(k) {
            Some(v) => {
                let n: f64 = v
                    .parse()
                    .with_context(|| format!("--{k} requires a number"))?;
                if !n.is_finite() || n < 0.0 {
                    bail!("--{k} must be a finite nonnegative number")
                };
                Ok(n)
            }
            None => Ok(default),
        }
    }
    pub fn list(&self, k: &str) -> Vec<String> {
        self.all(k)
            .iter()
            .flat_map(|v| v.split(','))
            .map(|s| s.trim().to_owned())
            .collect()
    }
    pub fn set(&mut self, k: &str, v: String) {
        self.values.entry(k.to_owned()).or_default().push(v);
    }
    pub fn parse() -> Result<Self> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        let mut opts = Self::default();
        let config = raw
            .iter()
            .find_map(|s| s.strip_prefix("--config=").map(PathBuf::from))
            .or_else(|| {
                raw.windows(2)
                    .find(|w| w[0] == "--config")
                    .map(|w| PathBuf::from(&w[1]))
            });
        let default = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("cloc-rs/options.txt"));
        if let Some(p) = config.as_ref().or(default.as_ref().filter(|p| p.is_file())) {
            let data =
                fs::read_to_string(p).with_context(|| format!("reading config {}", p.display()))?;
            let mut args = Vec::new();
            for line in data
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
            {
                let line = line.strip_prefix("--").unwrap_or(line);
                let (key, val) = line
                    .split_once('=')
                    .or_else(|| line.split_once(char::is_whitespace))
                    .unwrap_or((line, ""));
                args.push(if val.is_empty() {
                    format!("--{key}")
                } else {
                    format!(
                        "--{key}={}",
                        val.trim().trim_matches('"').trim_matches('\'')
                    )
                });
            }
            opts.consume(args)?;
        }
        opts.consume(raw)?;
        if opts.flag("by-file-by-lang") {
            opts.flags.insert("by-file".into());
        }
        if opts.has("diff-alignment")
            || opts.has("diff-list-file")
            || opts.has("diff-list-files")
            || opts.flag("git-diff-rel")
            || opts.flag("git-diff-all")
            || opts.flag("count-and-diff")
        {
            opts.flags.insert("diff".into());
        }
        if opts.flag("git-diff-all") || opts.flag("git-diff-rel") {
            opts.flags.insert("git".into());
        }
        if opts.has("csv-delimiter") {
            opts.flags.insert("csv".into());
        }
        if opts.has("xsl") {
            opts.flags.insert("xml".into());
        }
        if opts.flag("percent") {
            opts.set("by-percent", "t".into());
        }
        if let Some(v) = opts.value("fmt") {
            let n: u8 = v.parse().context("--fmt must be 1 through 5")?;
            if !(1..=5).contains(&n) {
                bail!("--fmt must be 1 through 5")
            };
            if n >= 3 {
                opts.flags.insert("by-file".into());
            }
        }
        for key in [
            "timeout",
            "diff-timeout",
            "max-file-size",
            "processes",
            "progress-rate",
        ] {
            opts.number(key, 0.0)?;
        }
        if let Some(v) = opts.value("by-percent") {
            if !["c", "cm", "cb", "cmb", "t"].contains(&v) {
                bail!("--by-percent must be c, cm, cb, cmb, or t")
            }
        }
        if opts.has("sql-append") && !opts.has("sql") {
            bail!("--sql-append requires --sql")
        }
        if opts.inputs.is_empty()
            && !opts.flags.contains("diff")
            && !opts.has("list-file")
            && !opts.has("vcs")
            && !opts.has("files-from")
            && !opts.has("help")
            && !opts.has("version")
            && !opts.has("show-lang")
            && !opts.has("show-ext")
            && !opts.has("show-os")
            && !opts.has("explain")
            && !opts.has("write-lang-def")
            && !opts.has("write-lang-def-incl-dup")
            && opts.value("no-autogen") != Some("list")
        {
            opts.flags.insert("help".into());
        }
        Ok(opts)
    }
    fn consume(&mut self, args: Vec<String>) -> Result<()> {
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                self.inputs.extend(args);
                break;
            }
            if !arg.starts_with('-') || arg == "-" {
                self.inputs.push(arg);
                continue;
            }
            let s = arg.trim_start_matches('-');
            let (mut key, val) = s
                .split_once('=')
                .map(|(a, b)| (a, Some(b.to_owned())))
                .unwrap_or((s, None));
            key = match key {
                "h" | "?" => "help",
                "V" => "version",
                "out" => "report-file",
                "files-from" => "vcs",
                "ksep" => "thousands-delimiter",
                k => k,
            };
            if key == "diff-list-files" {
                let a = val
                    .or_else(|| args.next())
                    .context("--diff-list-files requires two paths")?;
                let b = args
                    .next()
                    .context("--diff-list-files requires two paths")?;
                self.values.insert(key.into(), vec![a, b]);
                continue;
            }
            if FLAGS.split_whitespace().any(|k| k == key) {
                if val.is_some() {
                    bail!("--{key} does not accept a value")
                };
                if ["json", "yaml", "csv", "md", "xml"].contains(&key) {
                    for k in ["json", "yaml", "csv", "md", "xml"] {
                        self.flags.remove(k);
                    }
                }
                self.flags.insert(key.into());
            } else if VALUES.split_whitespace().any(|k| k == key) {
                let v = val
                    .or_else(|| args.next())
                    .with_context(|| format!("--{key} requires a value"))?;
                self.set(key, v);
            } else if OPTIONAL.split_whitespace().any(|k| k == key) {
                self.set(key, val.unwrap_or_default());
            } else {
                bail!("unknown option {arg}; use --help")
            }
        }
        Ok(())
    }
}
pub fn help() -> String {
    format!(
        "cloc-rs {} — Count Lines of Code in Rust\n\nUsage: cloc-rs [options] <files/directories/archives/git revisions>\n       cloc-rs --diff <old> <new>\n\nCounts blank, comment, and physical code lines. Mixed code/comment lines count as code.\n\nInput: --list-file FILE, --vcs git|auto|CMD, --git, --stdin-name FILE,\n       --unicode, --autoconf, --follow-links, --no-recurse, --max-file-size MB\nFilters: --include-lang LIST, --exclude-lang LIST, --include-ext LIST,\n         --exclude-ext LIST, --exclude-dir LIST, --exclude-list-file FILE,\n         --match-f REGEX, --not-match-f REGEX, --match-d REGEX, --not-match-d REGEX,\n         --fullpath, --include-content REGEX, --exclude-content REGEX,\n         --ignore-regex 'LANG,...|REGEX', --no-autogen\nCounting: --skip-leading N[,EXT], --only-count-files, --read-binary-files,\n          --by-file, --by-file-by-lang, --skip-uniqueness, --processes N,\n          --docstring-as-code, --force-lang LANG[,EXT], --lang-no-ext LANG,\n          --script-lang LANG,INTERPRETER, --read-lang-def FILE, --force-lang-def FILE\nDiff: --diff, --count-and-diff, --diff-list-file FILE, --diff-list-files OLD NEW,\n      --diff-alignment FILE, --ignore-case, --ignore-whitespace, --git-diff-all\nOutput: --3, --json, --yaml, --csv, --md, --xml, --sql FILE, --report-file FILE,\n        --csv-delimiter CHAR, --sql-project NAME, --sql-append, --xsl FILE,\n        --hide-rate, --quiet, --sum-one, --fmt 1..5, --by-percent c|cm|cb|cmb|t,\n        --summary-cutoff X:N, --thousands-delimiter CHAR, --sum-reports\nInspection: --show-lang[=LANG], --show-ext[=EXT], --explain LANG, --show-os,\n            --write-lang-def FILE, --counted FILE, --found FILE, --ignored FILE,\n            --unique FILE, --categorized FILE, --version, --help\nOther: --config FILE, --strip-comments EXT, --strip-code EXT, --original-dir,\n       --strip-str-comments, --print-filter-stages, --timeout N, --diff-timeout N\n\nConfiguration: $XDG_CONFIG_HOME/cloc-rs/options.txt or ~/.config/cloc-rs/options.txt.\nSyntax: --option=value and --option value. Independent MIT implementation; see\nREADME.md for the compatibility contract and verification procedure.\n",
        env!("CARGO_PKG_VERSION")
    )
}
