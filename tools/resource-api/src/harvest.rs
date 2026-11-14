//! Stage 1 of the common, source-backed GLUE 5 resource harvest.
//! All selectors are explicit. Never scan outside the approved source,
//! install into the live game, guess missing fonts, or publish extracted bytes.
use crate::{Resources, Result, MAX_BYTES, awc, gfx, hash, io, ytd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_SCHEMA: &str = "glue5.harvest-cache/1";
const PLAN_SCHEMA: &str = "glue5.harvest-plan/1";
const MAX_ITEMS: usize = 16;

/// One named asset from the owner's explicitly selected installation.
/// The URI uses the same loose-or-RPF grammar as Resources::read.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub uri: String,
    pub selection: Selection,
}

/// Supported, genuinely decoded resource classes, plus an index-only first
/// step for fonts. A font index is NOT a converted Chalet/TTF.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    GfxBitmap { character_id: u16 },
    YtdTexture { texture_index: usize },
    AwcMono { stream_id: u32 },
    ScaleformFontIndex,
}

impl Selection {
    fn output(&self) -> &'static str {
        match self {
            Self::GfxBitmap { .. } | Self::YtdTexture { .. } => "image.png",
            Self::AwcMono { .. } => "audio.wav",
            Self::ScaleformFontIndex => "font-index.json",
        }
    }
    fn conversion(&self) -> &'static str {
        match self {
            Self::GfxBitmap { .. } => "gfx-lossless-bitmap/1",
            Self::YtdTexture { .. } => "ytd-legacy-base-mip/1",
            Self::AwcMono { .. } => "awc-mono-wav/1",
            Self::ScaleformFontIndex => "gfx-font-tag-inventory/1 (index only, not TTF)",
        }
    }
}

struct Prepared {
    id: String,
    uri: String,
    selection: Selection,
    source_sha: String,
    output_sha: String,
    key: String,
    content: Vec<u8>,
}

