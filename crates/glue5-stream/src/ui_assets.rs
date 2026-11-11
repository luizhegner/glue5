//! Optional frontend resources from the served WebAssembly build.
//! This is independent of the engine catalog and its ordered file IDs.
use std::path::{Path, PathBuf};
use sha2::{Digest,Sha256};

pub(crate) const FONT_BLOB: &str = "ui-font-chalet-v1";
pub(crate) const MAX_FONT: u64 = 1024 * 1024;
const MAX_ART: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct Resource {
    pub name: &'static str,
    pub relative: &'static str,
    pub max: u64,
}

macro_rules! art {
    ($file:literal) => { Resource { name: concat!("ui-art-v1:", $file), relative: concat!("title/art/", $file), max: MAX_ART } };
}

/// Only known game paths are served; a network peer cannot choose a path.
/// The first three names retain compatibility with the first UI import build.
pub(crate) const RESOURCES: &[Resource] = &[
    Resource { name: FONT_BLOB, relative: "title/chalet.woff", max: MAX_FONT },
    Resource { name: "ui-art-beach-bg-v1", relative: "title/art/beach_bg.webp", max: MAX_ART },
    Resource { name: "ui-art-beach-fg-v1", relative: "title/art/beach_fg.webp", max: MAX_ART },
    Resource { name: "ui-spinner-v1", relative: "title/spinner.png", max: MAX_FONT },
    art!("ls0_background.webp"), art!("ls0_foreground.webp"),
    art!("ls1_background.webp"), art!("ls1_foreground.webp"), art!("ls1_foreground_franklin.webp"),
    art!("ls2_background.webp"), art!("ls2_foreground.webp"), art!("ls2_foreground_chop.webp"),
    art!("ls3_background.webp"), art!("ls3_foreground.webp"),
    art!("ls4_background.webp"), art!("ls4_foreground.webp"),
    art!("ls5_background.webp"), art!("ls5_foreground.webp"),
    art!("ls6_background.webp"), art!("ls6_foreground.webp"),
    art!("ls7_background.webp"), art!("ls7_foreground.webp"),
    art!("ls8_background.webp"), art!("ls8_foreground.webp"),
    art!("ls9_background.webp"), art!("ls9_foreground.webp"),
    art!("ls10_background.webp"), art!("ls10_foreground.webp"),
    art!("ls11_background.webp"), art!("ls11_foreground.webp"),
    art!("ls12_background.webp"), art!("ls12_foreground_michael.webp"),
    art!("ls13_background.webp"), art!("ls13_foreground.webp"),
    art!("ls14_background.webp"), art!("ls14_foreground.webp"),
    art!("ls15_background.webp"), art!("ls15_foreground.webp"),
    art!("ls16_background.webp"), art!("ls16_foreground.webp"),
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
    if resource.relative.ends_with(".png") {
        return bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR";
    }
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
        && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64 + 8 == bytes.len() as u64
}

pub(crate) fn hash(bytes:&[u8]) -> String { crate::catalog::hex(&Sha256::digest(bytes)) }

/// A same-sized replacement must not keep stale UI bytes in the cache.
pub(crate) fn cached_matches(path:&Path,resource:Resource,size:u64,expected:&str) -> bool {
    if size>resource.max || expected.len()!=64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len()==size) { return false; }
    std::fs::read(path).is_ok_and(|bytes| valid(resource,&bytes) && hash(&bytes)==expected)
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

    #[test]
    fn cached_resource_hash_changes_even_when_the_file_size_does_not() {
        let path=std::env::temp_dir().join(format!("glue5-ui-font-hash-{}.woff",std::process::id()));
        let resource=RESOURCES[0];
        let mut bytes=vec![0;48];
        bytes[..4].copy_from_slice(b"wOFF");
        bytes[8..12].copy_from_slice(&48u32.to_be_bytes());
        let expected=hash(&bytes);
        std::fs::write(&path,&bytes).unwrap();
        assert!(cached_matches(&path,resource,48,&expected));
        bytes[47]=1;
        std::fs::write(&path,&bytes).unwrap();
        assert!(!cached_matches(&path,resource,48,&expected));
        assert!(cached_matches(&path,resource,48,&hash(&bytes)));
        let _=std::fs::remove_file(path);
    }
}
