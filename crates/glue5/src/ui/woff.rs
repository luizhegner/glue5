//! Bounded WOFF 1 to sfnt conversion for the font in the user's game build.
//! No game bytes are embedded in the app.
use std::io::Read;

const MAX_FONT: usize = 8 * 1024 * 1024;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Decode one WOFF 1 font; reject truncation, oversized tables and malformed
/// compressed streams before giving the result to the font rasterizer.
pub(super) fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 44 || bytes.len() > MAX_FONT || bytes.get(..4)? != b"wOFF"
        || u32_at(bytes, 8)? as usize != bytes.len() || u16_at(bytes, 14)? != 0 {
        return None;
    }
    let flavor = bytes.get(4..8)?;
    if !matches!(flavor, [0, 1, 0, 0] | b"true" | b"OTTO") { return None; }
    let count = u16_at(bytes, 12)? as usize;
    if count == 0 || count > 4095 { return None; }
    let directory_end = 44usize.checked_add(count.checked_mul(20)?)?;
    if directory_end > bytes.len() { return None; }
    let expected = u32_at(bytes, 16)? as usize;
    let header_len = 12usize.checked_add(count.checked_mul(16)?)?;
    if expected < header_len || expected > MAX_FONT { return None; }
    let power = 1usize << (usize::BITS - 1 - count.leading_zeros());
    let mut out = Vec::with_capacity(expected);
    out.extend_from_slice(flavor);
    out.extend_from_slice(&(count as u16).to_be_bytes());
    out.extend_from_slice(&((power * 16) as u16).to_be_bytes());
    out.extend_from_slice(&((power.trailing_zeros()) as u16).to_be_bytes());
    out.extend_from_slice(&((count * 16 - power * 16) as u16).to_be_bytes());
    out.resize(header_len, 0);
    for i in 0..count {
        let entry = 44 + i * 20;
        let offset = u32_at(bytes, entry + 4)? as usize;
        let compressed = u32_at(bytes, entry + 8)? as usize;
        let original = u32_at(bytes, entry + 12)? as usize;
        if compressed > original || offset < directory_end || original > MAX_FONT { return None; }
        let source = bytes.get(offset..offset.checked_add(compressed)?)?;
        let destination = out.len();
        let end = destination.checked_add(original)?;
        let padded = end.checked_add(3)? & !3;
        if padded > expected { return None; }
        if compressed == original {
            out.extend_from_slice(source);
        } else {
            let mut decoder = flate2::read::ZlibDecoder::new(source).take((original as u64) + 1);
            decoder.read_to_end(&mut out).ok()?;
            if out.len() != end { return None; }
        }
        out.resize(padded, 0);
        let record = 12 + i * 16;
        out[record..record + 4].copy_from_slice(bytes.get(entry..entry + 4)?);
        out[record + 4..record + 8].copy_from_slice(bytes.get(entry + 16..entry + 20)?);
        out[record + 8..record + 12].copy_from_slice(&(destination as u32).to_be_bytes());
        out[record + 12..record + 16].copy_from_slice(&(original as u32).to_be_bytes());
    }
    (out.len() == expected).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_invalid_or_oversized_input() {
        assert!(decode(b"wOFF").is_none());
        assert!(decode(&vec![0; MAX_FONT + 1]).is_none());
    }

    #[test]
    fn converts_uncompressed_table() {
        let mut w = vec![0u8; 68];
        w[..4].copy_from_slice(b"wOFF");
        w[4..8].copy_from_slice(&[0, 1, 0, 0]);
        w[8..12].copy_from_slice(&68u32.to_be_bytes());
        w[12..14].copy_from_slice(&1u16.to_be_bytes());
        w[16..20].copy_from_slice(&32u32.to_be_bytes());
        w[44..48].copy_from_slice(b"test");
        w[48..52].copy_from_slice(&64u32.to_be_bytes());
        w[52..56].copy_from_slice(&4u32.to_be_bytes());
        w[56..60].copy_from_slice(&4u32.to_be_bytes());
        w[64..68].copy_from_slice(b"data");
        let sfnt = decode(&w).unwrap();
        assert_eq!(&sfnt[12..16], b"test");
        assert_eq!(&sfnt[28..32], b"data");
        w[48..52].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode(&w).is_none());
    }

    #[test]
    fn decompresses_tables_and_rejects_an_invalid_zlib_stream() {
        let table=vec![7;128];
        let mut encoder=flate2::write::ZlibEncoder::new(Vec::new(),flate2::Compression::default());
        encoder.write_all(&table).unwrap();
        let compressed=encoder.finish().unwrap();
        assert!(compressed.len()<table.len());
        let mut w=vec![0u8;64];
        w[..4].copy_from_slice(b"wOFF");
        w[4..8].copy_from_slice(&[0,1,0,0]);
        w[8..12].copy_from_slice(&((64+compressed.len()) as u32).to_be_bytes());
        w[12..14].copy_from_slice(&1u16.to_be_bytes());
        w[16..20].copy_from_slice(&156u32.to_be_bytes());
        w[44..48].copy_from_slice(b"test");
        w[48..52].copy_from_slice(&64u32.to_be_bytes());
        w[52..56].copy_from_slice(&(compressed.len() as u32).to_be_bytes());
        w[56..60].copy_from_slice(&128u32.to_be_bytes());
        w.extend_from_slice(&compressed);
        assert_eq!(&decode(&w).unwrap()[28..],table.as_slice());
        w[64]^=0xff;
        assert!(decode(&w).is_none());
    }
}
