fn main() {
    if let Err(e) = cloc_rs::run() {
        eprintln!("cloc-rs: {e:#}");
        std::process::exit(2)
    }
}
