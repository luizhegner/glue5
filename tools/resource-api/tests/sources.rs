use glue5_resource_api::{Resources, sources::{Candidate, Source, SourceKind, Sources}};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("glue5-sources-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn sources(&self) -> Sources {
        let mut sources = Sources::default();
        for (id, kind) in [("web", SourceKind::WebAssembly), ("legacy", SourceKind::Legacy)] {
            let root = self.0.join(id);
            let cache = self.0.join(format!("{id}-cache"));
            fs::create_dir(&root).unwrap();
            fs::create_dir(&cache).unwrap();
            fs::write(root.join("asset"), id.as_bytes()).unwrap();
            sources.register(id, Source { kind, resources: Resources::open(&root, &cache, None).unwrap() }).unwrap();
        }
        sources
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn legacy_has_priority_even_when_web_candidate_is_first() {
    let fixture = Fixture::new();
    let sources = fixture.sources();
    let candidates = [Candidate { source: "web", uri: "asset" }, Candidate { source: "legacy", uri: "asset" }];
    let result = sources.resolve(&candidates, |_, _, bytes| Ok(bytes.to_vec())).unwrap_or_else(|_| panic!("resolution failed"));
    assert_eq!(result.source, "legacy");
    assert_eq!(result.value, b"legacy");
    assert!(result.rejected.is_empty());
}

#[test]
fn incompatible_legacy_falls_back_with_reason() {
    let fixture = Fixture::new();
    let sources = fixture.sources();
    let candidates = [Candidate { source: "web", uri: "asset" }, Candidate { source: "legacy", uri: "asset" }];
    let result = sources.resolve(&candidates, |kind, _, bytes| {
        if kind == SourceKind::Legacy { Err("unsupported resource version".into()) }
        else { Ok(bytes.to_vec()) }
    }).unwrap_or_else(|_| panic!("fallback failed"));
    assert_eq!(result.source, "web");
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].reason, "unsupported resource version");
}

#[test]
fn missing_sources_and_files_are_reported() {
    let fixture = Fixture::new();
    let sources = fixture.sources();
    let candidates = [Candidate { source: "legacy", uri: "missing" }, Candidate { source: "unknown", uri: "asset" }];
    let errors = sources.resolve(&candidates, |_, _, _| Ok(())).err().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].source, "legacy");
    assert_eq!(errors[1].reason, "unknown source ID");
}
