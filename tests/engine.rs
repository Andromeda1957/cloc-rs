use cloc_rs::{
    cli::Options,
    counter, diff,
    languages::Registry,
    model::{Counts, FileCount, Kind, Report, Source},
    output,
};
fn fixture(name: &str, text: &str, o: Options) -> FileCount {
    let registry = Registry::new(&o).unwrap();
    let lang = registry.detect(name, text, &o).unwrap();
    counter::count(
        &Source {
            name: name.into(),
            key: name.into(),
            bytes: text.as_bytes().into(),
            explicit: true,
        },
        text,
        lang,
        &o,
    )
    .unwrap()
}
#[test]
fn strings_and_comments() {
    let f = fixture(
        "test.c",
        "// comment\n\nint x; // inline\nchar *url = \"https://host/*path*/\";\n/* block\n end */\n",
        Options::default(),
    );
    assert_eq!(
        f.counts,
        Counts {
            files: 1,
            blank: 1,
            comment: 3,
            code: 2
        }
    );
    assert_eq!(f.stripped[2], "int x; ");
}
#[test]
fn python_docstrings() {
    let f = fixture(
        "test.py",
        "\"\"\"doc\n\nmore\n\"\"\"\nx=1\n",
        Options::default(),
    );
    assert_eq!(
        f.counts,
        Counts {
            files: 1,
            blank: 1,
            comment: 3,
            code: 1
        }
    );
    let mut o = Options::default();
    o.flags.insert("docstring-as-code".into());
    assert_eq!(
        fixture("test.py", "\"\"\"doc\nmore\n\"\"\"\n", o)
            .counts
            .code,
        3
    );
}
#[test]
fn rust_raw_strings_and_lifetimes() {
    let f = fixture(
        "test.rs",
        "let s = r###\"// text /* text */\"###;\nlet c = '/';\nfn f<'a>(x: &'a str) {} // tail\n",
        Options::default(),
    );
    assert_eq!(f.counts.code, 3);
    assert_eq!(f.counts.comment, 0);
}
#[test]
fn fixed_columns() {
    assert_eq!(
        fixture("test.f", "C comment\n      CALL F()\n", Options::default())
            .counts
            .comment,
        1
    );
    assert_eq!(
        fixture(
            "test.bat",
            "@REM comment\necho \"REM value\"\n",
            Options::default()
        )
        .counts
        .comment,
        1
    );
}
#[test]
fn byte_order_marks() {
    let mut b = vec![0xff, 0xfe];
    b.extend("# hi\nx=1\n".encode_utf16().flat_map(u16::to_le_bytes));
    assert!(counter::decode(&b, false).is_none());
    assert_eq!(counter::decode(&b, true).as_deref(), Some("# hi\nx=1\n"));
    let mut b = vec![0, 0, 0xfe, 0xff];
    b.extend("x=1\n".chars().flat_map(|c| (c as u32).to_be_bytes()));
    assert_eq!(counter::decode(&b, true).as_deref(), Some("x=1\n"));
}
#[test]
fn line_endings() {
    let a = fixture("test.py", "# comment\r\nx=1\r\n", Options::default());
    let b = fixture("test.py", "# comment\nx=1", Options::default());
    assert_eq!(a.counts, b.counts);
}
#[test]
fn ignored_lines() {
    let mut o = Options::default();
    o.set("ignore-regex", "Python|^print".into());
    let f = fixture("test.py", "print(1)\nx=1\n", o);
    assert_eq!(f.counts.code, 1);
    assert_eq!(f.kinds[0], Kind::Ignored);
}
#[test]
fn unicode_safe_for_every_language() {
    let o = Options::default();
    let r = Registry::new(&o).unwrap();
    for l in r.languages.values() {
        let text = "αβγ\n\n\"日本語\"\n☃\n";
        let source = Source {
            name: "test".into(),
            key: "test".into(),
            bytes: text.as_bytes().into(),
            explicit: true,
        };
        let f = counter::count(&source, text, l, &o).unwrap();
        assert_eq!(f.counts.lines(), 4, "{}", l.name);
    }
}
#[test]
fn language_detection() {
    let r = Registry::new(&Options::default()).unwrap();
    let o = Options::default();
    for (p, l) in [
        ("main.C", "C++"),
        ("main.c", "C"),
        ("main.cs", "C#"),
        ("Makefile", "make"),
        ("CMakeLists.txt", "CMake"),
        ("pom.xml", "Maven"),
        ("main.ts", "TypeScript"),
    ] {
        assert_eq!(r.detect(p, "", &o).unwrap().name, l);
    }
}
#[test]
fn names_that_decide_language_without_content() {
    let o = Options::default();
    let r = Registry::new(&o).unwrap();
    for (p, l) in [
        ("data.yaml", "YAML"),
        ("README.md", "Markdown"),
        ("main.cs", "C#"),
        ("Makefile", "make"),
    ] {
        assert_eq!(r.named(p, &o).map(|l| l.name.as_str()), Some(l), "{p}");
    }
    // Content, a shebang, or nothing at all decides these.
    for p in [
        "main.m",
        "main.pl",
        "main.ts",
        "main.v",
        "script",
        "notes.zzz",
    ] {
        assert!(r.named(p, &o).is_none(), "{p}");
    }
}
#[test]
fn long_minified_lines() {
    // Many strings and comments, with the only backslashes near the start.
    let text = "a='\\\\'; ".to_owned() + &"b=\"x\"; /* c */ d='e'; ".repeat(20_000);
    let f = fixture("min.js", &(text + "// tail\n"), Options::default());
    assert_eq!((f.counts.code, f.counts.comment), (1, 0));
    let expected = "a='\\\\'; ".to_owned() + &"b=\"x\";  d='e'; ".repeat(20_000);
    assert_eq!(f.stripped[0], expected);
    let f = fixture(
        "nest.hs",
        "{- a {- b -} c -} x\n{- {- -}\n -}\n",
        Options::default(),
    );
    assert_eq!((f.counts.code, f.counts.comment), (1, 2));
}
#[test]
fn script_tags_ignore_only_ascii_case() {
    let f = fixture(
        "page.html",
        "<SCRIPT>\n// a\n</Script>\n// b\n<scr\u{130}pt>\n// c\n",
        Options::default(),
    );
    use Kind::{Code, Comment};
    assert_eq!(f.kinds, [Code, Comment, Code, Code, Code, Code]);
}
#[test]
fn literate() {
    let f = fixture(
        "file.Rmd",
        "Prose\n\n```{r}\n# comment\nx <- 1\n```\n",
        Options::default(),
    );
    assert_eq!(
        f.counts,
        Counts {
            files: 1,
            blank: 1,
            comment: 4,
            code: 1
        }
    );
}
#[test]
fn skipped_lines() {
    let mut o = Options::default();
    o.set("skip-leading", "1,py".into());
    let f = fixture("main.py", "# skip\nx=1\n", o);
    assert_eq!(f.counts.lines(), 1);
}
#[test]
fn diff_identity_includes_blanks() {
    let f = fixture("file.py", "# comment\n\nx=1\n", Options::default());
    let mut r = Report::default();
    r.files.push(f);
    let d = diff::compare_reports(&r, &r, &Options::default()).unwrap();
    assert_eq!(
        d.total.same,
        Counts {
            files: 1,
            blank: 1,
            comment: 1,
            code: 1
        }
    );
}
#[test]
fn diff_counts_inline_comment_changes() {
    let a = fixture("file.py", "x=1 # old\n", Options::default());
    let b = fixture("file.py", "x=1 # new\n", Options::default());
    let mut old = Report::default();
    old.files.push(a);
    let mut new = Report::default();
    new.files.push(b);
    let d = diff::compare_reports(&old, &new, &Options::default()).unwrap();
    assert_eq!(d.total.modified.code, 1);
    assert_eq!(d.total.modified.files, 1);
}
#[test]
fn json_and_yaml_output() {
    let mut r = Report::default();
    let f = fixture("file.py", "# comment\nx=1\n", Options::default());
    r.total = f.counts;
    r.languages.insert(f.language.clone(), f.counts);
    r.files.push(f);
    let mut o = Options::default();
    o.flags.insert("json".into());
    let v: serde_json::Value =
        serde_json::from_str(&output::render_count(&r, &o).unwrap()).unwrap();
    assert_eq!(v["Python"]["code"], 1);
    o.flags.remove("json");
    o.flags.insert("yaml".into());
    let v: serde_json::Value =
        serde_yaml::from_str(&output::render_count(&r, &o).unwrap()).unwrap();
    assert_eq!(v["SUM"]["nFiles"], 1);
}
#[test]
fn by_file_by_language_valid_json() {
    let mut r = Report::default();
    let f = fixture("file.py", "x=1\n", Options::default());
    r.total = f.counts;
    r.languages.insert(f.language.clone(), f.counts);
    r.files.push(f);
    let mut o = Options::default();
    o.flags
        .extend(["json", "by-file", "by-file-by-lang"].map(str::to_owned));
    let v = output::count_value(&r, &o).unwrap();
    assert_eq!(v["file.py"]["code"], 1);
    assert_eq!(v["Python"]["code"], 1);
}
#[test]
fn third_generation() {
    let mut r = Report::default();
    r.languages.insert(
        "Python".into(),
        Counts {
            files: 1,
            code: 2,
            ..Default::default()
        },
    );
    r.total = r.languages["Python"];
    let mut o = Options::default();
    o.flags.insert("3".into());
    let v = output::count_value(&r, &o).unwrap();
    assert_eq!(v["Python"]["factor"], 4.2);
    assert_eq!(v["SUM"]["scaled"], 8.4);
}
