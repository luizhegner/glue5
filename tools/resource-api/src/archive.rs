//! Bounded RPF7 metadata and binary-entry extraction. No keys are embedded.
use aes::{Aes256, cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray}};
use flate2::read::DeflateDecoder;
use serde::Serialize;
use std::{collections::HashSet, fs::File, io::{Read, Seek, SeekFrom}};

pub type Result<T> = std::result::Result<T, String>;
const TABLE_LIMIT: usize = 16 * 1024 * 1024;
const DIRECTORY: u32 = 0x7fffff00;

fn u32_at(data: &[u8], at: usize) -> u32 { u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) }
fn u24(data: &[u8]) -> u32 { u32::from_le_bytes([data[0], data[1], data[2], 0]) }
fn io(error: std::io::Error) -> String { error.to_string() }

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub stored_bytes: u64,
    pub decoded_bytes: u64,
    pub kind: &'static str,
    pub encrypted: bool,
    pub compressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_flags: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graphics_flags: Option<u32>,
    #[serde(skip)]
    offset: u64,
}

pub struct Archive<R=File> {
    file: R,
    aes_key: Option<[u8; 32]>,
    pub entries: Vec<Entry>,
    pub name_shift: u32,
    pub encryption: &'static str,
}

impl Archive<File> {
    pub fn open(file: File, key: Option<&[u8; 32]>) -> Result<Self> {
        let size = file.metadata().map_err(io)?.len();
        Self::open_reader(file,size,key)
    }
}

