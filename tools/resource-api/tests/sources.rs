use glue5_resource_api::{Resources, sources::{Candidate, Source, SourceKind, Sources}};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn bitmap_movie(red: u8) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&[0, red, 20, 30]).unwrap();
    let mut bitmap = vec![17, 0, 5, 1, 0, 1, 0];
    bitmap.extend(z.finish().unwrap());
    let mut body = vec![0, 0, 30, 1, 0];
    body.extend(((20u16 << 6) | bitmap.len() as u16).to_le_bytes());
    body.extend(bitmap);
    body.extend([0, 0]);
    let mut out = b"GFX\x0a".to_vec();
    out.extend((body.len() as u32 + 8).to_le_bytes());
    out.extend(body);
    out
}

#[test]
fn unified_harvest_falls_back_caches_and_detects_corruption() {
    use glue5_resource_api::{HarvestCandidate, HarvestRequest, HarvestSelection};
    let fixture = Fixture::new();
    let sources = fixture.sources();
    fs::write(fixture.0.join("web/asset"), bitmap_movie(10)).unwrap();
    // Legacy currently contains unsupported bytes; Web has a decodable image.
    let requests = [HarvestRequest { id: "loading/layer".into(), candidates:
        ["web", "legacy"].into_iter().map(|source| HarvestCandidate {
            source: source.into(), uri: "asset".into(),
            selection: HarvestSelection::GfxBitmap { character_id: 17 },
        }).collect(),
    }];
    let plan = sources.harvest(&requests, false).unwrap();
    assert_eq!(plan["items"][0]["source"], "web");
    assert_eq!(plan["items"][0]["rejected"][0]["source"], "legacy");
    assert_eq!(fs::read_dir(fixture.0.join("web-cache")).unwrap().count(), 0);
    let applied = sources.harvest(&requests, true).unwrap();
    assert_eq!(applied["items"][0]["status"], "installed");
    assert_eq!(sources.harvest(&requests, true).unwrap()["items"][0]["status"], "reused");
    let path = fixture.0.join("web-cache").join(applied["items"][0]["cache_relative"].as_str().unwrap());
    fs::write(&path, b"corrupt").unwrap();
    assert_eq!(sources.harvest(&requests, true).unwrap()["items"][0]["status"], "cache_error");
    assert_eq!(fs::read(path).unwrap(), b"corrupt");
    fs::write(fixture.0.join("legacy/asset"), bitmap_movie(40)).unwrap();
    let legacy = sources.harvest(&requests, true).unwrap();
    assert_eq!(legacy["items"][0]["source"], "legacy");
    assert_eq!(legacy["items"][0]["status"], "installed");
    assert_ne!(legacy["items"][0]["cache_key"], applied["items"][0]["cache_key"]);
}

#[test]
fn unified_harvest_reports_unavailable_without_cache_writes() {
    use glue5_resource_api::{HarvestCandidate, HarvestRequest, HarvestSelection};
    let fixture = Fixture::new();
    let sources = fixture.sources();
    let requests = [HarvestRequest { id: "loading/layer".into(), candidates: vec![HarvestCandidate {
        source: "legacy".into(), uri: "missing.gfx".into(),
        selection: HarvestSelection::GfxBitmap { character_id: 17 },
    }] }];
    let result = sources.harvest(&requests, true).unwrap();
    assert_eq!(result["items"][0]["status"], "unavailable");
    assert_eq!(fs::read_dir(fixture.0.join("legacy-cache")).unwrap().count(), 0);
}

#[test]
fn nested_sound_bank_retains_innermost_container_identity() {
    assert_eq!(glue5_resource_api::rel::bank_name(
        "update/update.rpf::x64/audio/sfx/RESIDENT.rpf::frontend.awc"
    ).unwrap(), "resident/frontend");
    assert!(glue5_resource_api::rel::bank_name(
        "update.rpf::../RESIDENT.rpf::frontend.awc"
    ).is_err());
}
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
