mod basic;
pub mod cli;
pub mod counter;
pub mod diff;
pub mod input;
pub mod languages;
pub mod model;
pub mod output;
mod progress;
pub mod scratch;

use anyhow::{Result, bail};
pub fn run() -> Result<()> {
    let o = cli::Options::parse()?;
    if o.flag("help") {
        print!("{}", cli::help());
        return Ok(());
    }
    if o.flag("version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if o.flag("show-os") {
        println!(
            "{}",
            if o.flag("windows") || cfg!(windows) {
                "Windows"
            } else {
                "UNIX"
            }
        );
        return Ok(());
    }
    if o.value("no-autogen") == Some("list") {
        println!("{}", input::GENERATED.replace(',', "\n"));
        return Ok(());
    }
    let registry = languages::Registry::new(&o)?;
    for key in ["show-lang", "show-ext"] {
        if let Some(query) = o.value(key) {
            print!("{}", registry.show(key == "show-ext", query));
            return Ok(());
        }
    }
    if let Some(n) = o.value("explain") {
        let d = registry
            .get(n)
            .ok_or_else(|| anyhow::anyhow!("unknown language {n}"))?;
        println!("{}", serde_json::to_string_pretty(d)?);
        return Ok(());
    }
    for key in ["write-lang-def", "write-lang-def-incl-dup"] {
        if let Some(p) = o.value(key) {
            registry.write(p)?;
            return Ok(());
        }
    }
    // Validate patterns before directory pruning, including patterns that match no files.
    for key in [
        "match-f",
        "not-match-f",
        "match-d",
        "not-match-d",
        "include-content",
        "exclude-content",
        "skip-archive",
    ] {
        for p in o.all(key) {
            fancy_regex::Regex::new(p)?;
        }
    }
    for n in o
        .list("include-lang")
        .into_iter()
        .chain(o.list("exclude-lang"))
    {
        if registry.get(&n).is_none() {
            bail!("unknown language {n}")
        }
    }
    if o.flag("use-sloccount") && !o.flag("quiet") {
        eprintln!(
            "cloc-rs uses its native Rust counters; --use-sloccount is a compatibility alias"
        );
    }
    if o.flag("sum-reports") {
        let mut r = output::sum_reports(&o.inputs)?;
        basic::filter(&mut r, &o)?;
        return output::emit(&output::render_count(&r, &o)?, &o);
    }
    if o.flag("diff") {
        let mut count_opts = o.clone();
        count_opts.flags.remove("diff");
        count_opts.flags.insert("retain-detail".into());
        count_opts.values.remove("list-file");
        count_opts.flags.insert("skip-uniqueness".into());
        if o.has("basic") {
            // Select minor languages across the pair, so a deletion cannot
            // disappear simply because its new-side percentage is zero.
            count_opts.set("basic", "0".into());
        }
        let (old_inputs, new_inputs) = if let Some(v) = o.values.get("diff-list-files") {
            (input::list_inputs(&v[0])?, input::list_inputs(&v[1])?)
        } else if o.has("diff-list-file") {
            let s = std::fs::read_to_string(o.value("diff-list-file").unwrap())?;
            let pairs = diff::alignment_pairs(&s)?;
            let a = pairs.iter().filter_map(|(a, _)| a.clone()).collect();
            let b = pairs.iter().filter_map(|(_, b)| b.clone()).collect();
            (a, b)
        } else {
            if o.inputs.len() != 2 {
                bail!("--diff requires exactly two inputs")
            };
            (vec![o.inputs[0].clone()], vec![o.inputs[1].clone()])
        };
        let mut a = input::report(&count_opts, &registry, &old_inputs)?;
        let mut b = input::report(&count_opts, &registry, &new_inputs)?;
        basic::filter_pair(&mut a, &mut b, &o)?;
        if o.has("basic") {
            input::write_diagnostics(&b, &o)?;
        }
        let d = diff::compare_reports(&a, &b, &o)?;
        if o.flag("count-and-diff") {
            if let Some(path) = o.value("report-file") {
                for (label, r) in [("1", &a), ("2", &b)] {
                    let mut opt = count_opts.clone();
                    opt.set("report-file", format!("{path}.{label}"));
                    output::emit(&output::render_count(r, &opt)?, &opt)?
                }
                let mut opt = o.clone();
                opt.set("report-file", format!("{path}.diff"));
                output::emit(&output::render_diff(&d, &opt)?, &opt)?;
            } else {
                for r in [&a, &b] {
                    output::emit(&output::render_count(r, &count_opts)?, &count_opts)?
                }
                output::emit(&output::render_diff(&d, &o)?, &o)?
            }
        } else {
            output::emit(&output::render_diff(&d, &o)?, &o)?
        }
    } else {
        let r = input::report(&o, &registry, &o.inputs)?;
        output::emit(&output::render_count(&r, &o)?, &o)?
    }
    Ok(())
}
