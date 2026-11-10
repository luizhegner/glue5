//! Bounded AWC bank reader and mono PCM/IMA -> WAV conversion.
//! Container/IMA layout references (not vendored code):
//! https://github.com/vgmstream/vgmstream/blob/master/src/meta/awc.c
//! https://github.com/vgmstream/vgmstream/blob/master/src/coding/ima_decoder.c
//! No game bytes, sound-name guesses or keys are embedded.
use crate::{Result, MAX_BYTES};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    pub tag: u8,
    pub offset: usize,
    pub bytes: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct Stream {
    /// Container's 29-bit waveform identifier, NOT a script SoundId.
    pub id: u32,
    pub chunks: Vec<Chunk>,
    pub samples: Option<u32>,
    pub sample_rate: Option<u16>,
    pub codec: Option<u8>,
}
pub struct Bank<'a> {
    bytes: &'a [u8],
    pub big_endian: bool,
    pub encrypted: bool,
    pub streamed: bool,
    pub streams: Vec<Stream>,
}
fn range(data: &[u8], at: usize, size: usize) -> Result<&[u8]> {
    data.get(at..at.checked_add(size).ok_or("AWC range overflow")?).ok_or_else(|| "truncated AWC range".into())
}
fn word(data: &[u8], at: usize, size: usize, be: bool) -> Result<u64> {
    let bytes = range(data, at, size)?;
    Ok(if be { bytes.iter().fold(0, |n, &b| (n << 8) | b as u64) }
        else { bytes.iter().rev().fold(0, |n, &b| (n << 8) | b as u64) })
}
impl<'a> Bank<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_BYTES { return Err("AWC input exceeds byte budget".into()); }
        let be = match bytes.get(..4) {
            Some(b"ADAT") => false, Some(b"TADA") => true,
            _ => return Err("expected ADAT/TADA AWC".into()),
        };
        let flags = word(bytes, 4, 4, be)? as u32;
        if flags & 0xff00ffff != 0xff000001 || flags & 0x00f00000 != 0 {
            return Err("unsupported AWC version/flags".into());
        }
        let count = word(bytes, 8, 4, be)? as usize;
        if !(1..=4096).contains(&count) { return Err("AWC stream count outside budget".into()); }
        let header_end = word(bytes, 12, 4, be)? as usize;
        let starts = if flags & 0x10000 != 0 { count * 2 } else { 0 };
        let ids_at = 16 + starts;
        let tags_at = ids_at + count * 4;
        range(bytes, 0, tags_at)?;
        let mut streams = Vec::with_capacity(count);
        let mut seen = HashSet::new();
        let mut tags = 0usize;
        let mut chunk_counts = Vec::with_capacity(count);
        for i in 0..count {
            let info = word(bytes, ids_at + i * 4, 4, be)? as u32;
            let id = info & 0x1fffffff;
            if !seen.insert(id) { return Err("ambiguous duplicate AWC stream ID".into()); }
            if starts != 0 && word(bytes, 16 + i * 2, 2, be)? as usize != tags {
                return Err("AWC tag-start table disagrees with streams".into());
            }
            let count = (info >> 29) as usize;
            streams.push(Stream { id, chunks: Vec::with_capacity(count), samples: None, sample_rate: None, codec: None });
            tags += count;
            chunk_counts.push(count);
        }
        let table_end = tags_at + tags * 8;
        if header_end < table_end || header_end > bytes.len() { return Err("invalid AWC header boundary".into()); }
        range(bytes, tags_at, tags * 8)?;
        let mut cursor = tags_at;
        for (stream, count) in streams.iter_mut().zip(chunk_counts) {
            let mut types = HashSet::new();
            for _ in 0..count {
                let tag = word(bytes, cursor, 8, be)?;
                cursor += 8;
                let kind = (tag >> 56) as u8;
                let offset = (tag & 0x0fffffff) as usize;
                let size = ((tag >> 28) & 0x0fffffff) as usize;
                if !types.insert(kind) { return Err("ambiguous duplicate AWC chunk type".into()); }
                if offset < header_end { return Err("AWC chunk overlaps its header".into()); }
                let data = range(bytes, offset, size)?;
                if kind == 0xfa {
                    range(data, 0, 20)?;
                    stream.samples = Some(word(data, 0, 4, be)? as u32);
                    stream.sample_rate = Some(word(data, 8, 2, be)? as u16);
                    stream.codec = Some(data[19]);
                }
                stream.chunks.push(Chunk { tag: kind, offset, bytes: size });
            }
        }
        let streamed = streams[0].id == 0 || flags & 0x40000 != 0;
        Ok(Self { bytes, big_endian: be, encrypted: flags & 0x80000 != 0, streamed, streams })
    }
    pub fn wav(&self, id: u32) -> Result<Vec<u8>> {
        if self.encrypted { return Err("AWC payload cipher is not implemented (distinct from RPF AES)".into()); }
        if self.streamed { return Err("streamed/multichannel AWC decoding is not implemented".into()); }
        let stream = self.streams.iter().find(|s| s.id == id).ok_or("AWC stream ID not found")?;
        let samples = stream.samples.ok_or("AWC stream lacks SFX format chunk")? as usize;
        let rate = stream.sample_rate.ok_or("AWC stream lacks sample rate")?;
        if samples == 0 || samples as u64 * 2 + 44 > MAX_BYTES || rate < 1000 {
            return Err("invalid or excessive AWC sample count/rate".into());
        }
        let data = stream.chunks.iter().find(|c| c.tag == 0x55).ok_or("AWC stream lacks data chunk")?;
        let bytes = range(self.bytes, data.offset, data.bytes)?;
        let pcm = match stream.codec {
            Some(0 | 1) => {
                let raw = range(bytes, 0, samples * 2)?;
                let mut pcm = raw.to_vec();
                if self.big_endian { for pair in pcm.chunks_exact_mut(2) { pair.swap(0, 1); } }
                pcm
            }
            Some(4) => {
                // AWC's IMA block headers are little-endian even in TADA banks.
                ima(bytes, samples)?
            }
            Some(codec) => return Err(format!("AWC codec 0x{codec:02x} is not implemented; Web-build codec IDs require their own verified adapter")),
            None => return Err("AWC stream lacks codec".into()),
        };
        let len = pcm.len() as u32;
        let mut wav = Vec::with_capacity(pcm.len() + 44);
        wav.extend(b"RIFF"); wav.extend((len + 36).to_le_bytes()); wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes()); wav.extend(1u16.to_le_bytes()); wav.extend(1u16.to_le_bytes());
        wav.extend((rate as u32).to_le_bytes()); wav.extend((rate as u32 * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes()); wav.extend(16u16.to_le_bytes());
        wav.extend(b"data"); wav.extend(len.to_le_bytes()); wav.extend(pcm);
        Ok(wav)
    }
}
const STEPS: [i32; 89] = [
    7,8,9,10,11,12,13,14,16,17,19,21,23,25,28,31,34,37,41,45,50,55,60,66,
    73,80,88,97,107,118,130,143,157,173,190,209,230,253,279,307,337,371,408,
    449,494,544,598,658,724,796,876,963,1060,1166,1282,1411,1552,1707,1878,
    2066,2272,2499,2749,3024,3327,3660,4026,4428,4871,5358,5894,6484,7132,
    7845,8630,9493,10442,11487,12635,13899,15289,16818,18500,20350,22385,
    24623,27086,29794,32767,
];
fn ima(bytes: &[u8], samples: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(samples * 2);
    let mut left = samples;
    let mut at = 0usize;
    while left != 0 {
        let count = left.min((0x800 - 4) * 2);
        let block = range(bytes, at, 4 + count.div_ceil(2))?;
        let mut index = i16::from_le_bytes([block[0], block[1]]) as i32;
        if !(0..=88).contains(&index) { return Err("invalid AWC IMA step index".into()); }
        let mut predictor = i16::from_le_bytes([block[2], block[3]]) as i32;
        // The predictor is state only: do NOT emit an extra sample per block.
        for i in 0..count {
            let nibble = ((block[4 + i / 2] >> ((i % 2) * 4)) & 15) as usize;
            let step = STEPS[index as usize];
            let mut delta = step >> 3;
            if nibble & 1 != 0 { delta += step >> 2; }
            if nibble & 2 != 0 { delta += step >> 1; }
            if nibble & 4 != 0 { delta += step; }
            predictor = (predictor + if nibble & 8 != 0 { -delta } else { delta }).clamp(-32768, 32767);
            out.extend((predictor as i16).to_le_bytes());
            index = (index + [-1,-1,-1,-1,2,4,6,8][nibble & 7]).clamp(0, 88);
        }
        left -= count;
        at += 0x800;
    }
    Ok(out)
}
