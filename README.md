# cloc-rs

An independently written, MIT-licensed Rust implementation of the
[cloc](https://github.com/AlDanial/cloc) command-line workflows. Counts physical
blank, comment, and code lines across 511 language definitions, including cloc
2.10's language roster, with additional syntax metadata from Tokei.

```sh
cloc-rs .
cloc-rs --json --exclude-dir=target,node_modules .
cloc-rs --by-file --include-lang=Rust,Python src
cloc-rs --vcs=git
cloc-rs --git HEAD
cloc-rs --diff old/ new/
cloc-rs --git --diff HEAD~1 HEAD
cloc-rs --csv --out=counts.csv project.tar.gz
cat main.py | cloc-rs --stdin-name=main.py -
```

## Install

Install Rust, then:

```sh
git clone https://github.com/Andromeda1957/cloc-rs.git
cd cloc-rs
cargo install --path . --locked
```

For `~/.local/bin`, use `cargo install --path . --locked --root "$HOME/.local"`.
The executable is `cloc-rs`, so it can coexist with Perl cloc. Perl is not required
at runtime. Git inputs require Git; SVN listings require SVN.

Native archives include zip/jar/war/ear, tar, and gzip/bzip2/xz/zstd tarballs.
They are read in memory without disk extraction. Other types can use an explicit
`--extract-with='COMMAND >FILE<'`. Custom extraction uses a unique temporary
directory removed after processing; `--sdir` selects its parent.

## Compatibility

The behavioral reference is cloc **2.10**. This is an independent implementation,
not a translation or relicensing of cloc's GPL source. The lexer, traversal,
filters, reports, and diff orchestration are written in Rust. Tokei syntax
metadata is used under MIT with its notice preserved in `licenses/`. Language
names, extensions, and numerical scale factors are public interface facts.
The program does not invoke or bundle cloc.

Workflows include recursive counting, duplicate detection, language and content
filters, filename/directory regular expressions, shebang detection, UTF-16 and
UTF-32 input with `--unicode`, forced languages, leading-line skipping, parallel
processing, configuration, file lists, local Git revisions, diffs, alignment
files, custom languages, code/comment stripping, report summation, and text,
JSON, YAML, CSV, Markdown, XML, and SQL reports.

Use `--help` for options, `--show-lang` for languages, and `--explain=LANGUAGE`
for syntax rules. Compatibility is verified through independently authored
snippets against an external cloc executable. This does not guarantee identical
counts for every possible file or byte-identical report formatting.

The following distinctions are deliberate:

- Headers identify cloc-rs and its version. UTF-8 is the default output encoding;
  alternate supported encodings use `--file-encoding`.
- JSON from `--by-file-by-lang` contains both file and language entries in one
  valid document, with one `SUM` entry.
- The lexer recognizes strings, including Rust and C++ raw strings. cloc's
  regex-based corner cases can give different counts. Rust block comments follow
  cloc's flat counting rule to match its default results.
- `--use-sloccount` selects the Rust engine instead of optional external counters.
  `--stat` is accepted because the walker already obtains metadata.
  `--strip-str-comments` is accepted because strings already protect comment
  markers.
- fancy-regex supports common Perl lookarounds and backreferences. Executable
  Perl regex extensions and arbitrary Perl filter subroutines are unsupported.
- Text language definitions accept `remove_inline`, `remove_matches`,
  `remove_between_general`, and standard-family `call_regexp_common` filters.
  JSON definitions support explicit comments, strings, docstrings, and nesting;
  `--write-lang-def=languages.json` provides the lossless export format.
- `--count-and-diff --out=report` writes `report.1`, `report.2`, and `report.diff`.
  File alignment uses relative paths, exact-content matching, and Git rename detection. Archive
  file reports use `archive/member` paths.

## Configuration and languages

Configuration lives at `$XDG_CONFIG_HOME/cloc-rs/options.txt`, falling back to
`~/.config/cloc-rs/options.txt`, or an explicit `--config=FILE`. Use one option
per line; blank lines and `#` comments are ignored. CLI values override config
values. Existing Perl cloc configuration is not loaded automatically.

```text
exclude-dir=target,node_modules
hide-rate
json
```

Example language definition:

```text
Widget
    extension wid
    filter remove_inline !
    filter remove_between_general /* */
    3rd_gen_scale 1.00
```

```sh
cloc-rs --read-lang-def=widget.def project/
cloc-rs --write-lang-def=languages.json
```

## Verify

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 tests/commands.py target/debug/cloc-rs
python3 tests/compatibility.py --reference cloc-2.10.pl --catalog
```

Supply the external cloc 2.10 reference yourself. The comparison suite covers
counts, filters, duplicates, line endings, binary/Unicode input, stdin, lists,
compressed archives, Git revisions, and diffs. The catalog pass tests each usable
reference language with independently constructed basic comment syntax.
Three ambiguous/dispatch-only upstream entries are excluded from forced-language
probes (`Pascal/Pawn`, `Unknown/BitBake`, `XML-Qt-GTK`): cloc cannot count them when
forced. A catalog probe checks basic syntax, not every feature of that language.
GitHub Actions runs Rust tests on Linux, macOS, and Windows and the comparison
suite on Linux.

## License

[MIT](LICENSE). Tokei's MIT notice is in
[licenses/tokei-MIT.txt](licenses/tokei-MIT.txt). Dependencies retain their
permissive licenses. [licenses/README.md](licenses/README.md) records provenance.
cloc remains a separate GPL-licensed project.
