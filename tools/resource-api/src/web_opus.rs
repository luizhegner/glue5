//! Packet-preserving Ogg encapsulation for the explicitly selected Web build.
//! This is NOT an Opus decoder or a generic interpretation of AWC codec 12.
//! Framing verified by read-only inspection of audDecoderOpus SubmitPacket/
//! Update in the owner's 8b0b5899ed module: u8 length, packet, repeat; 2880
//! samples per packet, mono, 48000 Hz. No executable/packet bytes are bundled.
//! Ogg/Opus structure: RFC 3533, RFC 7845; packet duration: RFC 6716.
use crate::{Result, MAX_BYTES};

pub const BUILD: &str = "8b0b5899ed";
const SAMPLES_PER_PACKET: u64 = 2880;
const MAX_PACKETS: usize = 30_000;

fn packet_samples(packet: &[u8]) -> Result<u64> {
    let toc = *packet.first().ok_or("empty Opus packet")?;
    if toc & 4 != 0 { return Err("stereo-coded packets are not covered by this mono profile".into()); }
    let config = toc >> 3;
    let frame = if config >= 16 { 120u64 << (config & 3) }
        else if config >= 12 { 480u64 << (config & 1) }
        else { [480,960,1920,2880][(config & 3) as usize] };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (packet.get(1).ok_or("truncated Opus frame-count header")? & 63) as u64,
    };
    if frames == 0 || frame * frames != SAMPLES_PER_PACKET {
        return Err("Opus packet duration is not 60 ms in the selected Web profile".into());
    }
    Ok(frame * frames)
}

fn crc(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in bytes {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 { crc = (crc << 1) ^ if crc & 0x80000000 != 0 { 0x04c11db7 } else { 0 }; }
    }
    crc
}

fn page(out: &mut Vec<u8>, packet: &[u8], flags: u8, granule: u64, serial: u32, sequence: u32) -> Result<()> {
    // One packet/page. A multiple of 255 needs a terminating zero segment.
    let segments = packet.len() / 255 + 1;
    let size = 27 + segments + packet.len();
    if segments > 255 || (out.len() + size) as u64 > MAX_BYTES { return Err("Ogg output exceeds byte/page budget".into()); }
    let start = out.len();
    out.extend(b"OggS"); out.push(0); out.push(flags);
    out.extend(granule.to_le_bytes()); out.extend(serial.to_le_bytes()); out.extend(sequence.to_le_bytes());
    out.extend(0u32.to_le_bytes()); out.push(segments as u8);
    out.extend(std::iter::repeat_n(255u8, segments - 1)); out.push((packet.len() % 255) as u8);
    out.extend(packet);
    let checksum = crc(&out[start..]); out[start+22..start+26].copy_from_slice(&checksum.to_le_bytes());
    Ok(())
}

/// Repack, never recompress. No PCM allocation, encoder, engine startup,
/// guessed decoder delay, resampling, gain changes or fabricated samples.
/// Explicit build selection prevents confusing Switch DSP with Web Opus.
pub fn ogg(payload: &[u8], samples: u32, stream_id: u32, build: &str) -> Result<Vec<u8>> {
    if build != BUILD { return Err("Web Opus profile is only verified for build 8b0b5899ed".into()); }
    if payload.len() as u64 > MAX_BYTES || samples == 0 { return Err("invalid/excessive Web Opus input".into()); }
    let mut packets = Vec::new();
    let mut at = 0;
    while at < payload.len() {
        let size = payload[at] as usize;
        at += 1;
        if size == 0 { return Err("zero-length Web Opus packet; not converted to silence".into()); }
        let end = at.checked_add(size).ok_or("Opus packet range overflow")?;
        let packet = payload.get(at..end).ok_or("truncated Web Opus packet")?;
        packet_samples(packet)?;
        if packets.len() >= MAX_PACKETS { return Err("Web Opus packet count exceeds budget".into()); }
        packets.push(packet);
        at = end;
    }
    let count = packets.len() as u64;
    if count == 0 || samples as u64 > count * SAMPLES_PER_PACKET
        || samples as u64 <= (count - 1) * SAMPLES_PER_PACKET {
        return Err("Web Opus sample count does not match packet framing".into());
    }
    let serial = stream_id ^ 0x35474c55;
    let mut out = Vec::new();
    let mut head = b"OpusHead".to_vec();
    head.extend([1,1]); // header version, mono channels
    head.extend(0u16.to_le_bytes()); // no added preskip; match engine packet output
    head.extend(48000u32.to_le_bytes()); head.extend(0i16.to_le_bytes()); head.push(0);
    page(&mut out, &head, 2, 0, serial, 0)?;
    let vendor = b"GLUE5 packet-preserving import";
    let mut tags = b"OpusTags".to_vec();
    tags.extend((vendor.len() as u32).to_le_bytes()); tags.extend(vendor); tags.extend(0u32.to_le_bytes());
    page(&mut out, &tags, 0, 0, serial, 1)?;
    for (i, packet) in packets.iter().enumerate() {
        let last = i + 1 == packets.len();
        let granule = if last { samples as u64 } else { (i as u64 + 1) * SAMPLES_PER_PACKET };
        page(&mut out, packet, if last {4}else{0}, granule, serial, i as u32 + 2)?;
    }
    Ok(out)
}

