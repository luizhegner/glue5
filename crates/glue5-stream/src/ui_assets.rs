//! Optional frontend resources from the served WebAssembly build.
//! This is independent of the engine catalog and its ordered file IDs.
use std::path::{Path, PathBuf};

pub(crate) const FONT_BLOB: &str = "ui-font-chalet-v1";
pub(crate) const MAX_FONT: u64 = 1024 * 1024;

pub(crate) fn font_path(root: &Path, build: &str) -> Option<PathBuf> {
    // Never let a peer's build identifier escape the streaming cache.
    if build.is_empty() || !build.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return None;
    }
    Some(root.join("b").join(build).join("title").join("chalet.woff"))
}

pub(crate) fn valid_font(bytes: &[u8]) -> bool {
    bytes.len() >= 44 && bytes.len() as u64 <= MAX_FONT && &bytes[..4] == b"wOFF"
        && u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confines_the_build_and_rejects_invalid_fonts() {
        let root = Path::new("game");
        assert!(font_path(root, "../outside").is_none());
        assert!(font_path(root, "a/b").is_none());
        assert!(font_path(root, "").is_none());
        assert_eq!(font_path(root, "test-1").unwrap(), root.join("b/test-1/title/chalet.woff"));
        assert!(!valid_font(b"wOFF"));
        let mut font = vec![0; 44];
        font[..4].copy_from_slice(b"wOFF");
        font[8..12].copy_from_slice(&44u32.to_be_bytes());
        assert!(valid_font(&font));
        font[11] = 43;
        assert!(!valid_font(&font));
    }
}
