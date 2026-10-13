use crate::{
    cli::Options,
    model::{Counts, DiffCounts, DiffReport, Report},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::Path,
};
pub fn scale(language: &str) -> f64 {
    static VALUES: std::sync::OnceLock<BTreeMap<String, f64>> = std::sync::OnceLock::new();
    VALUES
        .get_or_init(|| {
            serde_json::from_str(include_str!("../data/scales.json"))
                .expect("embedded scale factors")
        })
        .get(language)
        .copied()
        .unwrap_or(1.0)
}
const URL: &str = "github.com/Andromeda1957/cloc-rs";
fn header(elapsed: f64, n_files: u64, n_lines: u64, o: &Options) -> Value {
    let mut v = json!({"cloc_url":URL,"cloc_version":env!("CARGO_PKG_VERSION"),"n_files":n_files,"n_lines":n_lines});
    if !o.flag("hide-rate") {
        v["elapsed_seconds"] = json!(elapsed);
        v["files_per_second"] = json!(n_files as f64 / elapsed.max(1e-9));
        v["lines_per_second"] = json!(n_lines as f64 / elapsed.max(1e-9));
    }
    v
}
fn summary(r: &Report, o: &Options) -> Result<BTreeMap<String, Counts>> {
    let mut langs = r.languages.clone();
    if o.flag("by-file") {
        return Ok(langs);
    };
    if let Some(s) = o.value("summary-cutoff") {
        let (k, n) = s.split_once(':').context("--summary-cutoff requires X:N")?;
        let percent = n.ends_with('%');
        let threshold: f64 = n.trim_end_matches('%').parse()?;
        let measure = |c: &Counts| match k {
            "c" => c.code,
            "f" => c.files,
            "m" => c.comment,
            "cm" => c.code + c.comment,
            _ => 0,
        };
        if !["c", "f", "m", "cm"].contains(&k) {
            bail!("unknown --summary-cutoff column {k}")
        };
        let total = measure(&r.total) as f64;
        let mut other = Counts::default();
        langs.retain(|_, c| {
            if (if percent {
                100.0 * measure(c) as f64 / total.max(1.0)
            } else {
                measure(c) as f64
            }) < threshold
            {
                other.add(*c);
                false
            } else {
                true
            }
        });
        if other.files > 0 {
            langs.insert("Other".into(), other);
        }
    }
    Ok(langs)
}
pub fn count_value(r: &Report, o: &Options) -> Result<Value> {
    let mut v = json!({"header":header(r.elapsed,r.total.files,r.total.lines(),o)});
    if o.flag("by-file") {
        for f in &r.files {
            v[&f.name] = json!({"blank":f.counts.blank,"comment":f.counts.comment,"code":f.counts.code,"language":f.language});
        }
    }
    if !o.flag("by-file") || o.flag("by-file-by-lang") {
        for (n, c) in summary(r, o)? {
            v[&n] = serde_json::to_value(c)?;
        }
    }
    v["SUM"] = serde_json::to_value(r.total)?;
    if o.flag("3") {
        let mut scaled = 0.0;
        for (n, c) in &r.languages {
            let factor = scale(n);
            scaled += c.code as f64 * factor;
            if v.get(n).is_some() {
                v[n]["factor"] = json!(factor);
                v[n]["scaled"] = json!(c.code as f64 * factor);
            }
        }
        if o.flag("by-file") {
            for f in &r.files {
                v[&f.name]["factor"] = json!(scale(&f.language));
                v[&f.name]["scaled"] = json!(f.counts.code as f64 * scale(&f.language));
            }
        }
        v["SUM"]["scaled"] = json!(scaled);
        v["SUM"]["factor"] = json!(if r.total.code == 0 {
            0.0
        } else {
            scaled / r.total.code as f64
        });
    }

    Ok(v)
}
pub fn diff_value(r: &DiffReport, o: &Options) -> Result<Value> {
    let mut v = json!({"header":header(r.elapsed,r.total.added.files+r.total.removed.files+r.total.modified.files+r.total.same.files,r.total.same.lines()+r.total.modified.lines()+r.total.removed.lines(),o)});
    for status in ["added", "same", "modified", "removed"] {
        let mut langs = serde_json::Map::new();
        if o.flag("by-file") {
            for (n, (l, d)) in &r.files {
                let mut val = serde_json::to_value(d)?[status].clone();
                let _ = l;
                val["nFiles"] = json!(0);
                langs.insert(n.clone(), val);
            }
        } else {
            for (n, d) in &r.languages {
                langs.insert(n.clone(), serde_json::to_value(d)?[status].clone());
            }
        }
        v[status] = Value::Object(langs);
    }
    v["SUM"] = serde_json::to_value(&r.total)?;
    if o.flag("by-file") {
        for status in ["added", "same", "modified", "removed"] {
            v["SUM"][status]["nFiles"] = json!(0)
        }
        v["header"]["n_files"] = json!(0);
    }

    Ok(v)
}
fn banner(elapsed: f64, c: Counts, o: &Options) -> String {
    if o.flag("hide-rate") {
        format!("{URL} v {}", env!("CARGO_PKG_VERSION"))
    } else {
        format!(
            "{URL} v {}  T={elapsed:.2} s ({:.1} files/s, {:.1} lines/s)",
            env!("CARGO_PKG_VERSION"),
            c.files as f64 / elapsed.max(1e-9),
            c.lines() as f64 / elapsed.max(1e-9)
        )
    }
}
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn csv_cell(s: &str, delim: &str) -> String {
    if s.contains(delim) || s.contains(['"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}
fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn format_number(n: u64, o: &Options) -> String {
    let mut s = n.to_string();
    if o.has("fmt") {
        if let Some(d) = o.value("thousands-delimiter") {
            let mut groups = Vec::new();
            while s.len() > 3 {
                let t = s.split_off(s.len() - 3);
                groups.push(t)
            }
            groups.push(s);
            groups.reverse();
            return groups.join(d);
        }
    };
    s
}
fn num_values(c: Counts, total: Counts, o: &Options) -> Vec<String> {
    if let Some(method) = o.value("by-percent") {
        let den = match method {
            "c" => c.code,
            "cm" => c.code + c.comment,
            "cb" => c.code + c.blank,
            "cmb" => c.lines(),
            _ => 0,
        };
        let ratio = |n: u64, d: u64| {
            format!(
                "{:.2}",
                if d == 0 {
                    0.0
                } else {
                    100.0 * n as f64 / d as f64
                }
            )
        };
        if method == "t" {
            vec![
                ratio(c.blank, total.blank),
                ratio(c.comment, total.comment),
                ratio(c.code, total.code),
            ]
        } else {
            vec![
                ratio(c.blank, den),
                ratio(c.comment, den),
                format_number(c.code, o),
            ]
        }
    } else {
        vec![
            format_number(c.blank, o),
            format_number(c.comment, o),
            format_number(c.code, o),
        ]
    }
}
pub fn render_count(r: &Report, o: &Options) -> Result<String> {
    let value = count_value(r, o)?;
    if o.flag("json") {
        return Ok(serde_json::to_string_pretty(&value)? + "\n");
    }
    if o.flag("yaml") {
        return Ok(format!("---\n{}", serde_yaml::to_string(&value)?));
    }
    if o.has("sql") {
        return render_sql(r, o);
    }
    let mut rows: Vec<(String, String, Counts)> = if o.flag("by-file") {
        r.files
            .iter()
            .map(|f| (f.name.clone(), f.language.clone(), f.counts))
            .collect()
    } else {
        summary(r, o)?
            .into_iter()
            .map(|(n, c)| (n, String::new(), c))
            .collect()
    };
    rows.sort_by(|a, b| b.2.code.cmp(&a.2.code).then(a.0.cmp(&b.0)));
    if o.flag("by-file-by-lang") {
        rows.extend(
            r.languages
                .iter()
                .map(|(n, c)| (n.clone(), String::new(), *c)),
        );
    }
    if o.flag("csv") {
        let d = o.value("csv-delimiter").unwrap_or(",");
        if d.chars().count() != 1 {
            bail!("--csv-delimiter requires one character")
        }
        let extra = if o.flag("3") {
            format!("scale{d}3rd gen. equiv{d}")
        } else {
            String::new()
        };
        let mut out = if o.flag("by-file") {
            format!(
                "language{d}filename{d}blank{d}comment{d}code{d}{extra}\"{}\"\n",
                banner(r.elapsed, r.total, o)
            )
        } else {
            format!(
                "files{d}language{d}blank{d}comment{d}code{d}{extra}\"{}\"\n",
                banner(r.elapsed, r.total, o)
            )
        };
        for (n, l, c) in &rows {
            let mut cells = if o.flag("by-file") {
                vec![
                    csv_cell(l, d),
                    csv_cell(n, d),
                    c.blank.to_string(),
                    c.comment.to_string(),
                    c.code.to_string(),
                ]
            } else {
                vec![
                    c.files.to_string(),
                    csv_cell(n, d),
                    c.blank.to_string(),
                    c.comment.to_string(),
                    c.code.to_string(),
                ]
            };
            if o.flag("3") {
                let factor = scale(if l.is_empty() { n } else { l });
                cells.extend([factor.to_string(), (c.code as f64 * factor).to_string()]);
            }
            out += &cells.join(d);
            out += "\n";
        }
        out += &[
            r.total.files.to_string(),
            "SUM".into(),
            r.total.blank.to_string(),
            r.total.comment.to_string(),
            r.total.code.to_string(),
        ]
        .join(d);
        if o.flag("3") {
            out += &format!("{d}{}{d}{}", value["SUM"]["factor"], value["SUM"]["scaled"]);
        }
        out += "\n";
        return Ok(out);
    }
    if o.flag("xml") {
        let mut out = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_owned();
        if let Some(x) = o.value("xsl") {
            let p = if x == "1" { "cloc.xsl" } else { x };
            out += &format!(
                "<?xml-stylesheet type=\"text/xsl\" href=\"{}\"?>\n",
                escape_xml(p)
            );
        }
        out += "<results>\n<header>\n";
        if let Some(h) = value["header"].as_object() {
            for (k, v) in h {
                out += &format!(
                    "<{k}>{}</{k}>\n",
                    escape_xml(
                        &v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    )
                );
            }
        }
        out += "</header>\n";
        let group = if o.flag("by-file") {
            "files"
        } else {
            "languages"
        };
        let item = if o.flag("by-file") {
            "file"
        } else {
            "language"
        };
        out += &format!("<{group}>\n");
        for (n, l, c) in &rows {
            out += &format!(
                "<{item} name=\"{}\" blank=\"{}\" comment=\"{}\" code=\"{}\"{} />\n",
                escape_xml(n),
                c.blank,
                c.comment,
                c.code,
                if l.is_empty() {
                    format!(" nFiles=\"{}\"", c.files)
                } else {
                    format!(" language=\"{}\"", escape_xml(l))
                }
            );
        }
        out += &format!(
            "<total sum_files=\"{}\" blank=\"{}\" comment=\"{}\" code=\"{}\" />\n</{group}>\n</results>\n",
            r.total.files, r.total.blank, r.total.comment, r.total.code
        );
        return Ok(out);
    }
    if o.flag("md") {
        let mut out = format!(
            "{}\n\nLanguage{} | files | blank | comment | code\n:--- | ---: | ---: | ---: | ---:\n",
            banner(r.elapsed, r.total, o),
            if o.flag("by-file") { " / File" } else { "" }
        );
        for (n, _, c) in &rows {
            out += &format!(
                "{} | {} | {} | {} | {}\n",
                n.replace('|', "\\|"),
                c.files,
                c.blank,
                c.comment,
                c.code
            )
        }
        out += &format!(
            "SUM | {} | {} | {} | {}\n",
            r.total.files, r.total.blank, r.total.comment, r.total.code
        );
        return Ok(out);
    }
    let fmt = o.value("fmt").unwrap_or("1");
    if o.flag("3") {
        let mut plain = o.clone();
        plain.flags.remove("3");
        let rendered = render_count(r, &plain)?;
        let mut output = String::new();
        let mut index = 0;
        for line in rendered.lines() {
            if line.starts_with("Language") || line.starts_with("File") {
                output += &format!("{line}{:>12}{:>18}\n", "scale", "3rd gen. equiv")
            } else if line.starts_with("SUM:") {
                output += &format!(
                    "{line}{:>12}{:>18}\n",
                    value["SUM"]["factor"].to_string(),
                    value["SUM"]["scaled"].to_string()
                )
            } else if !line.starts_with('-') && !line.starts_with(URL) && index < rows.len() {
                let (n, l, c) = &rows[index];
                let factor = scale(if l.is_empty() { n } else { l });
                output += &format!("{line}{factor:>12.2}{:>18.2}\n", factor * c.code as f64);
                index += 1
            } else {
                output += line;
                output += "\n"
            }
        }
        return Ok(output);
    }

    let show_lang = ["3", "5"].contains(&fmt);
    let show_total = ["2", "4", "5"].contains(&fmt);
    let width = rows
        .iter()
        .map(|r| r.0.len())
        .max()
        .unwrap_or(8)
        .max(if o.flag("by-file") { 8 } else { 20 });
    let columns = if o.flag("by-file") { 0 } else { 12 };
    let line = "-".repeat(
        width + columns + 45 + if show_lang { 24 } else { 0 } + if show_total { 15 } else { 0 },
    );
    let mut out = format!(
        "{}\n{line}\n{:width$}{}{:>15}{:>15}{:>15}{}{}\n{line}\n",
        banner(r.elapsed, r.total, o),
        if o.flag("by-file") {
            "File"
        } else {
            "Language"
        },
        if o.flag("by-file") {
            String::new()
        } else {
            format!("{:>12}", "files")
        },
        "blank",
        "comment",
        "code",
        if show_total {
            format!("{:>15}", "total")
        } else {
            String::new()
        },
        if show_lang {
            format!("{:>24}", "language")
        } else {
            String::new()
        }
    );
    for (n, l, c) in &rows {
        let nums = num_values(*c, r.total, o);
        out += &format!(
            "{n:width$}{}{:>15}{:>15}{:>15}{}{}\n",
            if o.flag("by-file") {
                String::new()
            } else {
                format!("{:>12}", format_number(c.files, o))
            },
            nums[0],
            nums[1],
            nums[2],
            if show_total {
                format!("{:>15}", format_number(c.lines(), o))
            } else {
                String::new()
            },
            if show_lang {
                format!("{l:>24}")
            } else {
                String::new()
            }
        );
    }
    if rows.len() > 1 || o.flag("sum-one") {
        let n = num_values(r.total, r.total, o);
        out += &format!(
            "{line}\n{:width$}{}{:>15}{:>15}{:>15}{}\n",
            "SUM:",
            if o.flag("by-file") {
                String::new()
            } else {
                format!("{:>12}", format_number(r.total.files, o))
            },
            n[0],
            n[1],
            n[2],
            if show_total {
                format!("{:>15}", format_number(r.total.lines(), o))
            } else {
                String::new()
            }
        );
    };
    out += &format!("{line}\n");
    Ok(out)
}
fn render_sql(r: &Report, o: &Options) -> Result<String> {
    let project = o
        .value("sql-project")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let project = sql_quote(&project);
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();
    let mut out = if o.flag("sql-append") {
        String::new()
    } else {
        "CREATE TABLE metadata (id INTEGER PRIMARY KEY, timestamp VARCHAR(500), Project VARCHAR(500), elapsed_s REAL);\nCREATE TABLE t (id INTEGER, Project VARCHAR(500), Language VARCHAR(500), File VARCHAR(500), File_dirname VARCHAR(500), File_basename VARCHAR(500), nBlank INTEGER, nComment INTEGER, nCode INTEGER, nScaled REAL, FOREIGN KEY (id) REFERENCES metadata(id));\n".into()
    };
    if o.value("sql-style").is_some_and(|s| {
        !s.eq_ignore_ascii_case("sqlite")
            && !s.eq_ignore_ascii_case("Named_Columns")
            && !s.eq_ignore_ascii_case("Oracle")
    }) {
        bail!("--sql-style must be SQLite, Oracle, or Named_Columns")
    }
    out += &format!(
        "BEGIN TRANSACTION;\nINSERT INTO metadata VALUES ({id}, '{id}', {project}, {});\n",
        r.elapsed
    );
    for f in &r.files {
        let p = Path::new(&f.name);
        let columns = if o.value("sql-style") == Some("Named_Columns") {
            " (id, Project, Language, File, File_dirname, File_basename, nBlank, nComment, nCode, nScaled)"
        } else {
            ""
        };
        out += &format!(
            "INSERT INTO t{columns} VALUES ({id}, {project}, {}, {}, {}, {}, {}, {}, {}, {});\n",
            sql_quote(&f.language),
            sql_quote(&f.name),
            sql_quote(&p.parent().unwrap_or(Path::new("")).to_string_lossy()),
            sql_quote(&p.file_name().unwrap_or_default().to_string_lossy()),
            f.counts.blank,
            f.counts.comment,
            f.counts.code,
            f.counts.code as f64 * scale(&f.language)
        );
    }
    out += "COMMIT;\n";
    Ok(out)
}
pub fn render_diff(r: &DiffReport, o: &Options) -> Result<String> {
    let v = diff_value(r, o)?;
    if o.flag("json") {
        return Ok(serde_json::to_string_pretty(&v)? + "\n");
    };
    if o.flag("yaml") {
        return Ok(format!("---\n{}", serde_yaml::to_string(&v)?));
    }
    let mut rows: Vec<_> = if o.flag("by-file") {
        r.files.iter().map(|(n, (_, d))| (n.as_str(), d)).collect()
    } else {
        r.languages.iter().map(|(n, d)| (n.as_str(), d)).collect()
    };
    rows.push(("SUM", &r.total));
    if o.flag("xml") {
        let mut out =
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><diff_results>\n<header>\n".to_owned();
        for (k, v) in v["header"].as_object().unwrap() {
            out += &format!(
                "<{k}>{}</{k}>\n",
                escape_xml(
                    &v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                )
            )
        }
        out += "</header>\n";
        for status in ["same", "modified", "added", "removed"] {
            out += &format!("<{status}>\n");
            for (n, d) in &rows {
                let c = serde_json::to_value(d)?[status].clone();
                if *n == "SUM" {
                    out += &format!(
                        "<total sum_files=\"{}\" blank=\"{}\" comment=\"{}\" code=\"{}\" />\n",
                        c["nFiles"], c["blank"], c["comment"], c["code"]
                    )
                } else {
                    out += &format!(
                        "<language name=\"{}\" files_count=\"{}\" blank=\"{}\" comment=\"{}\" code=\"{}\" />\n",
                        escape_xml(n),
                        c["nFiles"],
                        c["blank"],
                        c["comment"],
                        c["code"]
                    )
                }
            }
            out += &format!("</{status}>\n")
        }
        out += "</diff_results>\n";
        return Ok(out);
    }
    if o.flag("csv") {
        let dl = o.value("csv-delimiter").unwrap_or(",");
        let mut cells = vec!["Language".to_owned()];
        for field in ["files", "blank", "comment", "code"] {
            for status in ["==", "!=", "+", "-"] {
                cells.push(format!("{status} {field}"))
            }
        }
        let mut out = cells.join(dl) + "\n";
        for (n, d) in &rows {
            if *n == "SUM" {
                continue;
            };
            let mut cells = vec![csv_cell(n, dl)];
            for field in ["nFiles", "blank", "comment", "code"] {
                for (_, c) in diff_rows(d) {
                    cells.push(serde_json::to_value(c)?[field].to_string())
                }
            }
            out += &cells.join(dl);
            out += "\n"
        }
        return Ok(out);
    }
    let mut out = if o.flag("csv") {
        format!(
            "{}\n",
            ["language", "status", "files", "blank", "comment", "code"]
                .join(o.value("csv-delimiter").unwrap_or(","))
        )
    } else if o.flag("md") {
        "Language | status | files | blank | comment | code\n:--- | :--- | ---: | ---: | ---: | ---:\n".into()
    } else {
        format!(
            "{URL} v {}\n{:<25}{:<12}{:>10}{:>12}{:>12}{:>12}\n",
            env!("CARGO_PKG_VERSION"),
            "Language",
            "status",
            "files",
            "blank",
            "comment",
            "code"
        )
    };
    for (n, d) in rows {
        for (s, c) in diff_rows(d) {
            if o.flag("csv") {
                let dl = o.value("csv-delimiter").unwrap_or(",");
                out += &[
                    csv_cell(n, dl),
                    s.into(),
                    c.files.to_string(),
                    c.blank.to_string(),
                    c.comment.to_string(),
                    c.code.to_string(),
                ]
                .join(dl);
                out += "\n";
            } else if o.flag("md") {
                out += &format!(
                    "{} | {s} | {} | {} | {} | {}\n",
                    n.replace('|', "\\|"),
                    c.files,
                    c.blank,
                    c.comment,
                    c.code
                )
            } else {
                out += &format!(
                    "{n:<25}{s:<12}{:>10}{:>12}{:>12}{:>12}\n",
                    c.files, c.blank, c.comment, c.code
                )
            }
        }
    }
    Ok(out)
}
fn diff_rows(d: &DiffCounts) -> [(&str, Counts); 4] {
    [
        ("same", d.same),
        ("modified", d.modified),
        ("added", d.added),
        ("removed", d.removed),
    ]
}
pub fn emit(text: &str, o: &Options) -> Result<()> {
    if o.value("xsl") == Some("1") {
        fs::write(
            "cloc.xsl",
            r#"<?xml version="1.0"?><xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><html><body><pre><xsl:copy-of select="*"/></pre></body></html></xsl:template></xsl:stylesheet>"#,
        )?;
    }
    let path = o.value("sql").or_else(|| o.value("report-file"));
    if let Some(p) = path.filter(|p| *p != "-") {
        let bytes = if let Some(e) = o.value("file-encoding") {
            if e.eq_ignore_ascii_case("UTF-16") || e.eq_ignore_ascii_case("UTF-16LE") {
                let mut b = vec![0xff, 0xfe];
                b.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
                b
            } else if e.eq_ignore_ascii_case("UTF-16BE") {
                let mut b = vec![0xfe, 0xff];
                b.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
                b
            } else {
                let enc = encoding_rs::Encoding::for_label(e.as_bytes())
                    .with_context(|| format!("unknown output encoding {e}"))?;
                let (data, _, errors) = enc.encode(text);
                if errors {
                    bail!("report cannot be represented in {e}")
                };
                data.into_owned()
            }
        } else {
            text.as_bytes().to_vec()
        };
        if o.flag("sql-append") {
            use std::fs::OpenOptions;
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(p)?
                .write_all(&bytes)?;
        } else {
            fs::write(p, bytes)?
        };
    } else {
        io::stdout().lock().write_all(text.as_bytes())?
    };
    Ok(())
}
pub fn sum_reports(paths: &[String]) -> Result<Report> {
    let mut r = Report::default();
    let pattern = regex::Regex::new(r"^\s*(.+?)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s*$")?;
    for p in paths {
        let text = fs::read_to_string(p)?;
        let v = serde_json::from_str::<Value>(&text)
            .ok()
            .or_else(|| serde_yaml::from_str::<Value>(&text).ok());
        if let Some(Value::Object(v)) = v {
            for (n, c) in v {
                if n == "SUM" || n == "header" {
                    continue;
                };
                if let Ok(c) = serde_json::from_value::<Counts>(c) {
                    r.languages.entry(n).or_default().add(c);
                }
            }
        } else {
            for line in text.lines() {
                if let Some(c) = pattern.captures(line) {
                    let n = c[1].trim();
                    if n == "SUM:" || n == "SUM" {
                        continue;
                    };
                    r.languages.entry(n.into()).or_default().add(Counts {
                        files: c[2].parse()?,
                        blank: c[3].parse()?,
                        comment: c[4].parse()?,
                        code: c[5].parse()?,
                    });
                }
            }
        }
    }
    for c in r.languages.values() {
        r.total.add(*c)
    }
    Ok(r)
}
