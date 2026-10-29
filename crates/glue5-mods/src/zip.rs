//! Zip files of mods, for the Studio's Export and Import: written with
//! deflate, read stored or deflated (what any zip tool makes). Only what a
//! mod is: a few small files, no folders above it, nothing outside it. The
//! application reads its GPU driver packages with it too ([`read_up_to`]).

use std::io::{Read, Write};

use flate2::Crc;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

/// What an imported zip may hold.
const MAX_FILES: usize = 256;
const MAX_SIZE: usize = 16 << 20;

fn crc(data: &[u8]) -> u32 {
    let mut c = Crc::new();
    c.update(data);
    c.sum()
}

/// `files` (path, contents) as a zip.
pub fn write(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let mut enc = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(data);
        let packed = enc.finish().unwrap_or_default();
        let (sum, offset) = (crc(data), out.len() as u32);
        // Local header: version 2.0, UTF-8 names (bit 11), deflate.
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0x0800u16.to_le_bytes());
            h.extend_from_slice(&8u16.to_le_bytes());
            h.extend_from_slice(&[0, 0, 0x21, 0]); // time 00:00, date 1980-01-01
            h.extend_from_slice(&sum.to_le_bytes());
            h.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&[0; 6]); // comment length, disk, internal attributes
                h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x04034b50, false));
        out.extend_from_slice(&packed);
        central.extend(header(0x02014b50, true));
    }
    let (start, size, count) = (out.len() as u32, central.len() as u32, files.len() as u16);
    out.extend(central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| "not a zip".into())
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "not a zip".into())
}

/// A mod's zip: its files (path, contents), see [`read_up_to`].
pub fn read(d: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    read_up_to(d, MAX_FILES, MAX_SIZE, "a mod")
}

/// A zip's files (path, contents), from its central directory; folders,
/// macOS's `__MACOSX` and hidden files left out. At most `max_files` files
/// of `max_size` bytes in all, else it is not `what`.
pub fn read_up_to(d: &[u8], max_files: usize, max_size: usize, what: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    // The end record, within the last 64 KiB (its comment).
    let from = d.len().saturating_sub(22 + 0xffff);
    let end = (from..d.len().saturating_sub(21)).rev().find(|&i| u32_at(d, i) == Ok(0x06054b50)).ok_or("not a zip")?;
    let count = u16_at(d, end + 10)? as usize;
    let mut at = u32_at(d, end + 16)? as usize;
    if count > max_files {
        return Err(format!("more than {max_files} files: not {what}"));
    }
    let mut files = Vec::new();
    let mut total = 0;
    for _ in 0..count {
        if u32_at(d, at)? != 0x02014b50 {
            return Err("a damaged zip".into());
        }
        let method = u16_at(d, at + 10)?;
        let (packed, size) = (u32_at(d, at + 20)? as usize, u32_at(d, at + 24)? as usize);
        let (name_len, extra, comment) = (u16_at(d, at + 28)? as usize, u16_at(d, at + 30)? as usize, u16_at(d, at + 32)? as usize);
        let local = u32_at(d, at + 42)? as usize;
        let name = String::from_utf8_lossy(d.get(at + 46..at + 46 + name_len).ok_or("a damaged zip")?).replace('\\', "/");
        at += 46 + name_len + extra + comment;
        if name.ends_with('/') || name.starts_with("__MACOSX/") || name.split('/').any(|p| p.starts_with('.')) {
            continue;
        }
        total += size;
        if total > max_size {
            return Err(format!("more than {} MB: not {what}", max_size >> 20));
        }
        if u32_at(d, local)? != 0x04034b50 {
            return Err("a damaged zip".into());
        }
        let data_at = local + 30 + u16_at(d, local + 26)? as usize + u16_at(d, local + 28)? as usize;
        let raw = d.get(data_at..data_at + packed).ok_or("a damaged zip")?;
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                DeflateDecoder::new(raw).take(size as u64 + 1).read_to_end(&mut out).map_err(|e| format!("{name}: {e}"))?;
                out
            }
            m => return Err(format!("{name}: compressed in a way GLUE 5 does not read ({m})")),
        };
        if data.len() != size {
            return Err(format!("{name}: damaged"));
        }
        files.push((name, data));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let files = vec![("trainer/main.luau".to_string(), b"print('hi')\n".repeat(50)), ("trainer/mod.json".to_string(), b"{}".to_vec())];
        let zip = super::write(&files);
        assert_eq!(super::read(&zip).unwrap(), files);
    }
}
