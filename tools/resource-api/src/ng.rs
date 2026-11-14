//! PC RPF7 NG decryption with caller-supplied tables.
use crate::{NgDecryptor, Result};
use std::{fs::File, io::Read, path::Path};

const KEY_COUNT: usize = 101;
const ROUNDS: usize = 17;
const WORDS_PER_KEY: usize = ROUNDS * 4;
const TABLE_WORDS: usize = ROUNDS * 16 * 256;
const DIRECT: [[usize; 4]; 4] = [[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10, 11], [12, 13, 14, 15]];
const MIXED: [[usize; 4]; 4] = [[0, 7, 10, 13], [1, 4, 11, 14], [2, 5, 8, 15], [3, 6, 9, 12]];

pub struct PcNg {
    keys: Vec<u32>,
    tables: Vec<u32>,
    lookup: [u8; 256],
}

impl PcNg {
    /// Read CodeWalker-format files from an explicitly selected local folder.
    pub fn load(directory: &Path) -> Result<Self> {
        fn read_exact_file(directory: &Path, name: &str, size: usize) -> Result<Vec<u8>> {
            let mut bytes = Vec::with_capacity(size);
            File::open(directory.join(name)).map_err(|_| format!("cannot open {name}"))?
                .take(size as u64 + 1).read_to_end(&mut bytes).map_err(|_| format!("cannot read {name}"))?;
            if bytes.len() != size { return Err(format!("invalid size for {name}")); }
            Ok(bytes)
        }
        let keys = read_exact_file(directory, "gtav_ng_key.dat", KEY_COUNT * WORDS_PER_KEY * 4)?;
        let tables = read_exact_file(directory, "gtav_ng_decrypt_tables.dat", TABLE_WORDS * 4)?;
        let lookup = read_exact_file(directory, "gtav_hash_lut.dat", 256)?;
        Self::from_bytes(&keys, &tables, &lookup)
    }

    pub fn from_bytes(keys: &[u8], tables: &[u8], lookup: &[u8]) -> Result<Self> {
        fn words(bytes: &[u8]) -> Vec<u32> {
            bytes.chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect()
        }
        if keys.len() != KEY_COUNT * WORDS_PER_KEY * 4 || tables.len() != TABLE_WORDS * 4 {
            return Err("invalid NG key or table size".into());
        }
        let lookup = lookup.try_into().map_err(|_| "invalid NG hash lookup size")?;
        Ok(Self { keys: words(keys), tables: words(tables), lookup })
    }
}

impl NgDecryptor for PcNg {
    fn decrypt(&self, basename: &str, file_size: u64, bytes: &mut [u8]) -> Result<()> {
        let mut hash = 0u32;
        for character in basename.encode_utf16() {
            let byte = *self.lookup.get(usize::from(character)).ok_or("NG basename exceeds hash alphabet")?;
            let step = hash.wrapping_add(u32::from(byte)).wrapping_mul(1025);
            hash = step ^ (step >> 6);
        }
        let step = hash.wrapping_mul(9);
        hash = (step ^ (step >> 11)).wrapping_mul(32769);
        let key = hash.wrapping_add(file_size as u32).wrapping_add(61) as usize % KEY_COUNT;
        let keys = &self.keys[key * WORDS_PER_KEY..][..WORDS_PER_KEY];
        for block in bytes.chunks_exact_mut(16) {
            for (round, subkey) in keys.chunks_exact(4).enumerate() {
                let lanes = if (2..16).contains(&round) { &MIXED } else { &DIRECT };
                let table = &self.tables[round * 16 * 256..][..16 * 256];
                let mut next = [0; 16];
                for (word, indices) in lanes.iter().enumerate() {
                    let value = indices.iter().fold(subkey[word], |value, &index| {
                        value ^ table[index * 256 + usize::from(block[index])]
                    });
                    next[word * 4..][..4].copy_from_slice(&value.to_le_bytes());
                }
                block.copy_from_slice(&next);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_size_selected_key_and_preserves_partial_block() {
        let keys: Vec<_> = (0..KEY_COUNT).flat_map(|index| vec![index as u8; WORDS_PER_KEY * 4]).collect();
        let decoder = PcNg::from_bytes(&keys, &vec![0; TABLE_WORDS * 4], &[0; 256]).unwrap();
        let mut bytes = [0xff; 19];
        decoder.decrypt("dlc.rpf", 45, &mut bytes).unwrap();
        assert_eq!(&bytes[..16], &[5; 16]);
        assert_eq!(&bytes[16..], &[0xff; 3]);
    }
}
