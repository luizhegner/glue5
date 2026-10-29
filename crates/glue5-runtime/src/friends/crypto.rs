//! The friends' cryptography: each player's key, friend codes, and what two
//! friends work out from their keys alone.
//!
//! - A player is an X25519 key pair, made on the device and kept in its
//!   user directory; a friend is known by its public key.
//! - A friend code is 75 bits and a check character, 16 letters and digits
//!   (Crockford's base 32): the bits are a hash of the owner's key and a
//!   random salt. It is a capability: whoever has it can find its owner and
//!   ask to be friends (the Noise handshake proves the code on both sides),
//!   nothing more. The owner answers with the salt, and the asker checks
//!   that the code comes from the key that answered: knowing a code is not
//!   enough to answer for it. A new salt makes a new code, which retires the
//!   old one.
//! - Two friends meet on the DHT under a key only they can compute, from
//!   the Diffie-Hellman secret of their two keys and the time (it changes
//!   every 6 hours): nothing on the DHT names either of them.

use sha2::{Digest, Sha256};
use snow::params::{DHChoice, NoiseParams};
use snow::resolvers::{CryptoResolver, DefaultResolver};

use crate::dht;

pub(crate) type Key = [u8; 32];

/// Friends: the initiator knows the responder's key (IK); a friend code:
/// neither knows the other's, both know the code (XXpsk0).
pub(crate) const FRIEND_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub(crate) const CODE_PATTERN: &str = "Noise_XXpsk0_25519_ChaChaPoly_BLAKE2s";
/// Bound into every handshake: this protocol and its version (2: message
/// 1 with a MAC, see `first_mac`).
pub(crate) const PROLOGUE: &[u8] = b"glue5 friends 2";
/// How long a meeting key holds.
const EPOCH_SECS: u64 = 6 * 3600;

pub(crate) fn params(pattern: &str) -> NoiseParams {
    pattern.parse().expect("a valid Noise pattern")
}

pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut b = [0; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

/// This player's key pair.
pub(crate) struct Identity {
    private: Key,
    public: Key,
}

impl Identity {
    pub(crate) fn generate() -> Identity {
        Identity::from_private(random())
    }

    pub(crate) fn from_private(private: Key) -> Identity {
        let mut dh = DefaultResolver.resolve_dh(&DHChoice::Curve25519).expect("X25519");
        dh.set(&private);
        let public = dh.pubkey().try_into().expect("a 32-byte key");
        Identity { private, public }
    }

    pub(crate) fn private(&self) -> &Key {
        &self.private
    }

    pub(crate) fn public(&self) -> &Key {
        &self.public
    }

    /// The secret this player shares with the owner of `public`; none for a
    /// key that would make it known (a low-order point gives zero).
    pub(crate) fn shared(&self, public: &Key) -> Option<Key> {
        let mut dh = DefaultResolver.resolve_dh(&DHChoice::Curve25519).expect("X25519");
        dh.set(&self.private);
        let mut out = [0; 32];
        dh.dh(public, &mut out).ok()?;
        (out != [0; 32]).then(|| Sha256::digest([b"glue5 shared 1".as_slice(), &out].concat()).into())
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        // The private key does not outlive its use in memory.
        self.private = [0; 32];
        std::hint::black_box(&self.private);
    }
}

fn hash(label: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    for p in parts {
        h.update((p.len() as u32).to_be_bytes());
        h.update(p);
    }
    h.finalize().into()
}

/// A handshake's message 1 ends with a MAC, under a key of whom it asks:
/// the friend's (public) key, or the friend code. The one asked checks it
/// before anything else: a stranger's junk costs it a hash, not a
/// Diffie-Hellman, and is not counted against the handshakes it answers
/// (junk from many addresses used them all up).
pub(crate) const MAC_LEN: usize = 16;

pub(crate) fn friend_mac_key(public: &Key) -> Key {
    hash("glue5 first mac 1", &[public])
}

pub(crate) fn code_mac_key(code: &str) -> Key {
    hash("glue5 first mac code 1", &[code.as_bytes()])
}

pub(crate) fn first_mac(key: &Key, data: &[u8]) -> [u8; MAC_LEN] {
    use blake2::digest::Mac;
    let mut mac = blake2::Blake2sMac::<blake2::digest::consts::U16>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The epoch of a time (seconds since 1970): meeting keys change with it.
pub(crate) fn epoch(unix: u64) -> u64 {
    unix / EPOCH_SECS
}

/// Where two friends meet on the DHT in an epoch, and the tag they look for
/// on the local network.
pub(crate) fn meeting_key(shared: &Key, epoch: u64) -> dht::Id {
    hash("glue5 meeting 1", &[shared, &epoch.to_be_bytes()])[..20].try_into().unwrap()
}

pub(crate) fn meeting_tag(shared: &Key, epoch: u64) -> Tag {
    hash("glue5 meeting tag 1", &[shared, &epoch.to_be_bytes()])[..8].try_into().unwrap()
}

pub(crate) type Tag = [u8; 8];

// ---- friend codes ----------------------------------------------------------

/// Crockford's base 32: no I, L, O or U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 16;

fn check(chars: &[u8]) -> u8 {
    // Weighted by position, modulo a prime: a character changed, or two
    // swapped, shows (but for 0 and Z, 31 apart).
    let sum: usize = chars.iter().enumerate().map(|(i, c)| (i + 1) * value(*c).unwrap_or(0) as usize).sum();
    ALPHABET[sum % 31]
}

fn value(c: u8) -> Option<u8> {
    ALPHABET.iter().position(|&a| a == c).map(|v| v as u8)
}

/// What a code is made with besides the owner's key.
pub(crate) type Salt = [u8; 16];

/// The friend code of the owner of `public` with `salt` (its 16
/// characters, without dashes); none for a salt of another length.
pub(crate) fn code_of(public: &Key, salt: &[u8]) -> Option<String> {
    if salt.len() != size_of::<Salt>() {
        return None;
    }
    let bits = hash("glue5 code of 1", &[public, salt]);
    let mut chars: Vec<u8> = bits[..CODE_LEN - 1].iter().map(|b| ALPHABET[usize::from(b % 32)]).collect();
    chars.push(check(&chars));
    Some(String::from_utf8(chars).unwrap())
}

/// A friend code as typed: upper case, without spaces or dashes, the
/// letters read as digits put back (I, L as 1; O as 0); none if it is not
/// one (its check character differs: mistyped).
pub(crate) fn parse_code(text: &str) -> Option<String> {
    let chars: Vec<u8> = text
        .bytes()
        .filter(|c| !matches!(c, b' ' | b'-' | b'\t'))
        .map(|c| match c.to_ascii_uppercase() {
            b'I' | b'L' => b'1',
            b'O' => b'0',
            c => c,
        })
        .collect();
    if chars.len() != CODE_LEN || chars.iter().any(|c| value(*c).is_none()) {
        return None;
    }
    (check(&chars[..CODE_LEN - 1]) == chars[CODE_LEN - 1]).then(|| String::from_utf8(chars).unwrap())
}

/// A code as shown: four groups of four.
pub(crate) fn format_code(code: &str) -> String {
    code.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

/// Where a code's owner waits on the DHT, its tag on the local network,
/// and the key both sides prove the code with.
pub(crate) fn code_key(code: &str) -> dht::Id {
    hash("glue5 code 1", &[code.as_bytes()])[..20].try_into().unwrap()
}

pub(crate) fn code_tag(code: &str) -> Tag {
    hash("glue5 code tag 1", &[code.as_bytes()])[..8].try_into().unwrap()
}

pub(crate) fn code_psk(code: &str) -> Key {
    hash("glue5 code psk 1", &[code.as_bytes()])
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_checked() {
        for _ in 0..100 {
            let code = code_of(&random(), &random::<16>()).unwrap();
            assert_eq!(code.len(), 16);
            assert_eq!(parse_code(&code).as_deref(), Some(code.as_str()));
            let shown = format_code(&code);
            assert_eq!(shown.len(), 19);
            assert_eq!(parse_code(&shown.to_lowercase()).as_deref(), Some(code.as_str()));
            // One character changed, or two swapped: not a code.
            let mut typo = code.clone().into_bytes();
            typo[3] = if typo[3] == b'7' { b'8' } else { b'7' };
            assert_eq!(parse_code(std::str::from_utf8(&typo).unwrap()), None);
            let mut swapped = code.clone().into_bytes();
            if swapped[1] != swapped[2] && !matches!((swapped[1], swapped[2]), (b'0', b'Z') | (b'Z', b'0')) {
                swapped.swap(1, 2);
                assert_eq!(parse_code(std::str::from_utf8(&swapped).unwrap()), None);
            }
        }
        assert_eq!(parse_code("ABCD"), None);
        assert_eq!(parse_code("ABCD-EFGH-JKMN-PQRU"), None);
    }

    #[test]
    fn a_code_comes_from_its_owner_s_key() {
        let (a, b) = (Identity::generate(), Identity::generate());
        let salt: Salt = random();
        let code = code_of(a.public(), &salt).unwrap();
        assert_eq!(code_of(a.public(), &salt), Some(code.clone()));
        assert_ne!(code_of(b.public(), &salt), Some(code.clone()));
        assert_ne!(code_of(a.public(), &random::<16>()), Some(code));
        assert_eq!(code_of(a.public(), &salt[1..]), None);
    }

    #[test]
    fn friends_share_a_secret_and_meet() {
        let (a, b, c) = (Identity::generate(), Identity::generate(), Identity::generate());
        let ab = a.shared(b.public()).unwrap();
        assert_eq!(Some(ab), b.shared(a.public()));
        assert_ne!(Some(ab), a.shared(c.public()));
        assert_eq!(Identity::from_private(*a.private()).public(), a.public());
        assert_ne!(meeting_key(&ab, 1), meeting_key(&ab, 2));
        assert_ne!(meeting_key(&ab, 1)[..8], meeting_tag(&ab, 1));
        // A key that would make the secret known.
        assert_eq!(a.shared(&[0; 32]), None);
        assert_eq!(unhex::<32>(&hex(a.public())), Some(*a.public()));
    }
}
