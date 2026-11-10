//! Local, read-only source access for GLUE 5 tooling. Public Rust API plus a
//! versioned JSON-lines adapter; no HTTP listener or bundled proprietary data.
mod archive;
pub mod awc;
pub mod gfx;
pub use archive::{Archive, Entry, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

pub const API_VERSION: u32 = 1;
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LIST: usize = 512;

pub(crate) fn safe_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9')) {
        return false;
    }
    !name.is_empty() && name != "." && name != ".." && name.len() <= 255
        && !name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        && !name.ends_with(['.', ' '])
}
fn relative(path: &str) -> Result<()> {
    if path.len() > 4096 || (!path.is_empty() && !path.split('/').all(safe_name)) { return Err("expected a safe slash-separated relative path".into()); }
    Ok(())
}
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn io(error: std::io::Error) -> String { error.to_string() }

pub struct Resources {
    root: PathBuf,
    cache: PathBuf,
    aes_key: Option<[u8; 32]>,
}

impl Resources {
    /// Both directories must already exist. Sources and exports may not overlap.
    /// Keys are supplied by the caller, are never logged, and are not in receipts.
    pub fn open(root: &Path, cache: &Path, aes_key: Option<[u8; 32]>) -> Result<Self> {
        let root = root.canonicalize().map_err(io)?;
        let cache = cache.canonicalize().map_err(io)?;
        if !root.is_dir() || !cache.is_dir() || root.starts_with(&cache) || cache.starts_with(&root) {
            return Err("source and export cache must be separate, existing directories".into());
        }
        Ok(Self { root, cache, aes_key })
    }
    fn file(&self, path: &str) -> Result<PathBuf> {
        relative(path)?;
        let file = self.root.join(path).canonicalize().map_err(io)?;
        if !file.starts_with(&self.root) { return Err("path or link escapes the approved source".into()); }
        Ok(file)
    }
    fn archive(&self, path: &str) -> Result<Archive> { Archive::open(File::open(self.file(path)?).map_err(io)?, self.aes_key.as_ref()) }

