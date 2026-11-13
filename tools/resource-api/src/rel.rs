//! Read-only DAT54 sound references. Layout checked against CodeWalker's
//! RelFile.cs. Resolving a reference is NOT rendering its processing graph.
use crate::{Result, MAX_BYTES};
use serde::Serialize;
use std::collections::HashMap;

fn range(data: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    data.get(at..at.checked_add(len).ok_or("REL range overflow")?)
        .ok_or_else(|| "truncated REL range".into())
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(range(data, at, 4)?.try_into().unwrap()))
}

/// GTA's case-insensitive ASCII Jenkins identifier; not a file-content hash.
pub fn name_hash(name: &str) -> Result<u32> {
    if name.is_empty() || name.len() > 255 || !name.is_ascii() || name.bytes().any(|c| c.is_ascii_control()) {
        return Err("sound identifiers must be 1..255 printable ASCII bytes".into());
    }
    let mut h = 0u32;
    for c in name.bytes() {
        h = h.wrapping_add(c.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    Ok(h.wrapping_add(h << 15))
}

/// Original container identity, not an export filename. For example,
/// x64/audio/sfx/RESIDENT.rpf::frontend.awc -> resident/frontend.
pub fn bank_name(uri: &str) -> Result<String> {
    let uri = uri.to_ascii_lowercase();
    let (pack, entry) = if let Some((archive, entry)) = uri.split_once("::") {
        crate::relative(archive)?; crate::relative(entry)?;
        let archive = archive.rsplit('/').next().unwrap_or("");
        (archive.strip_suffix(".rpf").ok_or("bank archive must retain its RPF name")?, entry)
    } else {
        crate::relative(&uri)?;
        let (parent, entry) = uri.rsplit_once('/').ok_or("loose bank URI must include its original pack directory")?;
        (parent.rsplit('/').next().unwrap_or(""), entry)
    };
    let wave = entry.strip_suffix(".awc").ok_or("bank entry must retain its AWC name")?;
    if pack.is_empty() || wave.is_empty() { return Err("empty AWC container identity".into()); }
    Ok(format!("{pack}/{wave}"))
}

#[derive(Debug, Serialize)]
pub struct WaveReference {
    pub soundset_hash: u32,
    pub event_hash: u32,
    pub child_sound_hash: u32,
    pub container_hash: u32,
    pub file_hash: u32,
    /// AWC stores only the low 29 bits of the full waveform-name hash.
    pub stream_id: u32,
    pub wave_slot: u8,
    /// Native engine processing remains authoritative (pitch, envelope, etc.).
    pub rendering: &'static str,
}

pub struct Sounds<'a> {
    records: HashMap<u32, &'a [u8]>,
}
impl<'a> Sounds<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_BYTES || u32_at(bytes, 0)? != 54 {
            return Err("expected bounded DAT54 REL".into());
        }
        let data = range(bytes, 8, u32_at(bytes, 4)? as usize)?;
        // First data word is the DAT54 version, not an indexed sound.
        range(data, 0, 4)?;
        let names_at = 8 + data.len();
        let names_len = u32_at(bytes, names_at)? as usize;
        let names = range(bytes, names_at + 4, names_len)?;
        let name_count = u32_at(names, 0)? as usize;
        if name_count > 100_000 { return Err("excessive REL names".into()); }
        range(names, 4, name_count * 4)?;
        let index_at = names_at + 4 + names_len;
        let count = u32_at(bytes, index_at)? as usize;
        if count > 100_000 { return Err("excessive REL index".into()); }
        let index = range(bytes, index_at + 4, count * 12)?;
        let mut records = HashMap::with_capacity(count);
        let mut extents = Vec::with_capacity(count);
        for entry in index.chunks_exact(12) {
            let hash = u32_at(entry, 0)?;
            let offset = u32_at(entry, 4)? as usize;
            let len = u32_at(entry, 8)? as usize;
            if offset < 4 || len < 5 { return Err("invalid REL record extent".into()); }
            let record = range(data, offset, len)?;
            if records.insert(hash, record).is_some() { return Err("ambiguous duplicate REL hash".into()); }
            extents.push((offset, offset + len));
        }
        extents.sort_unstable();
        if extents.windows(2).any(|w| w[0].1 > w[1].0) { return Err("overlapping REL records".into()); }
        // Validate the two reference-offset tables, without interpreting them
        // as sound records or trusting them to supply the selected waveform.
        let mut at = index_at + 4 + index.len();
        for _ in 0..2 {
            let count = u32_at(bytes, at)? as usize;
            if count > 1_000_000 { return Err("excessive REL reference table".into()); }
            at += 4;
            for offset in range(bytes, at, count * 4)?.chunks_exact(4) {
                range(bytes, u32::from_le_bytes(offset.try_into().unwrap()) as usize, 4)?;
            }
            at += count * 4;
        }
        if at != bytes.len() { return Err("unaccounted REL trailing data".into()); }
        Ok(Self { records })
    }

    fn body(&self, hash: u32, kind: u8) -> Result<&'a [u8]> {
        let data = self.records.get(&hash).ok_or("REL sound reference not found")?;
        if data[0] != kind {
            return Err(format!("REL sound type {} requires graph rendering; expected {kind}, no waveform guessed", data[0]));
        }
        // Optional header fields are gated by flags. An AA sentinel suppresses
        // its entire byte of flags, not individual alternating flag bits.
        const WIDTH: [usize; 32] = [4,2,2,2,2,2,2,2,2,2,4,4,2,2,2,4,2,2,2,2,4,2,1,1,1,4,4,2,2,2,2,2];
        let flags = u32_at(data, 1)?;
        let mut at = 5;
        for (bit, width) in WIDTH.iter().enumerate() {
            if ((flags >> ((bit / 8) * 8)) & 255) != 0xaa && flags & (1 << bit) != 0 {
                at += width;
            }
        }
        range(data, at, data.len().checked_sub(at).ok_or("truncated REL sound header")?)
    }

    /// Direct SoundSet -> SimpleSound only. Complex/random/layered references
    /// fail explicitly: choosing a random descendant would change the sound.
    pub fn resolve(&self, soundset: &str, event: &str) -> Result<WaveReference> {
        let soundset_hash = name_hash(soundset)?;
        let event_hash = name_hash(event)?;
        let set = self.body(soundset_hash, 32)?;
        let count = u32_at(set, 0)? as usize;
        if count > 4096 || set.len() != 4 + count * 8 { return Err("invalid soundset entries".into()); }
        let mut selected = None;
        let mut names = std::collections::HashSet::new();
        for item in set[4..].chunks_exact(8) {
            let event = u32_at(item, 0)?;
            if !names.insert(event) { return Err("ambiguous duplicate soundset event".into()); }
            if event == event_hash { selected = Some(u32_at(item, 4)?); }
        }
        let child_sound_hash = selected.ok_or("event not found in selected soundset")?;
        let simple = self.body(child_sound_hash, 12)?;
        if simple.len() != 9 { return Err("unsupported SimpleSound layout".into()); }
        let file_hash = u32_at(simple, 4)?;
        Ok(WaveReference { soundset_hash, event_hash, child_sound_hash,
            container_hash: u32_at(simple, 0)?, file_hash, stream_id: file_hash & 0x1fffffff,
            wave_slot: simple[8], rendering: "reference-only; engine sound graph not rendered" })
    }
}

