use crate::{cli::Options, languages::Registry};
use std::path::Path;

// Verified against the behavioral reference: these suffixes are ignored even
// when their contents have a shebang. Explicit language mappings override this.
const NON_SOURCE_EXTENSIONS: [&str; 8] = ["jpg", "png", "gif", "bmp", "tiff", "pdf", "mp3", "wav"];

/// Known media files, recognized from their names without opening them.
pub(super) struct Media {
    /// Whether some file name with this suffix could still map to a language.
    detectable: [bool; NON_SOURCE_EXTENSIONS.len()],
}

impl Media {
    pub(super) fn new(registry: &Registry, o: &Options) -> Self {
        // A forced language applies to every name, including media.
        let forced = o.all("force-lang").iter().any(|v| !v.contains(','));
        Self {
            detectable: NON_SOURCE_EXTENSIONS.map(|ext| forced || registry.maps_suffix(ext)),
        }
    }

    /// True when a media file has no language mapping and is never counted.
    pub(super) fn ignored(&self, p: &Path, registry: &Registry, o: &Options) -> bool {
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        match NON_SOURCE_EXTENSIONS.iter().position(|e| *e == ext) {
            None => false,
            // Large image directories skip detection unless a mapping could apply.
            Some(i) => {
                !self.detectable[i] || registry.detect(&p.to_string_lossy(), "", o).is_none()
            }
        }
    }
}
