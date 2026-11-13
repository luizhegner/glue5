//! Bounded AWC bank reader, mono SFX and blocked IMA music -> WAV.
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
#[derive(Clone, Debug, Serialize)]
pub struct MusicChannel {
    pub file_hash: u32,
    pub stream_id: u32,
    pub samples: u32,
    pub sample_rate: u16,
    pub codec: u8,
}
#[derive(Clone, Debug, Serialize)]
pub struct Music {
    pub blocks: u32,
    pub block_bytes: u32,
    /// Header order, NOT sorted stream-ID order. May contain separate stems.
    pub channels: Vec<MusicChannel>,
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

    pub fn web_opus_ogg(&self, id: u32, build: &str) -> Result<Vec<u8>> {
        if self.encrypted || self.streamed { return Err("Web Opus adapter currently requires an unencrypted mono SFX bank".into()); }
        let stream = self.streams.iter().find(|s| s.id == id).ok_or("AWC stream ID not found")?;
        if stream.codec != Some(12) || stream.sample_rate != Some(48000) {
            return Err("selected waveform is not the Web profile's codec-12/48kHz layout".into());
        }
        let samples = stream.samples.ok_or("AWC stream lacks sample count")?;
        let data = stream.chunks.iter().find(|c| c.tag == 0x55).ok_or("AWC waveform data missing")?;
        crate::web_opus::ogg(range(self.bytes, data.offset, data.bytes)?, samples, id, build)
    }

    /// Inspect music channel order without assuming that all channels should
    /// be mixed together or that a codec ID means the same in every build.
    pub fn music(&self) -> Result<Music> {
        if !self.streamed || self.streams[0].id != 0 { return Err("expected an AWC music master stream".into()); }
        let tag = self.streams[0].chunks.iter().find(|c| c.tag == 0x48).ok_or("music format chunk missing")?;
        let format = range(self.bytes, tag.offset, tag.bytes)?;
        let blocks = word(format, 0, 4, self.big_endian)? as u32;
        let block_bytes = word(format, 4, 4, self.big_endian)? as u32;
        let count = word(format, 8, 4, self.big_endian)? as usize;
        if blocks == 0 || blocks > 65536 || !(0x800..=4*1024*1024).contains(&block_bytes)
            || block_bytes % 0x800 != 0 || !(1..=32).contains(&count) || format.len() != 12 + count * 16 {
            return Err("invalid or unsupported AWC music layout".into());
        }
        let mut channels = Vec::with_capacity(count);
        let mut seen = HashSet::new();
        for ch in 0..count {
            let at = 12 + ch * 16;
            let file_hash = word(format, at, 4, self.big_endian)? as u32;
            let samples = word(format, at + 4, 4, self.big_endian)? as u32;
            let sample_rate = word(format, at + 10, 2, self.big_endian)? as u16;
            if samples == 0 || sample_rate < 1000 || !seen.insert(file_hash & 0x1fffffff) {
                return Err("invalid or ambiguous music channel".into());
            }
            channels.push(MusicChannel { file_hash, stream_id: file_hash & 0x1fffffff,
                samples, sample_rate, codec: format[at + 12] });
        }
        let data = self.streams[0].chunks.iter().find(|c| c.tag == 0x55).ok_or("music data chunk missing")?;
        if (blocks as u64 * block_bytes as u64) != data.bytes as u64 {
            return Err("music block count does not cover its data exactly".into());
        }
        Ok(Music { blocks, block_bytes, channels })
    }