impl<R:Read+Seek> Archive<R> {
    /// Also supports a bounded source backed by GLUE5 streaming ranges;
    /// callers must not open a sparse cache file as if it were complete.
    pub fn open_reader(mut file:R,size:u64,key:Option<&[u8;32]>)->Result<Self> {
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut header = [0; 16];
        file.read_exact(&mut header).map_err(io)?;
        if &header[..4] != b"7FPR" { return Err("unsupported archive: expected RPF7".into()); }
        let count = u32_at(&header, 4) as usize;
        let word = u32_at(&header, 8);
        let shift = (word >> 28) & 7;
        let names_len = (word & 0x0fffffff) as usize;
        let table_len = count.checked_mul(16).and_then(|n| n.checked_add(names_len)).ok_or("table overflow")?;
        if !(2..=100_000).contains(&count) || names_len == 0 || table_len > TABLE_LIMIT || table_len as u64 + 16 > size {
            return Err("invalid or excessive archive table".into());
        }
        let encryption = match u32_at(&header, 12) {
            0x4e45504f => "open",
            0x0ffffff9 => "aes",
            _ => return Err("unsupported archive encryption: PC NG requires a future provider; do not rewrite the archive".into()),
        };
        let mut table = vec![0; table_len];
        file.read_exact(&mut table).map_err(io)?;
        if encryption == "aes" {
            let cipher = Aes256::new_from_slice(key.ok_or("AES archive requires an explicitly supplied 32-byte key")?).map_err(|_| "invalid key")?;
            for block in table.chunks_exact_mut(16) {
                cipher.decrypt_block(GenericArray::from_mut_slice(block));
            }
        }
        let raw = &table[..count * 16];
        let names = &table[count * 16..];
        let mut entries = Vec::new();
        let mut visited = HashSet::new();
        let mut paths = HashSet::new();
        fn walk(index: usize, parent: &str, depth: usize, raw: &[u8], names: &[u8], shift: u32,
                size: u64, data_start: u64, visited: &mut HashSet<usize>, paths: &mut HashSet<String>, out: &mut Vec<Entry>) -> Result<()> {
            if depth > 32 || index >= raw.len() / 16 || !visited.insert(index) { return Err("cyclic, overlapping or excessive directory tree".into()); }
            let e = &raw[index * 16..index * 16 + 16];
            let directory = u32_at(e, 4) == DIRECTORY;
            let name_index = if directory { u32_at(e, 0) } else { u16::from_le_bytes([e[0], e[1]]) as u32 };
            let at = (name_index as usize).checked_shl(shift).ok_or("name offset overflow")?;
            let tail = names.get(at..).ok_or("name outside table; check key and name shift")?;
            if at > 0 && names[at - 1] != 0 { return Err("name offset starts inside another name; check declared shift".into()); }
            let end = tail.iter().position(|&b| b == 0).ok_or("unterminated archive name")?;
            let name = std::str::from_utf8(&tail[..end]).map_err(|_| "invalid UTF-8 archive name")?;
            if index == 0 {
                if !directory || !name.is_empty() { return Err("archive must have an unnamed root directory".into()); }
            } else if !super::safe_name(name) { return Err("unsafe archive entry name".into()); }
            let path = if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") };
            if index != 0 && !paths.insert(path.to_ascii_lowercase()) { return Err("ambiguous duplicate archive path".into()); }
            if directory {
                let first = u32_at(e, 8) as usize;
                let children = u32_at(e, 12) as usize;
                let stop = first.checked_add(children).ok_or("directory overflow")?;
                if stop > raw.len() / 16 { return Err("directory exceeds table".into()); }
                for child in first..stop { walk(child, &path, depth + 1, raw, names, shift, size, data_start, visited, paths, out)?; }
            } else {
                let sector = u24(&e[5..8]);
                let resource = sector & 0x800000 != 0;
                let stored = u24(&e[2..5]) as u64;
                let decoded = if resource { super::rsc7::page_bytes(u32_at(e, 8)) + super::rsc7::page_bytes(u32_at(e, 12)) }
                    else { u32_at(e, 8) as u64 };
                let stored = if stored == 0 && !resource { decoded } else { stored };
                let offset = (sector & 0x7fffff) as u64 * 512;
                if offset < data_start || offset.checked_add(stored).is_none_or(|end| end > size) { return Err("archive entry outside file".into()); }
                out.push(Entry { path, stored_bytes: stored, decoded_bytes: decoded,
                    kind: if resource { "rsc7-resource" } else { "binary" },
                    encrypted: !resource && u32_at(e, 12) != 0, compressed: !resource && u24(&e[2..5]) != 0,
                    system_flags: resource.then_some(u32_at(e,8)),graphics_flags: resource.then_some(u32_at(e,12)),offset });
            }
            Ok(())
        }
        walk(0, "", 0, raw, names, shift, size, table_len as u64 + 16, &mut visited, &mut paths, &mut entries)?;
        if visited.len() != count { return Err("archive contains unreachable table entries".into()); }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { file, aes_key: key.copied(), entries, name_shift: shift, encryption })
    }

    pub fn read(&mut self, path: &str, limit: u64) -> Result<Vec<u8>> {
        let entry = self.entries.iter().find(|e| e.path.eq_ignore_ascii_case(path)).ok_or("archive entry not found")?;
        if entry.kind != "binary" && (entry.path.to_ascii_lowercase().ends_with(".ysc") || entry.stored_bytes == 0xffffff) {
            return Err("encrypted scripts/extended RSC7 headers are not implemented".into());
        }
        if entry.stored_bytes > limit || entry.decoded_bytes > limit { return Err("resource exceeds extraction limit".into()); }
        self.file.seek(SeekFrom::Start(entry.offset)).map_err(io)?;
        let mut stored = vec![0; entry.stored_bytes as usize];
        self.file.read_exact(&mut stored).map_err(io)?;
        if entry.kind == "rsc7-resource" {
            // Preserve the on-disk envelope; resource decoding is a separate API.
            // Resource TOC words are page flags, NOT binary-entry cipher fields.
            if stored.len() < 16 || &stored[..4] != b"RSC7"
                || Some(u32_at(&stored,8)) != entry.system_flags
                || Some(u32_at(&stored,12)) != entry.graphics_flags {
                return Err("RSC7 envelope and archive page flags disagree".into());
            }
            return Ok(stored);
        }
        if entry.encrypted {
            if self.encryption != "aes" { return Err("unsupported per-entry cipher for this archive".into()); }
            let key = self.aes_key.as_ref().ok_or("entry requires an explicitly supplied AES key")?;
            let cipher = Aes256::new_from_slice(key).map_err(|_| "invalid AES key")?;
            // RPF encrypts complete ECB blocks; any trailing partial block
            // remains plaintext. Decrypt before bounded deflate expansion.
            for block in stored.chunks_exact_mut(16) { cipher.decrypt_block(GenericArray::from_mut_slice(block)); }
        }
        if !entry.compressed { return Ok(stored); }
        let mut decoded = Vec::new();
        DeflateDecoder::new(&stored[..]).take(limit + 1).read_to_end(&mut decoded).map_err(io)?;
        if decoded.len() as u64 != entry.decoded_bytes || decoded.len() as u64 > limit { return Err("decompressed size mismatch or limit exceeded".into()); }
        Ok(decoded)
    }
}

