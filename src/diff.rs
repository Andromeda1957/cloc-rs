use crate::{
    cli::Options,
    model::{DiffCounts, DiffReport, FileCount, Kind, Report},
};
use anyhow::{Result, bail};
use similar::{Algorithm, DiffOp};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    time::{Duration, Instant},
};
pub fn alignment_pairs(text: &str) -> Result<Vec<(Option<String>, Option<String>)>> {
    let mut result = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        if line.contains('\t') {
            let v: Vec<_> = line.split('\t').collect();
            if v.len() < 3 {
                bail!("invalid alignment: {line}")
            };
            result.push((
                if v[1] == "-" { None } else { Some(v[1].into()) },
                if v[2] == "-" { None } else { Some(v[2].into()) },
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("+ ") {
            let path = rest.rsplit_once(" ; ").map(|v| v.0).unwrap_or(rest);
            result.push((None, Some(path.trim().into())));
        } else if let Some(rest) = line.strip_prefix("- ") {
            let path = rest.rsplit_once(" ; ").map(|v| v.0).unwrap_or(rest);
            result.push((Some(path.trim().into()), None));
        } else if let Some(rest) = line
            .strip_prefix("== ")
            .or_else(|| line.strip_prefix("!= "))
        {
            let paths = rest.rsplit_once(" ; ").map(|v| v.0).unwrap_or(rest);
            let (a, b) = paths
                .split_once(" | ")
                .ok_or_else(|| anyhow::anyhow!("invalid alignment: {line}"))?;
            result.push((Some(a.trim().into()), Some(b.trim().into())));
        } else if !line.starts_with("Files added:")
            && !line.starts_with("Files removed:")
            && !line.starts_with("File pairs compared:")
        {
            bail!("unknown alignment row: {line}")
        }
    }
    Ok(result)
}
fn normalized(s: &str, o: &Options) -> String {
    let s = if o.flag("ignore-whitespace") {
        s.chars().filter(|c| *c != ' ' && *c != '\t').collect()
    } else {
        s.to_owned()
    };
    if o.flag("ignore-case") {
        s.to_lowercase()
    } else {
        s
    }
}
fn git_renames(o: &Options) -> BTreeMap<String, String> {
    if o.inputs.len() != 2 || o.inputs.iter().any(|p| std::path::Path::new(p).exists()) {
        return BTreeMap::new();
    }
    let resolve = |revision: &str| {
        std::process::Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ])
            .output()
            .ok()
            .filter(|r| r.status.success())
            .and_then(|r| String::from_utf8(r.stdout).ok())
            .map(|s| s.trim().to_owned())
    };
    let (Some(a), Some(b)) = (resolve(&o.inputs[0]), resolve(&o.inputs[1])) else {
        return BTreeMap::new();
    };
    let Ok(out) = std::process::Command::new("git")
        .args(["diff", "--name-status", "-z", "-M", &a, &b])
        .output()
    else {
        return BTreeMap::new();
    };
    let mut renames = BTreeMap::new();
    if out.status.success() {
        let fields: Vec<_> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .collect();
        let mut i = 0;
        while i < fields.len() {
            let status = fields[i];
            i += 1;
            if status.starts_with(b"R") && i + 1 < fields.len() {
                renames.insert(
                    String::from_utf8_lossy(fields[i]).into_owned(),
                    String::from_utf8_lossy(fields[i + 1]).into_owned(),
                );
                i += 2;
            } else {
                i += 1;
            }
        }
    }
    renames
}
fn compare(a: &FileCount, b: &FileCount, o: &Options) -> Result<DiffCounts> {
    let mut d = DiffCounts::default();
    for kind in [Kind::Comment, Kind::Code] {
        let get = |f: &FileCount| {
            f.lines
                .iter()
                .zip(&f.kinds)
                .zip(&f.stripped)
                .filter(|((_, k), _)| **k == kind)
                .map(|((line, _), _stripped)| normalized(line, o))
                .collect::<Vec<_>>()
        };
        let old = get(a);
        let new = get(b);
        let timeout = o.number("diff-timeout", 10.0)?;
        let deadline = if timeout > 0.0 {
            Some(Instant::now() + Duration::from_secs_f64(timeout))
        } else {
            None
        };
        let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, &old, &new, deadline);
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(crate::counter::TimedOut("diff").into());
        }
        for op in ops {
            match op {
                DiffOp::Equal { len, .. } => {
                    for _ in 0..len {
                        d.same.line(kind)
                    }
                }
                DiffOp::Delete { old_len, .. } => {
                    for _ in 0..old_len {
                        d.removed.line(kind)
                    }
                }
                DiffOp::Insert { new_len, .. } => {
                    for _ in 0..new_len {
                        d.added.line(kind)
                    }
                }
                DiffOp::Replace {
                    old_len, new_len, ..
                } => {
                    for _ in 0..old_len.min(new_len) {
                        d.modified.line(kind)
                    }
                    for _ in new_len..old_len {
                        d.removed.line(kind)
                    }
                    for _ in old_len..new_len {
                        d.added.line(kind)
                    }
                }
            }
        }
    }
    if a.lines == b.lines {
        d.same.blank = a.counts.blank;
        d.same.files = 1
    } else {
        d.modified.files = 1;
        d.added.blank = b.counts.blank.saturating_sub(a.counts.blank);
        d.removed.blank = a.counts.blank.saturating_sub(b.counts.blank);
    };
    Ok(d)
}
fn find<'a>(r: &'a Report, n: &str) -> Option<&'a FileCount> {
    r.files.iter().find(|f| f.name == n || f.key == n)
}
fn whole(f: &FileCount, added: bool) -> DiffCounts {
    let mut d = DiffCounts::default();
    let c = f.counts;

    if added {
        d.added = c
    } else {
        d.removed = c
    };
    d
}
pub fn compare_reports(a: &Report, b: &Report, o: &Options) -> Result<DiffReport> {
    let start = Instant::now();
    let mut r = DiffReport::default();
    let old: BTreeMap<_, _> = a.files.iter().map(|f| (f.key.clone(), f)).collect();
    let new: BTreeMap<_, _> = b.files.iter().map(|f| (f.key.clone(), f)).collect();
    let mut pairs = Vec::new();
    let mut used_old = BTreeSet::new();
    let mut used_new = BTreeSet::new();
    if o.has("diff-list-file") {
        let s = fs::read_to_string(o.value("diff-list-file").unwrap())?;
        for (l, n) in alignment_pairs(&s)? {
            let left = l.as_deref().and_then(|n| find(a, n));
            let right = n.as_deref().and_then(|n| find(b, n));
            if (l.is_some() && left.is_none()) || (n.is_some() && right.is_none()) {
                bail!("alignment references missing or filtered input")
            };
            pairs.push((left, right));
        }
    } else {
        for (key, left) in &old {
            if let Some(right) = new.get(key) {
                pairs.push((Some(*left), Some(*right)));
                used_old.insert(key.clone());
                used_new.insert(key.clone());
            }
        }
        for (left, right) in git_renames(o) {
            if !used_old.contains(&left) && !used_new.contains(&right) {
                if let (Some(l), Some(n)) = (old.get(&left), new.get(&right)) {
                    pairs.push((Some(*l), Some(*n)));
                    used_old.insert(left);
                    used_new.insert(right);
                }
            }
        }
        // A direct comparison of two files always pairs them even when renamed.
        if old.len() == 1 && new.len() == 1 && pairs.is_empty() {
            pairs.push((old.values().next().copied(), new.values().next().copied()));
            used_old.extend(old.keys().cloned());
            used_new.extend(new.keys().cloned());
        }
        // Exact renamed files can be aligned without touching the working tree.
        for (key, left) in &old {
            if used_old.contains(key) {
                continue;
            };
            if let Some((rk, right)) = new.iter().find(|(k, f)| {
                !used_new.contains(*k) && f.language == left.language && f.lines == left.lines
            }) {
                pairs.push((Some(*left), Some(*right)));
                used_old.insert(key.clone());
                used_new.insert(rk.clone());
            }
        }
        for (key, f) in &old {
            if !used_old.contains(key) {
                pairs.push((Some(*f), None))
            }
        }
        for (key, f) in &new {
            if !used_new.contains(key) {
                pairs.push((None, Some(*f)))
            }
        }
    }
    for (left, right) in pairs {
        if !o.flag("git-diff-all")
            && o.inputs.len() == 2
            && o.inputs.iter().all(|p| !std::path::Path::new(p).exists())
            && left.is_some_and(|l| right.is_some_and(|n| l.key == n.key && l.lines == n.lines))
        {
            continue;
        }
        if let (Some(l), Some(n)) = (left, right) {
            if l.language != n.language {
                let removed = whole(l, false);
                let added = whole(n, true);
                r.languages
                    .entry(l.language.clone())
                    .or_default()
                    .add(&removed);
                r.languages
                    .entry(n.language.clone())
                    .or_default()
                    .add(&added);
                r.total.add(&removed);
                r.total.add(&added);
                continue;
            }
        }
        let file = left.or(right).unwrap();
        let d = match (left, right) {
            (Some(l), Some(n)) => match compare(l, n, o) {
                Ok(d) => d,
                Err(e) if e.downcast_ref::<crate::counter::TimedOut>().is_some() => {
                    if !o.flag("quiet") || o.flag("show-errors") {
                        eprintln!("{}: {e}", l.name)
                    };
                    continue;
                }
                Err(e) => return Err(e),
            },
            (Some(l), None) => whole(l, false),
            (None, Some(n)) => whole(n, true),
            _ => unreachable!(),
        };
        let status = if left.is_none() {
            "added"
        } else if right.is_none() {
            "removed"
        } else if d.same.files == 1 {
            "same"
        } else {
            "modified"
        };
        let alignment = match (left, right) {
            (None, Some(n)) => format!("  + {} ; {}", n.name, n.language),
            (Some(l), None) => format!("  - {} ; {}", l.name, l.language),
            (Some(l), Some(n)) => format!(
                "  {} {} | {} ; {}",
                if status == "same" { "==" } else { "!=" },
                l.name,
                n.name,
                l.language
            ),
            _ => unreachable!(),
        };
        r.alignment.push(alignment);
        r.languages
            .entry(file.language.clone())
            .or_default()
            .add(&d);
        r.total.add(&d);
        r.files
            .insert(file.name.clone(), (file.language.clone(), d));
    }
    r.elapsed = start.elapsed().as_secs_f64();
    if let Some(p) = o.value("diff-alignment") {
        fs::write(p, r.alignment.join("\n") + "\n")?
    };
    Ok(r)
}