fn validate(items: &[Item]) -> Result<()> {
    if items.is_empty() || items.len() > MAX_ITEMS {
        return Err("harvest requires 1..16 explicitly selected items".into());
    }
    let mut seen = HashSet::new();
    for item in items {
        let segments: Vec<_> = item.id.split('/').collect();
        if item.id.len() > 96 || segments.is_empty() || segments.len() > 3
            || segments.iter().any(|part| part.is_empty() || part.len() > 40
                || !part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'))
            || !seen.insert(item.id.clone())
        {
            return Err("asset IDs must be unique lowercase slash-separated names (max 3 segments, 96 bytes)".into());
        }
        if item.uri.is_empty() || item.uri.len() > 4096 {
            return Err("each asset needs one nonempty relative source URI".into());
        }
        if let Selection::AwcMono { stream_id } = item.selection
            && stream_id > 0x1fff_ffff {
                return Err("AWC waveform ID exceeds 29 bits".into());
            }
    }
    Ok(())
}

fn prepare(api: &Resources, item: &Item) -> Result<Prepared> {
    // The existing API enforces explicit root boundaries, safe relative paths,
    // supported RPF encryption and bounded reads.
    let source = api.read(&item.uri)?;
    prepare_bytes(item, &source)
}

fn prepare_bytes(item: &Item, source: &[u8]) -> Result<Prepared> {
    let source_sha = hash(&source);
    let content = match &item.selection {
        Selection::GfxBitmap { character_id } =>
            gfx::Movie::open(&source)?.bitmap(*character_id)?.png()?,
        Selection::YtdTexture { texture_index } =>
            ytd::Dictionary::open(&source)?.bitmap(*texture_index)?.png()?,
        Selection::AwcMono { stream_id } =>
            awc::Bank::open(&source)?.wav(*stream_id)?,
        Selection::ScaleformFontIndex => {
            let movie = gfx::Movie::open(&source)?;
            let tags: Vec<_> = movie.tags.iter()
                .filter(|tag| matches!(tag.code, 48 | 75 | 91))
                .map(|tag| json!({
                    "tag":tag.code,"character_id":tag.character_id,
                    "bytes":tag.bytes,"offset":tag.offset,
                    "convertible":matches!(tag.code,48|75)
                }))
                .collect();
            if tags.is_empty() {
                return Err("this Scaleform movie contains no embedded font tags; choose the actual font library, not a movie importing it".into());
            }
            serde_json::to_vec_pretty(&json!({
                "schema":"glue5.font-index/1","source_sha256":source_sha,
                "tags":tags,"ttf_generated":false,
                "note":"SWF DefineFont2/3 to TTF conversion is not implemented"
            })).map_err(|e| e.to_string())?
        },
    };
    if content.is_empty() || content.len() as u64 > MAX_BYTES {
        return Err("derived asset is empty or exceeds the decoded resource budget".into());
    }
    let output_sha = hash(&content);
    let recipe = serde_json::to_vec(&item.selection).map_err(|e| e.to_string())?;
    let mut identity = Vec::new();
    identity.extend_from_slice(CACHE_SCHEMA.as_bytes());
    identity.extend_from_slice(item.id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(item.uri.as_bytes());
    identity.push(0);
    identity.extend_from_slice(&recipe);
    identity.extend_from_slice(source_sha.as_bytes());
    identity.extend_from_slice(output_sha.as_bytes());
    let key = hash(&identity);
    Ok(Prepared {
        id: item.id.clone(), uri: item.uri.clone(), selection: item.selection.clone(),
        source_sha, output_sha, key, content,
    })
}

/// Candidate paths and selections are produced by installation discovery, not
/// by a user-facing import dialog. Layouts and waveform IDs can differ by build.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub source: String,
    pub uri: String,
    pub selection: Selection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub candidates: Vec<Candidate>,
}

impl crate::sources::Sources {
    /// Decode Legacy first and fall back to WebAssembly, recording failures.
    /// Each successful item is cached atomically in its selected source cache.
    /// A failed item is reported without discarding other successful items.
    pub fn harvest(&self, requests: &[Request], apply: bool) -> Result<Value> {
        if requests.is_empty() || requests.len() > MAX_ITEMS {
            return Err("harvest requires 1..16 resources".into());
        }
        let mut ids = HashSet::new();
        for request in requests {
            if !ids.insert(&request.id) || request.candidates.is_empty() || request.candidates.len() > 32 {
                return Err("unique resource IDs and 1..32 candidates are required".into());
            }
            let mut addresses = HashSet::new();
            for candidate in &request.candidates {
                self.get(&candidate.source)?;
                if !addresses.insert((&candidate.source, &candidate.uri)) {
                    return Err("duplicate candidate address".into());
                }
                validate(&[Item { id: request.id.clone(), uri: candidate.uri.clone(), selection: candidate.selection.clone() }])?;
            }
        }
        let mut reports = Vec::new();
        for request in requests {
            // Resolve one source-qualified candidate at a time, preserving
            // caller DLC precedence within each installation kind.
            let mut rejected = Vec::new();
            let mut selected = None;
            for kind in [crate::sources::SourceKind::Legacy, crate::sources::SourceKind::WebAssembly] {
                for candidate in &request.candidates {
                    let source = self.get(&candidate.source)?;
                    if source.kind != kind { continue; }
                    let item = Item { id: request.id.clone(), uri: candidate.uri.clone(), selection: candidate.selection.clone() };
                    match prepare(&source.resources, &item) {
                        Ok(prepared) => { selected = Some((candidate, prepared)); break; }
                        Err(reason) => rejected.push(json!({"source":candidate.source,"uri":candidate.uri,"reason":reason})),
                    }
                }
                if selected.is_some() { break; }
            }
            let Some((candidate, prepared)) = selected else {
                reports.push(json!({"id":request.id,"status":"unavailable","rejected":rejected}));
                continue;
            };
            let api = &self.get(&candidate.source)?.resources;
            let result = (|| -> Result<&str> {
                let base = namespace(api, apply)?;
                if apply {
                    Ok(if save(&base.ok_or("cache namespace missing")?, &prepared)? { "reused" } else { "installed" })
                } else {
                    let cached = match base { Some(base) => cache_hit(&base, &prepared)?, None => false };
                    Ok(if cached { "cached" } else { "ready" })
                }
            })();
            let mut report = summary(&prepared, result.as_ref().copied().unwrap_or("cache_error"));
            report["source"] = json!(candidate.source);
            report["rejected"] = json!(rejected);
            if let Err(reason) = result { report["error"] = json!(reason); }
            reports.push(report);
        }
        Ok(json!({"schema":"glue5.unified-harvest/1","items":reports,"apply":apply}))
    }
}

fn receipt(item: &Prepared) -> Value {
    json!({
        "schema":CACHE_SCHEMA,
        "id":item.id,
        "uri":item.uri,
        "selection":item.selection,
        "conversion":item.selection.conversion(),
        "source_sha256":item.source_sha,
        "output_sha256":item.output_sha,
        "output_bytes":item.content.len(),
        "output":item.selection.output(),
        "font_index_only":matches!(item.selection, Selection::ScaleformFontIndex)
    })
}

fn checked_directory(parent: &Path, segment: &str, create: bool) -> Result<Option<PathBuf>> {
    let path = parent.join(segment);
    if create && !path.exists() {
        match fs::create_dir(&path) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(e) => return Err(io(e)),
        }
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(Some(path)),
        Ok(_) => Err("harvest cache directory is a symlink or not a directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

fn namespace(api: &Resources, create: bool) -> Result<Option<PathBuf>> {
    let Some(dir) = checked_directory(&api.cache, "harvest", create)? else { return Ok(None) };
    checked_directory(&dir, "v1", create)
}

fn cache_hit(base: &Path, item: &Prepared) -> Result<bool> {
    let Some(dir) = checked_directory(base, &item.key, false)? else { return Ok(false) };
    // Refuse symlinked leaves, including a modified receipt. Never follow
    // unexpected files from another process into the approved cache.
    let artifact = dir.join(item.selection.output());
    let manifest = dir.join("receipt.json");
    for path in [&artifact, &manifest] {
        let meta = fs::symlink_metadata(path).map_err(io)?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_BYTES {
            return Err("cached harvest file has an unexpected type or size".into());
        }
    }
    let bytes = fs::read(&artifact).map_err(io)?;
    let saved: Value = serde_json::from_slice(&fs::read(manifest).map_err(io)?)
        .map_err(|_| "cached harvest receipt is invalid JSON")?;
    if bytes.len() != item.content.len() || hash(&bytes) != item.output_sha || saved != receipt(item) {
        return Err("existing harvested resource or receipt failed integrity validation; no overwrite performed".into());
    }
    Ok(true)
}

fn save(base: &Path, item: &Prepared) -> Result<bool> {
    if cache_hit(base, item)? { return Ok(true); }
    let ticks = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is invalid")?.as_nanos();
    let temp = base.join(format!(".pending-{}-{}-{ticks}", item.key, std::process::id()));
    fs::create_dir(&temp).map_err(io)?;
    let result = (|| -> Result<()> {
        let receipt_bytes = serde_json::to_vec_pretty(&receipt(item)).map_err(|e|e.to_string())?;
        for (name, content) in [
            (item.selection.output(), item.content.as_slice()),
            ("receipt.json", receipt_bytes.as_slice()),
        ] {
            let mut file = OpenOptions::new().create_new(true).write(true)
                .open(temp.join(name)).map_err(io)?;
            file.write_all(content).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        fs::rename(&temp, base.join(&item.key)).map_err(io)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
        // A concurrent harvest could have installed the same complete item.
        if cache_hit(base, item)? { return Ok(true); }
    }
    result?;
    Ok(false)
}

impl Resources {
    /// Validate and decode only explicitly selected entries. No filesystem
    /// mutations: a plan shows real codecs and source hashes, not guesses.
    pub fn harvest_plan(&self, items: &[Item]) -> Result<Value> {
        validate(items)?;
        let mut report = Vec::with_capacity(items.len());
        let base = namespace(self, false)?;
        for item in items {
            let prepared = prepare(self, item)?;
            let cached = match &base {
                Some(root) => cache_hit(root, &prepared)?,
                None => false,
            };
            report.push(summary(&prepared, if cached { "cached" } else { "ready" }));
        }
        Ok(json!({"schema":PLAN_SCHEMA,"items":report,"writes":false}))
    }

    /// Prepare selected assets in a private content-addressed cache, never
    /// under the game installation. Each item is atomically installed.
    /// Partial batch success is reported through its already written receipts.
    pub fn harvest_apply(&self, items: &[Item]) -> Result<Value> {
        validate(items)?;
        let base = namespace(self, true)?.ok_or("could not create harvest cache namespace")?;
        let mut report = Vec::with_capacity(items.len());
        for item in items {
            let prepared = prepare(self, item)?;
            let reused = save(&base, &prepared)?;
            report.push(summary(&prepared, if reused { "reused" } else { "installed" }));
        }
        Ok(json!({"schema":"glue5.harvest-result/1","items":report,
            "cache_relative":"harvest/v1","writes":"cache-only"}))
    }
}

fn summary(item: &Prepared, status: &str) -> Value {
    json!({
        "id":item.id,"uri":item.uri,"selection":item.selection,
        "status":status,
        "source_sha256":item.source_sha,"output_sha256":item.output_sha,
        "bytes":item.content.len(),"conversion":item.selection.conversion(),
        "cache_key":item.key,
        "cache_relative":format!("harvest/v1/{}/{}",item.key,item.selection.output()),
    })
}