    /// URI grammar: relative loose path, or archive.rpf::relative/entry.gfx.
    /// Nested archives are deliberately explicit unsupported capability in v1.
    pub fn read(&self, uri: &str) -> Result<Vec<u8>> {
        if let Some((archive, entry)) = uri.split_once("::") {
            relative(entry)?;
            if entry.is_empty() || entry.contains("::") { return Err("invalid entry URI; nested archives are not supported in v1".into()); }
            self.archive(archive)?.read(entry, MAX_BYTES)
        } else {
            let mut file = File::open(self.file(uri)?).map_err(io)?;
            if file.metadata().map_err(io)?.len() > MAX_BYTES { return Err("loose resource exceeds extraction limit".into()); }
            let mut bytes = Vec::new();
            (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(io)?;
            if bytes.len() as u64 > MAX_BYTES { return Err("source grew beyond extraction limit".into()); }
            Ok(bytes)
        }
    }

    pub fn list(&self, path: &str, offset: usize, limit: usize) -> Result<Value> {
        if limit == 0 || limit > MAX_LIST { return Err("list limit must be 1..512".into()); }
        let file = self.file(path)?;
        let mut entries: Vec<Value>;
        let mut metadata = json!({});
        if file.is_dir() {
            let mut found = Vec::new();
            for (seen, item) in fs::read_dir(file).map_err(io)?.enumerate() {
                let item = item.map_err(io)?;
                if seen >= 100_000 { return Err("directory inventory limit exceeded".into()); }
                let kind = item.file_type().map_err(io)?;
                if kind.is_symlink() { continue; }
                let name = item.file_name().to_str().ok_or("non-UTF8 path")?.to_owned();
                if !safe_name(&name) { continue; }
                let relative = if path.is_empty() { name } else { format!("{path}/{name}") };
                // Also reject junctions/reparse points escaping the approved root.
                if self.file(&relative).is_err() { continue; }
                let len = item.metadata().map_err(io)?.len();
                found.push(json!({"uri":relative,"kind":if kind.is_dir(){"directory"}else{"file"},"stored_bytes":len}));
            }
            entries = found;
        } else {
            let archive = self.archive(path)?;
            metadata = json!({"archive_encryption":archive.encryption,"name_shift":archive.name_shift});
            entries = archive.entries.into_iter().map(|entry| {
                let mut value = serde_json::to_value(&entry).unwrap();
                value["uri"] = json!(format!("{path}::{}", entry.path));
                value
            }).collect();
        }
        entries.sort_by(|a,b| a["uri"].as_str().cmp(&b["uri"].as_str()));
        let total = entries.len();
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        Ok(json!({"entries":page,"total":total,"offset":offset,"next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None},"metadata":metadata}))
    }

    pub fn inspect(&self, uri: &str) -> Result<Value> {
        let bytes = self.read(uri)?;
        let magic = bytes.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>();
        let format = match bytes.get(..4) {
            Some(b"RSC7") => "rsc7", Some(b"7FPR") => "rpf7", Some(b"ADAT") | Some(b"TADA") => "awc",
            Some(signature) if matches!(&signature[..3], b"GFX" | b"CFX" | b"FWS" | b"CWS") => "scaleform-swf",
            _ if std::str::from_utf8(&bytes).is_ok() => "utf8-text", _ => "binary",
        };
        Ok(json!({"uri":uri,"decoded_bytes":bytes.len(),"sha256":hash(&bytes),"format":format,"magic_prefix_hex":magic}))
    }

    /// Export is opt-in. A fresh bundle is created exclusively; no source
    /// files, prior bundles, engine overlays or live server data are replaced.
    pub fn extract(&self, uri: &str) -> Result<Value> {
        let bytes = self.read(uri)?;
        self.export(uri, &bytes, "resource.bin", "no format conversion; RPF AES and deflate decoded as declared", Value::Null)
    }

    fn export(&self, uri: &str, bytes: &[u8], output: &str, conversion: &str, selection: Value) -> Result<Value> {
        let digest = hash(bytes);
        let ticks = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "invalid clock")?.as_nanos();
        let bundle = format!("extract-{ticks}-{}", std::process::id());
        let directory = self.cache.join(&bundle);
        fs::create_dir(&directory).map_err(io)?;
        let write = |name: &str, bytes: &[u8]| -> Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(directory.join(name)).map_err(io)?;
            file.write_all(bytes).map_err(io)?;
            file.sync_all().map_err(io)
        };
        write(output, bytes)?;
        let source_path = uri.split_once("::").map_or(uri, |(archive, _)| archive);
        let source_metadata = fs::metadata(self.file(source_path)?).map_err(io)?;
        let receipt = json!({"schema":"glue5.resource-receipt/1","source_uri":uri,"source_file_bytes":source_metadata.len(),
            "source_mtime_unix_ms":source_metadata.modified().ok().and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|t|t.as_millis()),
            "decoded_bytes":bytes.len(),"sha256":digest,"output":output,"conversion":conversion,"selection":selection,
            "extractor_version":env!("CARGO_PKG_VERSION")});
        write("receipt.json", &serde_json::to_vec_pretty(&receipt).map_err(|e|e.to_string())?)?;
        Ok(json!({"bundle":bundle,"receipt":receipt}))
    }


    /// Discover waveform IDs; script event names require separate verified metadata.
    pub fn audio_list(&self, uri: &str, offset: usize, limit: usize) -> Result<Value> {
        if limit == 0 || limit > MAX_LIST { return Err("list limit must be 1..512".into()); }
        let bytes = self.read(uri)?;
        let bank = awc::Bank::open(&bytes)?;
        let total = bank.streams.len();
        let streams: Vec<_> = bank.streams.iter().skip(offset).take(limit).collect();
        Ok(json!({"uri":uri,"source_sha256":hash(&bytes),"big_endian":bank.big_endian,
            "encrypted":bank.encrypted,"streamed":bank.streamed,"streams":streams,"total":total,
            "next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None}}))
    }
    /// Decode a selected mono SFX waveform into a fresh provenance bundle.
    /// Does not modify the live sfx cache or claim a SoundId mapping.
    pub fn audio_extract(&self, uri: &str, stream_id: u32) -> Result<Value> {
        let bytes = self.read(uri)?;
        let bank = awc::Bank::open(&bytes)?;
        let wav = bank.wav(stream_id)?;
        self.export(uri, &wav, "audio.wav", "AWC mono PCM16/IMA decoded to PCM16 WAV; no trimming or resampling",
            json!({"stream_id":stream_id,"source_sha256":hash(&bytes)}))
    }

    /// List movie tags without executing ActionScript or rendering anything.
    pub fn movie_list(&self, uri: &str, offset: usize, limit: usize) -> Result<Value> {
        if limit == 0 || limit > MAX_LIST { return Err("list limit must be 1..512".into()); }
        let bytes = self.read(uri)?;
        let movie = gfx::Movie::open(&bytes)?;
        let total = movie.tags.len();
        let tags: Vec<_> = movie.tags.iter().skip(offset).take(limit).collect();
        Ok(json!({"uri":uri,"source_sha256":hash(&bytes),"version":movie.version,"scaleform":movie.scaleform,
            "frames":movie.frames,"frame_rate_fixed8":movie.frame_rate_fixed8,"tags":tags,"total":total,
            "next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None}}))
    }
    /// Export an embedded lossless bitmap, not a movie screenshot/vector renderer.
    pub fn movie_extract_bitmap(&self, uri: &str, character_id: u16) -> Result<Value> {
        let bytes = self.read(uri)?;
        let movie = gfx::Movie::open(&bytes)?;
        let bitmap = movie.bitmap(character_id)?;
        let png = bitmap.png()?;
        self.export(uri, &png, "image.png", "SWF/GFX lossless XRGB/ARGB32 bitmap to straight-alpha RGBA8 PNG",
            json!({"character_id":character_id,"width":bitmap.width,"height":bitmap.height,"source_sha256":hash(&bytes)}))
    }

    /// Small JSON-RPC 2.0 subset over stdio: one request and response per line.
    /// No notifications, batches, network listener, arbitrary shell or URL access.
    pub fn dispatch(&self, request: Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if request.get("jsonrpc") != Some(&json!("2.0")) || request.get("id").is_none()
            || request.get("method").and_then(Value::as_str).is_none()
            || (!id.is_null() && !id.is_number() && !id.is_string()) {
            return json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid request"}});
        }
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let method = request["method"].as_str().unwrap();
        let invalid = || json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"invalid params: use typed relative paths, nonnegative offset and page limit 1..512"}});
        if !params.is_object()
            || params.get("path").is_some_and(|v| !v.is_string())
            || params.get("uri").is_some_and(|v| !v.is_string())
            || params.get("offset").is_some_and(|v| v.as_u64().is_none_or(|n|usize::try_from(n).is_err()))
            || params.get("limit").is_some_and(|v| v.as_u64().is_none_or(|n|n==0 || n>MAX_LIST as u64))
            || params.get("stream_id").is_some_and(|v| v.as_u64().is_none_or(|n| n>0x1fffffff))
            || (method=="audio.extract" && params.get("stream_id").is_none())
            || params.get("character_id").is_some_and(|v| v.as_u64().is_none_or(|n| n>u16::MAX as u64))
            || (method=="movie.extract_bitmap" && params.get("character_id").is_none())
            || (matches!(method,"resources.inspect"|"resources.extract"|"audio.list"|"audio.extract"|"movie.list"|"movie.extract_bitmap") && params.get("uri").and_then(Value::as_str).is_none_or(str::is_empty)) {
            return invalid();
        }
        let path = params.get("path").and_then(Value::as_str).unwrap_or("");
        let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "api.capabilities" => Ok(json!({"api_version":API_VERSION,"source_access":"read-only","methods":["api.capabilities","resources.list","resources.inspect","resources.extract","audio.list","audio.extract","movie.list","movie.extract_bitmap"],
                "limits":{"decoded_resource_bytes":MAX_BYTES,"rpf_table_bytes":16*1024*1024,"list_page_entries":MAX_LIST},
                "supported":["loose-files","rpf7-open","rpf7-aes-toc","rpf7-aes-binary-entry","rpf7-binary-deflate","awc-metadata","awc-mono-pcm16","awc-mono-ima","gfx-swf-zlib","gfx-swf-tag-inventory","gfx-swf-lossless-argb32-png"],
                "not_implemented":["pc-ng","nested-rpf","rsc7-decoding","awc-payload-cipher","awc-streamed-audio","awc-web-opus","soundset-to-waveform-mapping","font-conversion","swf-jpeg-palette-rgb15","swf-external-textures","swf-vector-rendering","native-resolution-hud"],"key_available":self.aes_key.is_some()})),
            "resources.list" => self.list(path, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "audio.list" => self.audio_list(uri, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "audio.extract" => self.audio_extract(uri, params["stream_id"].as_u64().unwrap() as u32),
            "movie.list" => self.movie_list(uri, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "movie.extract_bitmap" => self.movie_extract_bitmap(uri, params["character_id"].as_u64().unwrap() as u16),
            "resources.inspect" => self.inspect(uri),
            "resources.extract" => self.extract(uri),
            _ => return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown method"}}),
        };
        match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}}),
        }
    }
}
