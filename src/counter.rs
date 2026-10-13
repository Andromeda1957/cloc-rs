use crate::{
    cli::Options,
    languages::Language,
    model::{Counts, FileCount, Kind, Source},
};
use anyhow::{Context, Result};
use fancy_regex::Regex;

#[derive(Debug)]
pub struct TimedOut(pub &'static str);
impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} time limit exceeded", self.0)
    }
}
impl std::error::Error for TimedOut {}
#[derive(Debug, Clone)]
enum State {
    Normal,
    Block {
        start: String,
        end: String,
        depth: usize,
    },
    Quote {
        end: String,
        verbatim: bool,
    },
}
fn regex_any(regexes: &[Regex], text: &str) -> Result<bool> {
    for regex in regexes {
        if regex.is_match(text)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn starts(text: &str, pos: usize, needle: &str) -> bool {
    text[pos..].starts_with(needle)
}
fn raw_quote(text: &str, pos: usize, lang: &str) -> Option<(usize, String)> {
    if lang == "Rust" {
        let s = &text[pos..];
        let skip = if s.starts_with("br") {
            2
        } else if s.starts_with('r') {
            1
        } else {
            return None;
        };
        let n = s[skip..].bytes().take_while(|b| *b == b'#').count();
        if s.as_bytes().get(skip + n) == Some(&b'"') {
            return Some((skip + n + 1, format!("\"{}", "#".repeat(n))));
        }
    }
    if ["C++", "C/C++ Header", "Objective-C++"].contains(&lang) && text[pos..].starts_with("R\"") {
        if let Some(n) = text[pos + 2..].find('(').filter(|n| *n <= 16) {
            let d = &text[pos + 2..pos + 2 + n];
            return Some((n + 3, format!("){d}\"")));
        }
    }
    None
}
fn fixed_comment(line: &str, name: &str) -> bool {
    match name {
        "Fortran 77" => line.starts_with(['c', 'C', '*', '!']),
        "COBOL" => line
            .as_bytes()
            .get(6)
            .is_some_and(|b| *b == b'*' || *b == b'/'),
        "ABAP" => line.starts_with('*'),
        "DOS Batch" => {
            let s = line.trim_start().trim_start_matches('@').to_lowercase();
            s == "rem" || s.starts_with("rem ") || s.starts_with("::")
        }
        _ => false,
    }
}
pub fn decode(bytes: &[u8], unicode: bool) -> Option<String> {
    if unicode && (bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]))
    {
        let le = bytes[0] == 0xff;
        if (bytes.len() - 4) % 4 != 0 {
            return None;
        };
        return bytes[4..]
            .chunks_exact(4)
            .map(|b| {
                char::from_u32(if le {
                    u32::from_le_bytes(b.try_into().unwrap())
                } else {
                    u32::from_be_bytes(b.try_into().unwrap())
                })
            })
            .collect();
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if unicode {
        let (data, encoding) = if bytes.starts_with(&[0xff, 0xfe]) {
            (&bytes[2..], Some(encoding_rs::UTF_16LE))
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            (&bytes[2..], Some(encoding_rs::UTF_16BE))
        } else if bytes.len() >= 4 && bytes[1] == 0 && bytes[3] == 0 {
            (bytes, Some(encoding_rs::UTF_16LE))
        } else if bytes.len() >= 4 && bytes[0] == 0 && bytes[2] == 0 {
            (bytes, Some(encoding_rs::UTF_16BE))
        } else {
            (bytes, None)
        };
        if let Some(e) = encoding {
            let (s, _, errors) = e.decode(data);
            return (!errors).then(|| s.into_owned());
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}
pub fn count(source: &Source, text: &str, language: &Language, o: &Options) -> Result<FileCount> {
    let skip = o
        .value("skip-leading")
        .map(|v| -> Result<usize> {
            let (n, ext) = v.split_once(',').unwrap_or((v, ""));
            if !ext.is_empty()
                && std::path::Path::new(&source.name)
                    .extension()
                    .and_then(|s| s.to_str())
                    != Some(ext)
            {
                return Ok(0);
            };
            Ok(n.parse()?)
        })
        .transpose()?
        .unwrap_or(0);
    let lines: Vec<String> = text
        .lines()
        .skip(skip)
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect();
    let mut state = State::Normal;
    let mut kinds = Vec::with_capacity(lines.len());
    let mut stripped = Vec::with_capacity(lines.len());
    let mut counts = Counts {
        files: 1,
        ..Default::default()
    };
    let mut blocks = language.block.clone();
    let mut quotes = language.quotes.clone();
    for pair in &language.doc {
        if o.flag("docstring-as-code") {
            quotes.push(pair.clone())
        } else {
            blocks.push(pair.clone())
        }
    }
    if language.name == "Python" {
        blocks.retain(|(s, _)| !s.starts_with("=pod"));
    }
    if language.name == "PHP" {
        blocks.push(("<!--".into(), "-->".into()));
    }
    let mut inline = language.line.clone();
    if language.name == "Fortran 77" {
        inline = vec!["!".into()];
    }
    if language.name == "ABAP" {
        inline = vec![];
    }
    if language.name == "DOS Batch" {
        inline.clear();
    }
    blocks.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    quotes.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    let mut ignores = Vec::new();
    for v in o.all("ignore-regex") {
        let (names, pattern) = v
            .split_once('|')
            .context("--ignore-regex requires LANG,...|REGEX")?;
        if names == "*"
            || names
                .split(',')
                .any(|n| n.eq_ignore_ascii_case(&language.name))
        {
            ignores.push(
                Regex::new(pattern).with_context(|| format!("invalid --ignore-regex {pattern}"))?,
            );
        }
    }
    let remove: Vec<Regex> = language
        .filters
        .iter()
        .filter(|(f, _)| f == "remove_matches")
        .map(|(_, p)| Regex::new(p).map_err(anyhow::Error::from))
        .collect::<Result<_>>()?;
    let mut embedded = false;
    let mut fenced = false;
    let mut notebook_code = false;
    let mut notebook_cell_code = false;
    let budget = o.number("timeout", 0.0)?;
    let deadline = if budget > 0.0 {
        Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(budget))
    } else {
        None
    };
    for line in &lines {
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(TimedOut("counting").into());
        }

        if language.name == "Rmd"
            || language.name.starts_with("Literate ")
            || language.name == "Jupyter Notebook"
        {
            let trim = line.trim();
            let kind = if trim.is_empty() {
                Kind::Blank
            } else if language.name.starts_with("Literate ") {
                if trim.starts_with('>') {
                    Kind::Code
                } else {
                    Kind::Comment
                }
            } else if language.name == "Rmd" {
                if trim.starts_with("```") {
                    fenced = !fenced;
                    Kind::Comment
                } else if fenced && !trim.starts_with('#') {
                    Kind::Code
                } else {
                    Kind::Comment
                }
            } else {
                if trim.contains("\"cell_type\"") {
                    notebook_code = false;
                    notebook_cell_code = trim.contains("code");
                }
                let value = trim.trim_end_matches(',');
                if let Ok(value) = serde_json::from_str::<String>(value) {
                    if notebook_cell_code
                        && !value.trim().is_empty()
                        && !value.trim_start().starts_with('#')
                        && !value.starts_with("# ")
                    {
                        notebook_code = true;
                        Kind::Code
                    } else {
                        Kind::Comment
                    }
                } else if notebook_code {
                    Kind::Code
                } else {
                    Kind::Comment
                }
            };
            counts.line(kind);
            kinds.push(kind);
            stripped.push(if kind == Kind::Code {
                line.clone()
            } else {
                String::new()
            });
            continue;
        }
        let mut clean = String::new();
        let mut code = false;
        let mut comment = matches!(state, State::Block { .. });
        let mut i = 0;
        if matches!(state, State::Normal) && fixed_comment(line, &language.name) {
            comment = true;
            i = line.len();
        }
        if matches!(state, State::Normal)
            && language.name == "Perl"
            && line.starts_with('=')
            && line.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            && !line.starts_with("=cut")
        {
            state = State::Block {
                start: "=pod".into(),
                end: "=cut".into(),
                depth: 1,
            };
            comment = true;
            i = line.len();
        }
        if matches!(state, State::Normal) && regex_any(&remove, line)? {
            comment = true;
            i = line.len();
        }
        if ["HTML", "Vuejs Component", "Svelte"].contains(&language.name.as_str())
            && line.to_lowercase().contains("<script")
        {
            embedded = true
        }
        while i < line.len() {
            let ch = line[i..].chars().next().unwrap();
            match state.clone() {
                State::Block { start, end, depth } => {
                    comment = true;
                    if starts(line, i, &end) {
                        i += end.len();
                        if depth == 1 {
                            state = State::Normal
                        } else {
                            state = State::Block {
                                start,
                                end,
                                depth: depth - 1,
                            }
                        }
                    } else if (language.nested && language.name != "Rust")
                        && start != end
                        && starts(line, i, &start)
                    {
                        i += start.len();
                        state = State::Block {
                            start,
                            end,
                            depth: depth + 1,
                        }
                    } else {
                        i += ch.len_utf8()
                    }
                }
                State::Quote { end, verbatim } => {
                    code = true;
                    if starts(line, i, &end) {
                        clean.push_str(&end);
                        i += end.len();
                        if verbatim && starts(line, i, &end) {
                            clean.push_str(&end);
                            i += end.len()
                        } else {
                            state = State::Normal
                        }
                    } else if ch == '\\' && !verbatim {
                        clean.push(ch);
                        i += 1;
                        if i < line.len() {
                            let c = line[i..].chars().next().unwrap();
                            clean.push(c);
                            i += c.len_utf8();
                        }
                    } else {
                        clean.push(ch);
                        i += ch.len_utf8();
                    }
                }
                State::Normal => {
                    if let Some((n, end)) = raw_quote(line, i, &language.name) {
                        code = true;
                        clean.push_str(&line[i..i + n]);
                        i += n;
                        state = State::Quote {
                            end,
                            verbatim: true,
                        };
                        continue;
                    }
                    if let Some((start, end)) = blocks.iter().find(|(s, _)| starts(line, i, s)) {
                        comment = true;
                        i += start.len();
                        state = State::Block {
                            start: start.clone(),
                            end: end.clone(),
                            depth: 1,
                        };
                        continue;
                    }
                    if embedded && starts(line, i, "/*") {
                        comment = true;
                        i += 2;
                        state = State::Block {
                            start: "/*".into(),
                            end: "*/".into(),
                            depth: 1,
                        };
                        continue;
                    }
                    if inline.iter().any(|s| starts(line, i, s))
                        || (embedded && starts(line, i, "//"))
                    {
                        comment = true;
                        break;
                    }
                    if language.name == "Rust" && ch == '\'' {
                        let tail = &line[i + 1..];
                        if let Some(n) = tail.find('\'').filter(|n| *n <= 8) {
                            let len = n + 2;
                            code = true;
                            clean.push_str(&line[i..i + len]);
                            i += len;
                            continue;
                        }
                    }
                    if let Some((s, e, verbatim)) = language
                        .verbatim
                        .iter()
                        .map(|(a, b)| (a, b, true))
                        .chain(quotes.iter().map(|(a, b)| (a, b, false)))
                        .find(|(s, _, _)| starts(line, i, s))
                    {
                        code = true;
                        clean.push_str(s);
                        i += s.len();
                        state = State::Quote {
                            end: e.clone(),
                            verbatim,
                        };
                        continue;
                    }
                    if !ch.is_whitespace() {
                        code = true
                    };
                    clean.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        if embedded && line.to_lowercase().contains("</script") {
            embedded = false
        }
        let kind = if line.starts_with("#!") {
            Kind::Code
        } else if line.trim().is_empty() {
            Kind::Blank
        } else if code {
            Kind::Code
        } else if comment {
            Kind::Comment
        } else {
            Kind::Code
        };
        let kind = if kind == Kind::Code && regex_any(&ignores, &clean)? {
            Kind::Ignored
        } else {
            kind
        };
        counts.line(kind);
        kinds.push(kind);
        stripped.push(clean);
        // Ordinary C-family quotes cannot continue past a physical newline unless escaped.
        if let State::Quote {
            ref end,
            verbatim: false,
        } = state
        {
            if end.len() == 1 && end != "`" && language.name != "Python" && !line.ends_with('\\') {
                state = State::Normal
            }
        }
    }
    if o.flag("only-count-files") {
        counts.blank = 0;
        counts.comment = 0;
        counts.code = 0;
    }
    Ok(FileCount {
        name: source.name.clone(),
        key: source.key.clone(),
        language: language.name.clone(),
        counts,
        lines,
        kinds,
        stripped,
    })
}
