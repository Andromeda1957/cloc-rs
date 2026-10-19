use crate::cli::Options;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Language {
    pub name: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub line: Vec<String>,
    #[serde(default)]
    pub block: Vec<(String, String)>,
    #[serde(default)]
    pub quotes: Vec<(String, String)>,
    #[serde(default)]
    pub verbatim: Vec<(String, String)>,
    #[serde(default)]
    pub doc: Vec<(String, String)>,
    #[serde(default)]
    pub nested: bool,
    #[serde(default)]
    pub filters: Vec<(String, String)>,
}
/// Texts that, with empty text, reach every content rule in [`Registry::detect`]:
/// the first meets each rule's first test, the second the alternative `.m`
/// test. A new content rule must be met by one of them, and missed by another.
const CONTENT_PROBES: [&str; 2] = ["<?xml #import :- Require Import\n", "(*\n"];
#[derive(Clone)]
pub struct Registry {
    pub languages: BTreeMap<String, Language>,
    pub extensions: BTreeMap<String, String>,
    filenames: BTreeMap<String, String>,
}
impl Registry {
    pub fn new(o: &Options) -> Result<Self> {
        let defs: Vec<Language> = serde_json::from_str(include_str!("../data/catalog.json"))?;
        let mut r = Self {
            languages: BTreeMap::new(),
            extensions: BTreeMap::new(),
            filenames: BTreeMap::new(),
        };
        if !o.has("force-lang-def") {
            for d in defs {
                r.insert(d);
            }
        }
        if !o.has("force-lang-def") {
            let map: BTreeMap<String, String> =
                serde_json::from_str(include_str!("../data/extensions.json"))?;
            r.extensions
                .extend(map.into_iter().filter(|(_, n)| r.languages.contains_key(n)));
        }
        // Resolve historically ambiguous extensions by independently inspecting content.
        for (ext, name) in [
            ("cs", "C#"),
            ("m", "MATLAB"),
            ("pl", "Perl"),
            ("inc", "PHP"),
            ("ts", "TypeScript"),
            ("v", "Verilog"),
            ("d", "D"),
            ("h", "C/C++ Header"),
            ("C", "C++"),
            ("F", "Fortran 77"),
            ("f", "Fortran 77"),
        ] {
            if r.languages.contains_key(name) {
                r.extensions.insert(ext.into(), name.into());
            }
        }
        for p in o
            .all("read-lang-def")
            .into_iter()
            .chain(o.all("force-lang-def"))
        {
            r.read(p)?;
        }
        for val in o.all("force-lang") {
            let (n, e) = val.split_once(',').unwrap_or((val, ""));
            let name = r
                .get(n)
                .with_context(|| format!("unknown language {n}"))?
                .name
                .clone();
            if !e.is_empty() {
                r.extensions.insert(e.trim_start_matches('.').into(), name);
            }
        }
        for d in r.languages.values() {
            for (a, b) in d
                .block
                .iter()
                .chain(&d.quotes)
                .chain(&d.doc)
                .chain(&d.verbatim)
            {
                if a.is_empty() || b.is_empty() {
                    bail!("{}: comment and string delimiters cannot be empty", d.name)
                }
            }
        }
        Ok(r)
    }
    pub fn insert(&mut self, d: Language) {
        for ext in &d.extensions {
            self.extensions.insert(ext.clone(), d.name.clone());
        }
        let name = d.name.clone();
        let filenames = d.filenames.clone();
        if self.languages.insert(name.clone(), d).is_some() {
            // Custom definitions may replace a language's filename rules.
            self.filenames.clear();
            for language in self.languages.values() {
                for filename in &language.filenames {
                    self.filenames
                        .entry(filename.to_ascii_lowercase())
                        .or_insert_with(|| language.name.clone());
                }
            }
        } else {
            for filename in filenames {
                self.filenames
                    .entry(filename.to_ascii_lowercase())
                    .and_modify(|previous| {
                        // Preserve the previous alphabetical language priority.
                        if name < *previous {
                            *previous = name.clone();
                        }
                    })
                    .or_insert_with(|| name.clone());
            }
        }
    }
    /// Whether a file name ending in `.{ext}` can map to a language by its
    /// name alone, through an extension, a compound extension, or a filename.
    pub(crate) fn maps_suffix(&self, ext: &str) -> bool {
        let dotted = format!(".{ext}");
        self.extensions
            .keys()
            .chain(self.filenames.keys())
            .any(|k| k == ext || k.ends_with(&dotted))
    }
    /// The language a file name selects whatever the file contains, if any.
    ///
    /// Empty text and [`CONTENT_PROBES`] together reach every content rule in
    /// [`Registry::detect`], so a name whose detection depends on content
    /// gives different answers for them and is not reported here.
    pub fn named<'a>(&'a self, path: &str, o: &Options) -> Option<&'a Language> {
        let language = self.detect(path, "", o)?;
        CONTENT_PROBES
            .iter()
            .all(|text| {
                self.detect(path, text, o)
                    .is_some_and(|other| std::ptr::eq(other, language))
            })
            .then_some(language)
    }
    pub fn get(&self, name: &str) -> Option<&Language> {
        self.languages.get(name).or_else(|| {
            self.languages
                .values()
                .find(|d| d.name.eq_ignore_ascii_case(name))
        })
    }
    pub fn detect<'a>(&'a self, path: &str, text: &str, o: &Options) -> Option<&'a Language> {
        if let Some(v) = o
            .all("force-lang")
            .into_iter()
            .rev()
            .find(|v| !v.contains(','))
        {
            return self.get(v);
        }
        let filename = Path::new(path).file_name()?.to_str()?;
        if ["Makefile", "GNUmakefile", "makefile"].contains(&filename) {
            return self.get("make");
        };
        if filename == "CMakeLists.txt" {
            return self.get("CMake");
        };
        if let Some(name) = self.filenames.get(&filename.to_ascii_lowercase()) {
            return self.get(name);
        }
        if filename == "build.xml" {
            return self.get("Ant");
        };
        if filename == "pom.xml" {
            return self.get("Maven");
        };
        let full = if o.flag("ignore-case-ext") || o.flag("windows") {
            filename.to_lowercase()
        } else {
            filename.to_owned()
        };
        let full = if o.flag("autoconf") {
            full.strip_suffix(".in").unwrap_or(&full)
        } else {
            &full
        };
        // Every content rule below must stay reachable from CONTENT_PROBES.
        let mut found = None;
        for (i, _) in full.match_indices('.') {
            let ext = &full[i + 1..];
            if let Some(n) = self.extensions.get(ext) {
                found = self.get(n);
                break;
            }
        }
        if found.is_none() {
            found = self.extensions.get(full).and_then(|n| self.get(n));
        }
        if let Some(d) = found {
            if full.ends_with(".m") {
                if text.contains("#import")
                    || text.contains("@interface")
                    || text.contains("@implementation")
                {
                    return self.get("Objective-C");
                };
                if text.contains("(*") || text.contains("BeginPackage[") {
                    return self.get("Mathematica");
                };
            }
            if full.ends_with(".pl") && (text.contains(":-") || text.contains("?-")) {
                return self.get("Prolog");
            };
            if full.ends_with(".ts") && text.trim_start().starts_with("<?xml") {
                return self.get("Qt Linguist");
            };
            if full.ends_with(".v")
                && (text.contains("Require Import") || text.contains("Theorem "))
            {
                return self.get("Coq");
            };
            return Some(d);
        }
        if !filename.contains('.') {
            if let Some(n) = o.value("lang-no-ext") {
                return self.get(n);
            };
        }
        let first = text.lines().next().unwrap_or("");
        if first.starts_with("#!") {
            for v in o.all("script-lang") {
                if let Some((n, exe)) = v.split_once(',') {
                    if first
                        .split_whitespace()
                        .any(|s| s.rsplit('/').next() == Some(exe))
                    {
                        return self.get(n);
                    }
                }
            }
            let exe = first
                .split_whitespace()
                .rev()
                .find(|s| !s.starts_with('-'))?
                .rsplit('/')
                .next()?;
            let name = if exe.starts_with("python") {
                "Python"
            } else {
                match exe {
                    "sh" | "dash" | "ash" => "Bourne Shell",
                    "bash" => "Bourne Again Shell",
                    "zsh" => "zsh",
                    "perl" => "Perl",
                    "ruby" => "Ruby",
                    "node" | "nodejs" => "JavaScript",
                    "php" => "PHP",
                    "Rscript" => "R",
                    "fish" => "fish",
                    _ => "",
                }
            };
            if let Some(d) = self.get(name) {
                return Some(d);
            };
            return self
                .languages
                .values()
                .find(|d| d.env.iter().any(|s| s == exe));
        }
        None
    }
    fn read(&mut self, p: &str) -> Result<()> {
        let text =
            fs::read_to_string(p).with_context(|| format!("reading language definition {p}"))?;
        if text.trim_start().starts_with('[') {
            for d in serde_json::from_str::<Vec<Language>>(&text)? {
                self.insert(d)
            }
            return Ok(());
        }
        let mut d: Option<Language> = None;
        for line in text
            .lines()
            .filter(|s| !s.trim().is_empty() && !s.trim_start().starts_with('#'))
        {
            if !line.starts_with(char::is_whitespace) {
                if let Some(d) = d.take() {
                    self.insert(d)
                };
                d = Some(Language {
                    name: line.trim().into(),
                    ..Default::default()
                });
                continue;
            }
            let def = d
                .as_mut()
                .context("filter appears before a language name")?;
            let line = line.trim();
            let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            match key {
                "extension" => def
                    .extensions
                    .extend(rest.split_whitespace().map(str::to_owned)),
                "filename" => def.filenames.push(rest.into()),
                "filter" => {
                    let (f, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    match f {
                        "remove_matches" => def.filters.push((f.into(), arg.trim().into())),
                        "remove_inline" => def.line.push(arg.trim().into()),
                        "remove_between_general" => {
                            let v: Vec<_> = arg.split_whitespace().collect();
                            if v.len() < 2 {
                                bail!("{p}: remove_between_general requires start and end")
                            };
                            def.block.push((v[0].into(), v[1].into()));
                        }
                        "call_regexp_common" => match arg {
                            "C" | "C++" | "Java" | "C#" => {
                                def.line.push("//".into());
                                def.block.push(("/*".into(), "*/".into()));
                            }
                            "HTML" | "XML" => def.block.push(("<!--".into(), "-->".into())),
                            "Python" | "Perl" | "Ruby" => def.line.push("#".into()),
                            _ => bail!("{p}: unsupported comment syntax {arg}"),
                        },
                        _ => bail!(
                            "{p}: unsupported custom filter {f}; JSON definitions accept explicit syntax"
                        ),
                    }
                }
                "3rd_gen_scale" => (),
                _ => bail!("{p}: unknown definition directive {key}"),
            }
        }
        if let Some(d) = d {
            self.insert(d)
        };
        Ok(())
    }
    pub fn show(&self, ext: bool, query: &str) -> String {
        if ext {
            let mut out =
                "Extension       Language\n---------       --------------------\n".to_owned();
            for (e, n) in &self.extensions {
                if query.is_empty() || e == query {
                    out += &format!("{e:16}{n}\n")
                }
            }
            out
        } else {
            let mut out="Language                   Extension(s)\n--------------------       ----------------------------------------\n".to_owned();
            for d in self.languages.values() {
                if query.is_empty() || d.name.eq_ignore_ascii_case(query) {
                    out += &format!("{:27}({})\n", d.name, d.extensions.join(", "))
                }
            }
            out
        }
    }
    pub fn write(&self, p: &str) -> Result<()> {
        if p.ends_with(".json") {
            fs::write(
                p,
                serde_json::to_string_pretty(&self.languages.values().collect::<Vec<_>>())?,
            )?;
            return Ok(());
        }
        let mut out = String::new();
        for d in self.languages.values() {
            out += &format!("{}\n", d.name);
            for e in &d.extensions {
                out += &format!("    extension {e}\n")
            }
            for l in &d.line {
                out += &format!("    filter remove_inline {l}\n")
            }
            for (a, b) in &d.block {
                out += &format!("    filter remove_between_general {a} {b}\n")
            }
            for (f, a) in &d.filters {
                out += &format!("    filter {f} {a}\n")
            }
            out += "    3rd_gen_scale 1.00\n\n";
        }
        fs::write(p, out)?;
        Ok(())
    }
}
