//! Optional frontend resources from the served WebAssembly build.
//! This is independent of the engine catalog and its ordered file IDs.
use std::path::{Path, PathBuf};

pub(crate) const FONT_BLOB: &str = "ui-font-chalet-v1";
pub(crate) const MAX_FONT: u64 = 1024 * 1024;
const MAX_ART: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct Resource {
    pub name: &'static str,
    pub relative: &'static str,
    pub max: u64,
}

/// Only known game paths are served; a network peer cannot choose a path.
pub(crate) const RESOURCES: [Resource; 3] = [
    Resource { name: FONT_BLOB, relative: "title/chalet.woff", max: MAX_FONT },
    Resource { name: "ui-art-beach-bg-v1", relative: "title/art/beach_bg.webp", max: MAX_ART },
    Resource { name: "ui-art-beach-fg-v1", relative: "title/art/beach_fg.webp", max: MAX_ART },
];

pub(crate) fn resource_path(root: &Path, build: &str, resource: Resource) -> Option<PathBuf> {
    // Never let a peer's build identifier escape the streaming cache.
    if build.is_empty() || !build.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return None;
    }
    Some(root.join("b").join(build).join(resource.relative))
}

pub(crate) fn valid(resource: Resource, bytes: &[u8]) -> bool {
    if bytes.len() as u64 > resource.max { return false; }
    if resource.name == FONT_BLOB {
        return bytes.len() >= 44 && &bytes[..4] == b"wOFF"
            && u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize == bytes.len();
    }
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
        && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64 + 8 == bytes.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confines_the_build_and_rejects_invalid_fonts() {
        let root = Path::new("game");
        let resource = RESOURCES[0];
        assert!(resource_path(root, "../outside", resource).is_none());
        assert!(resource_path(root, "a/b", resource).is_none());
        assert!(resource_path(root, "", resource).is_none());
        assert_eq!(resource_path(root, "test-1", resource).unwrap(), root.join("b/test-1/title/chalet.woff"));
        assert!(!valid(resource, b"wOFF"));
        let mut font = vec![0; 44];
        font[..4].copy_from_slice(b"wOFF");
        font[8..12].copy_from_slice(&44u32.to_be_bytes());
        assert!(valid(resource, &font));
        font[11] = 43;
        assert!(!valid(resource, &font));
        let art = b"RIFF\x04\0\0\0WEBP";
        assert!(valid(RESOURCES[1], art));
        assert!(!valid(RESOURCES[1], &art[..11]));
    }
}
