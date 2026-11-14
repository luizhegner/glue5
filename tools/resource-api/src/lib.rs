//! Local, read-only source access for GLUE 5 tooling. Public Rust API plus a
//! versioned JSON-lines adapter; no HTTP listener or bundled proprietary data.
mod archive;
mod dlc;
pub use dlc::DlcPack;
pub mod sources;
mod harvest;
pub use harvest::{Item as HarvestItem, Selection as HarvestSelection};
pub use harvest::{Request as HarvestRequest, Candidate as HarvestCandidate};
pub mod range;
pub mod awc;
pub mod rel;
pub mod web_opus;
pub mod gfx;
pub mod rsc7;
pub mod ytd;
pub use archive::{Archive, Entry, Result};
pub use archive::NgDecryptor;
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
fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(bytes) { write!(&mut out, "{byte:02x}").unwrap(); }
    out
}
fn io(error: std::io::Error) -> String { error.to_string() }

pub struct Resources {
    root: PathBuf,
    cache: PathBuf,
    aes_key: Option<[u8; 32]>,
    ng: Option<std::sync::Arc<dyn NgDecryptor>>,
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
        Ok(Self { root, cache, aes_key, ng: None })
    }
    pub fn with_ng_decryptor(mut self, decryptor: std::sync::Arc<dyn NgDecryptor>) -> Self {
        self.ng = Some(decryptor);
        self
    }
    fn file(&self, path: &str) -> Result<PathBuf> {
        relative(path)?;
        let file = self.root.join(path).canonicalize().map_err(io)?;
        if !file.starts_with(&self.root) { return Err("path or link escapes the approved source".into()); }
        Ok(file)
    }
    fn archive(&self, path: &str) -> Result<Archive<Box<dyn range::ReadSeek>>> {
        let parts: Vec<_> = path.split("::").collect();
        if parts.len() > 8 { return Err("nested archive depth exceeds 8".into()); }
        for part in &parts {
            relative(part)?;
            if part.is_empty() { return Err("empty archive path".into()); }
        }
        let file = File::open(self.file(parts[0])?).map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        let reader: Box<dyn range::ReadSeek> = Box::new(file);
        let mut archive = Archive::open_reader_with_ng(reader, size, self.aes_key.as_ref(), parts[0], self.ng.clone())?;
        for part in &parts[1..] { archive = archive.into_nested(part)?; }
        Ok(archive)
    }

    /// URI grammar: relative loose path, or archive.rpf::relative/entry.gfx.
    /// Nested archives use additional `::` components, with a depth cap of 8.
    pub fn read(&self, uri: &str) -> Result<Vec<u8>> {
        if let Some((archive, entry)) = uri.rsplit_once("::") {
            relative(entry)?;
            if entry.is_empty() { return Err("empty entry URI".into()); }
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
        let file = self.file(path.split("::").next().unwrap())?;
        let mut entries: Vec<Value>;
        let mut metadata = json!({});
        if !path.contains("::") && file.is_dir() {
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
            "music":if bank.streamed { Some(bank.music()?) } else { None },
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

    /// Resolve only a verified direct reference; never scan for a similar name
    /// or mistake an event hash for an AWC waveform identifier.
    pub fn audio_resolve(&self, uri: &str, soundset: &str, event: &str) -> Result<Value> {
        let bytes = self.read(uri)?;
        let reference = rel::Sounds::open(&bytes)?.resolve(soundset, event)?;
        Ok(json!({"uri":uri,"source_sha256":hash(&bytes),"reference":reference}))
    }

    /// End-to-end direct SFX export: verify the REL container binding before
    /// decoding its waveform. Both sources are inside the approved game root.
    /// Bank URI must retain its original archive/entry (or loose pack) name.
    pub fn audio_extract_event(&self, uri: &str, bank_uri: &str, soundset: &str, event: &str) -> Result<Value> {
        let rel_bytes = self.read(uri)?;
        let reference = rel::Sounds::open(&rel_bytes)?.resolve(soundset, event)?;
        let bank_name = rel::bank_name(bank_uri)?;
        if rel::name_hash(&bank_name)? != reference.container_hash {
            return Err("selected AWC does not match the REL container reference".into());
        }
        let bank_bytes = self.read(bank_uri)?;
        let bank = awc::Bank::open(&bank_bytes)?;
        let wav = bank.wav(reference.stream_id)?;
        self.export(bank_uri, &wav, "audio.wav", "REL direct SoundSet reference + AWC PCM16/IMA to WAV; native graph processing not rendered",
            json!({"decoder":"rel-dat54-direct-awc-sfx/1","soundset":soundset,"event":event,"reference":reference,
                "rel_uri":uri,"rel_sha256":hash(&rel_bytes),"source_sha256":hash(&bank_bytes),"container_name":bank_name}))
    }

    /// Export the explicit channel selection, not an implicit mix of music stems.
    pub fn audio_extract_music(&self, uri: &str, channels: &[usize]) -> Result<Value> {
        let bytes = self.read(uri)?;
        let bank = awc::Bank::open(&bytes)?;
        let music = bank.music()?;
        let wav = bank.music_wav(channels)?;
        self.export(uri, &wav, "audio.wav", "AWC blocked IMA to PCM16 WAV; explicit channel order, no mixing/resampling/looping",
            json!({"decoder":"awc-music-ima-block18/1","channels":channels,"music":music,"source_sha256":hash(&bytes)}))
    }

    /// The owner-selected Web build's mono waveform, repackaged losslessly
    /// for an ordinary Ogg Opus decoder. Does not invoke an Opus encoder.
    pub fn audio_extract_web_opus(&self, uri: &str, stream_id: u32, build: &str) -> Result<Value> {
        let bytes = self.read(uri)?;
        let bank = awc::Bank::open(&bytes)?;
        let ogg = bank.web_opus_ogg(stream_id, build)?;
        self.export(uri, &ogg, "audio.ogg", "Web u8-framed Opus to Ogg Opus; packets unchanged, mono 48kHz, no added preskip or transcoding",
            json!({"decoder":"awc-web-opus-u8-60ms-ogg/1","build":build,"stream_id":stream_id,"source_sha256":hash(&bytes)}))
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

    /// Validate/decompress RSC7 page spaces without interpreting texture objects.
    pub fn rsc7_inspect(&self, uri: &str) -> Result<Value> {
        let bytes = self.read(uri)?;
        let resource = rsc7::Resource::open(&bytes)?;
        Ok(json!({"uri":uri,"source_sha256":hash(&bytes),"version":resource.version,
            "system_flags":resource.system_flags,"graphics_flags":resource.graphics_flags,
            "system_bytes":resource.system_bytes,"graphics_bytes":resource.graphics_bytes,
            "system_sha256":hash(resource.system()),"graphics_sha256":hash(resource.graphics())}))
    }

    /// Inventory legacy texture objects, with explicit per-entry decode limits.
    pub fn textures_list(&self, uri: &str, offset: usize, limit: usize) -> Result<Value> {
        if limit == 0 || limit > MAX_LIST { return Err("list limit must be 1..512".into()); }
        let bytes = self.read(uri)?;
        let dictionary = ytd::Dictionary::open(&bytes)?;
        let total = dictionary.textures.len();
        let textures: Vec<_> = dictionary.textures.iter().skip(offset).take(limit).collect();
        Ok(json!({"uri":uri,"source_sha256":hash(&bytes),"layout":"ytd-legacy-pc64-v13",
            "textures":textures,"total":total,"offset":offset,
            "next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None}}))
    }

    /// Base mip only; no composition, parallax inference or live-cache writes.
    pub fn textures_extract(&self, uri: &str, texture_index: usize) -> Result<Value> {
        let bytes = self.read(uri)?;
        let dictionary = ytd::Dictionary::open(&bytes)?;
        let bitmap = dictionary.bitmap(texture_index)?;
        let texture = &dictionary.textures[texture_index];
        self.export(uri, &bitmap.png()?, "image.png", "Legacy YTD base mip to RGBA8 PNG; stored channels preserved; no gamma conversion or resizing",
            json!({"decoder":"ytd-legacy-pc64-v13-base/1","texture_index":texture_index,"name":texture.name,
                "name_hash":texture.name_hash,"width":bitmap.width,"height":bitmap.height,"mip_level":0,
                "format":texture.format,"format_code":texture.format_code,"source_sha256":hash(&bytes)}))
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
            || (matches!(method,"audio.extract"|"audio.extract_web_opus") && params.get("stream_id").is_none())
            || (method=="audio.extract_web_opus" && params.get("build").and_then(Value::as_str) != Some(web_opus::BUILD))
            || (matches!(method,"audio.resolve"|"audio.extract_event") && ["soundset","event"].iter().any(|key| params.get(*key).and_then(Value::as_str).is_none_or(str::is_empty)))
            || (method=="audio.extract_event" && params.get("bank_uri").and_then(Value::as_str).is_none_or(str::is_empty))
            || (method=="audio.extract_music" && params.get("channels").and_then(Value::as_array).is_none_or(|items|
                items.is_empty() || items.len()>2 || items.iter().any(|v| v.as_u64().is_none_or(|n| n>=32))))
            || params.get("character_id").is_some_and(|v| v.as_u64().is_none_or(|n| n>u16::MAX as u64))
            || (method=="movie.extract_bitmap" && params.get("character_id").is_none())
            || params.get("texture_index").is_some_and(|v| v.as_u64().is_none_or(|n|usize::try_from(n).is_err()))
            || (method=="textures.extract" && params.get("texture_index").is_none())
            || (matches!(method,"resources.inspect"|"resources.extract"|"audio.list"|"audio.extract"|"audio.extract_web_opus"|"audio.resolve"|"audio.extract_event"|"audio.extract_music"|"movie.list"|"movie.extract_bitmap"|"rsc7.inspect"|"textures.list"|"textures.extract") && params.get("uri").and_then(Value::as_str).is_none_or(str::is_empty)) {
            return invalid();
        }
        let path = params.get("path").and_then(Value::as_str).unwrap_or("");
        let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "api.capabilities" => Ok(json!({"api_version":API_VERSION,"source_access":"read-only","methods":["api.capabilities","resources.list","dlc.discover","dlc.manifest","resources.inspect","resources.extract","harvest.plan","harvest.apply","audio.list","audio.extract","audio.extract_web_opus","audio.resolve","audio.extract_event","audio.extract_music","movie.list","movie.extract_bitmap","rsc7.inspect","textures.list","textures.extract"],
                "limits":{"decoded_resource_bytes":MAX_BYTES,"rpf_table_bytes":16*1024*1024,"list_page_entries":MAX_LIST,"archive_depth":8},
                "nested_rpf":{"stored_unencrypted_entry":true,"compressed_or_encrypted_entry":false,"access":"bounded-ranges"},
                "supported":["loose-files","rpf7-open","rpf7-aes-toc","rpf7-aes-binary-entry","rpf7-binary-deflate","awc-metadata","awc-mono-pcm16","awc-mono-ima","awc-music-ima-block18-explicit-channels","awc-web-8b0b5899ed-mono-opus-ogg-repack","rel-dat54-direct-soundset-reference","gfx-swf-zlib","gfx-swf-tag-inventory","gfx-swf-lossless-argb32-png","rsc7-deflate-envelope","rsc7-bounded-address-spaces","ytd-legacy-pc64-v13-inventory","ytd-base-mip-bgra-rgba-a8-l8-png","ytd-base-mip-bc1-bc2-bc3-png"],
                "not_implemented":["pc-ng","nested-rpf-compressed-encrypted-entry","rsc7-cipher-extended-header","ytd-gen9-console-volumes-cubemaps","ytd-bc4-bc5-bc7","ytd-mip-selection","awc-payload-cipher","awc-other-streamed-codecs-layouts","awc-web-opus-other-builds-streamed-layouts","rel-sound-graph-rendering","automatic-audio-import-activation","large-music-range-stream-export","font-conversion","swf-jpeg-palette-rgb15","swf-external-textures","swf-vector-rendering","native-resolution-hud"],"key_available":self.aes_key.is_some()})),
            "resources.list" => self.list(path, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "audio.list" => self.audio_list(uri, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "audio.extract" => self.audio_extract(uri, params["stream_id"].as_u64().unwrap() as u32),
            "audio.extract_web_opus" => self.audio_extract_web_opus(uri, params["stream_id"].as_u64().unwrap() as u32, params["build"].as_str().unwrap()),
            "audio.resolve" => self.audio_resolve(uri, params["soundset"].as_str().unwrap(), params["event"].as_str().unwrap()),
            "audio.extract_event" => self.audio_extract_event(uri, params["bank_uri"].as_str().unwrap(), params["soundset"].as_str().unwrap(), params["event"].as_str().unwrap()),
            "audio.extract_music" => self.audio_extract_music(uri, &params["channels"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>()),
            "movie.list" => self.movie_list(uri, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "movie.extract_bitmap" => self.movie_extract_bitmap(uri, params["character_id"].as_u64().unwrap() as u16),
            "rsc7.inspect" => self.rsc7_inspect(uri),
            "textures.list" => self.textures_list(uri, params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                params.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize),
            "textures.extract" => self.textures_extract(uri, params["texture_index"].as_u64().unwrap() as usize),
            "resources.inspect" => self.inspect(uri),
            "dlc.discover" => self.discover_dlc_packs().map(|packs| json!({"packs":packs})),
            "dlc.manifest" => self.dlc_manifest(if uri.is_empty() {
                "update/update.rpf::common/data/dlclist.xml"
            } else { uri }),
            "resources.extract" => self.extract(uri),
            "harvest.plan" | "harvest.apply" => {
                match serde_json::from_value::<Vec<HarvestItem>>(params.get("items").cloned().unwrap_or(Value::Null)) {
                    Ok(items) if method == "harvest.plan" => self.harvest_plan(&items),
                    Ok(items) => self.harvest_apply(&items),
                    Err(_) => Err("invalid harvest items: expected named assets with a typed selection".into()),
                }
            },
            _ => return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown method"}}),
        };
        match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}}),
        }
    }
}

