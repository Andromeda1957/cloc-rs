use crate::{cli::Options, model::Report};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

const DIRECTORIES: &str = "node_modules,venv,vendor,target,.python_libs,release,build,dist,.recovery-tools,.venv,__pycache__,.mypy_cache,.ruff_cache,.pytest_cache,.gradle,coverage";
const DOCUMENTS: &[&str] = &[
    "Text",
    "JSON",
    "JSON5",
    "YAML",
    "Markdown",
    "TOML",
    "SVG",
    "XML",
    "INI",
    "Properties",
    "CSV",
    "reStructuredText",
    "AsciiDoc",
    "TeX",
    "ProGuard",
    "Gradle",
    "make",
    "CMake",
    "Dockerfile",
    "Containerfile",
    "Qt Project",
    "Autoconf",
    "Maven",
    "MSBuild script",
    "Visualforce Page",
    "BitBake",
    "Unknown/BitBake",
    "HCL",
    "Meson",
    "Bazel",
    "Ant",
    "XSLT",
];

pub(crate) fn configure(o: &mut Options) -> Result<()> {
    if !o.has("basic") {
        return Ok(());
    }
    if o.value("basic") == Some("") {
        o.set("basic", "1".into());
    }
    let percent = o.number("basic", 1.0)?;
    if percent > 100.0 {
        bail!("--basic percentage must be between 0 and 100");
    }
    o.set("exclude-dir", DIRECTORIES.into());
    Ok(())
}

pub(crate) fn excluded_language(o: &Options, name: &str, included: &[String]) -> bool {
    o.has("basic")
        && DOCUMENTS.iter().any(|n| n.eq_ignore_ascii_case(name))
        && !included.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn selected<'a>(
    reports: impl IntoIterator<Item = &'a Report>,
    o: &Options,
) -> Result<BTreeSet<String>> {
    let mut languages = BTreeMap::<String, u64>::new();
    let pinned = o.list("include-lang");
    for report in reports {
        for (name, counts) in &report.languages {
            if !excluded_language(o, name, &pinned) {
                *languages.entry(name.clone()).or_default() += counts.code;
            }
        }
    }
    let total: u64 = languages.values().sum();
    let threshold = o.number("basic", 1.0)?;
    Ok(languages
        .into_iter()
        .filter(|(name, code)| {
            total == 0
                || 100.0 * *code as f64 / total as f64 >= threshold
                || pinned.iter().any(|n| n.eq_ignore_ascii_case(name))
        })
        .map(|(name, _)| name)
        .collect())
}

fn retain(report: &mut Report, names: &BTreeSet<String>) {
    let ignored = &mut report.ignored;
    report.files.retain(|file| {
        if names.contains(&file.language) {
            true
        } else {
            ignored.push((
                file.name.clone(),
                "language below --basic percentage".into(),
            ));
            false
        }
    });
    report.languages.retain(|name, _| names.contains(name));
    report.total = Default::default();
    for counts in report.languages.values() {
        report.total.add(*counts);
    }
}

pub(crate) fn filter(report: &mut Report, o: &Options) -> Result<()> {
    if o.has("basic") {
        let names = selected([&*report], o)?;
        retain(report, &names);
    }
    Ok(())
}

pub(crate) fn filter_pair(old: &mut Report, new: &mut Report, o: &Options) -> Result<()> {
    if o.has("basic") {
        let names = selected([&*old, &*new], o)?;
        retain(old, &names);
        retain(new, &names);
    }
    Ok(())
}