    /// Decode explicit mono/stereo channel indices, keeping their order.
    /// No implicit stem summing, resampling, looping or guessed channel pair.
    pub fn music_wav(&self, selected: &[usize]) -> Result<Vec<u8>> {
        if self.encrypted { return Err("AWC payload cipher is not implemented".into()); }
        let music = self.music()?;
        if selected.is_empty() || selected.len() > 2 || (selected.len() == 2 && selected[0] == selected[1]) {
            return Err("select one or two distinct music channel indices".into());
        }
        let mut rate = 0;
        let mut samples = 0;
        // Validate every channel's codec, including unselected ones: the
        // block addressing below is only verified for the IMA layout.
        if music.channels.iter().any(|c| c.codec != 4) {
            return Err("music decoder requires IMA codec 4; Web Opus and other layouts are not guessed".into());
        }
        for &i in selected {
            let ch = music.channels.get(i).ok_or("music channel index outside header")?;
            if rate != 0 && (rate != ch.sample_rate || samples != ch.samples) {
                return Err("selected music channels disagree in rate or sample count".into());
            }
            rate = ch.sample_rate;
            samples = ch.samples;
        }
        let pcm_bytes = samples as u64 * selected.len() as u64 * 2;
        if pcm_bytes + 44 > MAX_BYTES { return Err("music PCM output exceeds byte budget; needs a streaming export provider".into()); }
        let data = self.streams[0].chunks.iter().find(|c| c.tag == 0x55).unwrap();
        let payload = range(self.bytes, data.offset, data.bytes)?;
        let mut out = wav_header(rate, selected.len() as u16, pcm_bytes as u32);
        let mut done = 0usize;
        for block in payload.chunks_exact(music.block_bytes as usize) {
            let n = music.channels.len();
            range(block, 0, n * 0x18)?;
            let mut frames = Vec::with_capacity(n);
            let mut counts = Vec::with_capacity(n);
            let mut header_bytes = n * 0x18;
            for ch in 0..n {
                let at = ch * 0x18;
                let count = word(block, at + 4, 4, self.big_endian)? as usize;
                let decoded = word(block, at + 12, 4, self.big_endian)? as usize;
                if count == 0 || count > block.len() / 0x800 || decoded == 0 || decoded > count * 4088
                    || word(block, at + 8, 4, self.big_endian)? != 0
                    || word(block, at + 16, 4, self.big_endian)? != 0
                    || word(block, at + 20, 4, self.big_endian)? != 0 {
                    return Err("unsupported or invalid IMA music block channel header".into());
                }
                header_bytes += count * 4;
                frames.push(count);
                counts.push(decoded);
            }
            let mut at = header_bytes.next_multiple_of(0x800);
            range(block, 0, at)?;
            let mut decoded = Vec::with_capacity(selected.len());
            for ch in 0..n {
                let raw = range(block, at, frames[ch] * 0x800)?;
                if selected.contains(&ch) { decoded.push((ch, ima(raw, counts[ch])?)); }
                at += frames[ch] * 0x800;
            }
            let count = counts[selected[0]];
            if selected.iter().any(|&ch| counts[ch] != count) || done + count > samples as usize {
                return Err("music block samples disagree with selected channels or total".into());
            }
            for frame in 0..count {
                for &ch in selected {
                    let pcm = &decoded.iter().find(|(i, _)| *i == ch).unwrap().1;
                    out.extend_from_slice(&pcm[frame * 2..frame * 2 + 2]);
                }
            }
            done += count;
        }
        if done != samples as usize { return Err("music sample total does not match header; partial audio not exported".into()); }
        Ok(out)
    }
}

fn wav_header(rate: u16, channels: u16, bytes: u32) -> Vec<u8> {
    let mut wav = Vec::with_capacity(bytes as usize + 44);
    wav.extend(b"RIFF"); wav.extend((bytes + 36).to_le_bytes()); wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes()); wav.extend(1u16.to_le_bytes()); wav.extend(channels.to_le_bytes());
    wav.extend((rate as u32).to_le_bytes()); wav.extend((rate as u32 * channels as u32 * 2).to_le_bytes());
    wav.extend((channels * 2).to_le_bytes()); wav.extend(16u16.to_le_bytes());
    wav.extend(b"data"); wav.extend(bytes.to_le_bytes());
    wav
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

