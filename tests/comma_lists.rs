use cloc_rs::{cli::Options, input, languages::Registry};

fn count(option: &str, value: &str) -> cloc_rs::model::Report {
    let mut o = Options::default();
    o.set(option, value.into());
    let root = std::env::temp_dir().join(format!("comma-lists-{}-{option}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("source.py");
    std::fs::write(&path, "x=1\n").unwrap();
    let registry = Registry::new(&o).unwrap();
    let result = input::report(&o, &registry, &[root.to_string_lossy().into_owned()]);
    std::fs::remove_dir_all(&root).unwrap();
    result.unwrap()
}

#[test]
fn empty_language_entries_are_ignored() {
    let mut o = Options::default();
    o.set("exclude-lang", "Text,JSON,, C#, ".into());
    assert_eq!(o.list("exclude-lang"), ["Text", "JSON", "C#"]);
}

#[test]
fn empty_included_languages_are_ignored() {
    let mut o = Options::default();
    o.set("include-lang", "Python,".into());
    assert_eq!(o.list("include-lang"), ["Python"]);
    assert_eq!(count("include-lang", "Python,").total.code, 1);
}

#[test]
fn empty_excluded_directories_do_not_exclude_root() {
    assert_eq!(count("exclude-dir", "missing,").total.code, 1);
}
