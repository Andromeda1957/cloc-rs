//! Parallel directory traversal with the selection semantics of the serial walk.
//!
//! Directory listings are independent filesystem reads, so they run on the
//! worker pool. A cold or contended filesystem then serves several requests at
//! once instead of waiting for each directory in turn. The caller sorts the
//! result, so traversal order never reaches a report.

use super::{Selection, media::Media};
use crate::{cli::Options, languages::Registry, progress::Progress};
use anyhow::Result;
use rayon::prelude::*;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use walkdir::WalkDir;

/// A file found under a directory argument.
pub(super) struct Found {
    pub(super) path: PathBuf,
    /// The path may pass through a symbolic link, so it needs full resolution.
    pub(super) link: bool,
}

pub(super) struct Walker<'a> {
    pub(super) sel: &'a Selection,
    pub(super) media: &'a Media,
    pub(super) registry: &'a Registry,
    pub(super) o: &'a Options,
    pub(super) progress: &'a Progress,
    pub(super) discovered: &'a AtomicUsize,
}

impl Walker<'_> {
    /// Lists candidate files below `root` in no particular order.
    ///
    /// Without link following this matches the serial walk: the root is always
    /// entered, a root that is not itself a link must pass directory selection,
    /// excluded directories are never read, and regular files plus links to
    /// files are candidates. Following links keeps the serial walk, including
    /// its filesystem loop detection.
    pub(super) fn walk(&self, root: &Path, max_depth: usize, follow: bool) -> Result<Vec<Found>> {
        if follow {
            return self.follow(root, max_depth);
        }
        let root_is_link = fs::symlink_metadata(root)
            .map_err(|e| io_error(root, e))?
            .file_type()
            .is_symlink();
        if (!root_is_link && !self.sel.dir(root).unwrap_or(false)) || max_depth == 0 {
            return Ok(Vec::new());
        }
        self.directory(root, 1, max_depth)
    }

    fn directory(&self, dir: &Path, depth: usize, max_depth: usize) -> Result<Vec<Found>> {
        let mut files = Vec::new();
        let mut directories = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| io_error(dir, e))? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| io_error(&path, e))?;
            if kind.is_dir() {
                if depth < max_depth && self.sel.dir(&path).unwrap_or(false) {
                    directories.push(path);
                }
            } else if kind.is_file() || (kind.is_symlink() && path.is_file()) {
                self.candidate(path, kind.is_symlink(), &mut files);
            }
        }
        let nested: Vec<Result<Vec<Found>>> = directories
            .par_iter()
            .map(|d| self.directory(d, depth + 1, max_depth))
            .collect();
        for result in nested {
            files.extend(result?);
        }
        Ok(files)
    }

    fn follow(&self, root: &Path, max_depth: usize) -> Result<Vec<Found>> {
        let mut files = Vec::new();
        for entry in WalkDir::new(root)
            .max_depth(max_depth)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| !e.file_type().is_dir() || self.sel.dir(e.path()).unwrap_or(false))
        {
            let entry = entry?;
            if entry.file_type().is_file()
                || (entry.file_type().is_symlink() && entry.path().is_file())
            {
                self.candidate(entry.into_path(), true, &mut files);
            }
        }
        Ok(files)
    }

    fn candidate(&self, path: PathBuf, link: bool, files: &mut Vec<Found>) {
        let n = self.discovered.fetch_add(1, Ordering::Relaxed) + 1;
        self.progress.update("Scanning", n, None);
        // Known media is pruned before any file is opened or resolved.
        if !self.media.ignored(&path, self.registry, self.o) {
            files.push(Found { path, link });
        }
    }
}

/// The serial walk's report for a path it could not read, including the
/// underlying error as the cause, so messages stay identical.
fn io_error(path: &Path, error: io::Error) -> anyhow::Error {
    let message = format!("IO error for operation on {}: {}", path.display(), error);
    anyhow::Error::new(error).context(message)
}
