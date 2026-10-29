//! The Studio's web page, embedded at build time (build.rs): each file with
//! its gzip copy when that is smaller.

pub struct Asset {
    pub body: &'static [u8],
    pub gzip: Option<&'static [u8]>,
    pub mime: &'static str,
    pub etag: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// The file served at `path` (`/` is the Studio, `/control` the control page).
pub fn find(path: &str) -> Option<&'static Asset> {
    let path = match path {
        "/" => "/index.html",
        "/control" | "/control/" => "/control.html",
        other => other,
    };
    ASSETS.binary_search_by(|(p, _)| (*p).cmp(path)).ok().map(|i| &ASSETS[i].1)
}
