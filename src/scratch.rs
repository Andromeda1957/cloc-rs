use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
pub struct Scratch {
    pub path: PathBuf,
}
impl Scratch {
    pub fn new(base: Option<&str>) -> Result<Self> {
        let root = base.map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&root)?;
        let path = root.join(format!(
            "cloc-rs-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)
            .with_context(|| format!("creating scratch directory {}", path.display()))?;
        Ok(Self { path })
    }
    pub fn extract(&self, command: &str, file: &Path) -> Result<()> {
        // Substitute only a shell-quoted absolute pathname; the command itself is explicitly supplied by the caller.
        let file = fs::canonicalize(file)?;
        let quoted = if cfg!(windows) {
            format!("\"{}\"", file.to_string_lossy().replace('"', "\"\""))
        } else {
            format!("'{}'", file.to_string_lossy().replace('\'', "'\\''"))
        };
        let cmd = command.replace(">FILE<", &quoted);
        if !command.contains(">FILE<") {
            bail!("--extract-with requires a >FILE< placeholder")
        }
        let mut process = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &cmd]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &cmd]);
            c
        };
        let output = process
            .current_dir(&self.path)
            .output()
            .context("starting extraction command")?;
        if !output.status.success() {
            bail!(
                "extraction command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        };
        Ok(())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
