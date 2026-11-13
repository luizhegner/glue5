//! Parts of the C library compiled into the game that the host does
//! faster with the same results.

use std::collections::HashMap;
use std::sync::Mutex;

use glue5_game::{Env, orig};

use crate::Locked;
use crate::memory::Mem;

/// Hook for `strtod`: musl's goes through binary128 software floats, ~210
/// ns a number of the game's data (it has ~360,000 at boot: timecycles,
/// handling, weapons); Rust's parser takes ~10. Both round correctly: the
/// same double for every decimal. Hex, inf/nan, no digits, and results that
/// are out of range or subnormal (musl sets errno) stay musl's.
pub(crate) unsafe extern "C" fn strtod(env: *mut Env, s: u64, end: u64) -> f64 {
    let mem = unsafe { Mem::of(env) };
    let mut i = s;
    while matches!(mem.u8(i), b' ' | b'\t'..=b'\r') {
        i += 1;
    }
    let start = i;
    if matches!(mem.u8(i), b'+' | b'-') {
        i += 1;
    }
    if mem.u8(i) == b'0' && matches!(mem.u8(i + 1), b'x' | b'X') {
        return unsafe { orig::strtod(env, s, end) };
    }
    // Digits, a point and digits: how many, any but zeros.
    let (mut digits, mut nonzero) = (0u32, false);
    let mut point = false;
    loop {
        match mem.u8(i) {
            b'0'..=b'9' => {
                nonzero |= mem.u8(i) != b'0';
                digits += 1;
            }
            b'.' if !point => point = true,
            _ => break,
        }
        i += 1;
    }
    if digits == 0 {
        return unsafe { orig::strtod(env, s, end) };
    }
    // An exponent only with digits (else the number ends before the `e`).
    if matches!(mem.u8(i), b'e' | b'E') {
        let mut j = i + 1;
        if matches!(mem.u8(j), b'+' | b'-') {
            j += 1;
        }
        if mem.u8(j).is_ascii_digit() {
            while mem.u8(j).is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    // ASCII only (scanned above).
    let text = String::from_utf8_lossy(mem.bytes(start, (i - start) as usize));
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() && !v.is_subnormal() && (v != 0.0 || !nonzero) => {
            if end != 0 {
                mem.set_u64(end, i);
            }
            v
        }
        _ => unsafe { orig::strtod(env, s, end) },
    }
}

// ---- inflate ------------------------------------------------------------------

/// zlib 1.2.3's `z_stream` in this build.
const NEXT_IN: u64 = 0;
const AVAIL_IN: u64 = 8;
const TOTAL_IN: u64 = 16;
const NEXT_OUT: u64 = 24;
const AVAIL_OUT: u64 = 32;
const TOTAL_OUT: u64 = 40;
const MSG: u64 = 48;
const STATE: u64 = 56;
/// Its `inflate_state`: the mode (HEAD after an init or reset, the
/// engine's inlined ones too), wrap (0 raw, 1 zlib, 2 gzip), window bits.
const MODE: u64 = 0;
const WRAP: u64 = 8;
const WBITS: u64 = 48;
const HEAD: u32 = 0;
const TYPE: u32 = 11;
const DONE: u32 = 26;
const BAD: u32 = 27;

/// Streams inflated natively, by their engine state's address (heap
/// memory: it never moves, unlike the stack z_streams some callers use).
static STREAMS: Mutex<Option<HashMap<u64, Native>>> = Mutex::new(None);
/// zlib's messages, copied once into the engine's heap.
static MESSAGES: Mutex<Option<HashMap<&'static str, u64>>> = Mutex::new(None);

struct Native {
    inflate: zlib_rs::Inflate,
    zlib: bool,
}

/// Hook for zlib's `inflate`: zlib-rs for streams with a 32 KB window, raw
/// or zlib-wrapped (the archives, the scripts, Scaleform, saves), from
/// their start; the others (zlibStream's 512-byte windows, which 1.2.3 holds
/// to and zlib-rs not) stay 1.2.3's. The same return codes, counters and
/// input consumed at every call (compared call by call on the boot set's
/// streams, whole and corrupted); ~1.8x faster. The engine's window is never
/// made for a native stream; `adler` and `data_type` are not kept (nothing
/// reads them).
pub(crate) unsafe extern "C" fn inflate(env: *mut Env, strm: u64, flush: u32) -> u32 {
    let mem = unsafe { Mem::of(env) };
    match inflate_natively(mem, strm, flush, |text| message(env, mem, text)) {
        Some(code) => code as u32,
        None => unsafe { orig::inflate(env, strm, flush) },
    }
}

/// [`inflate`] for a stream zlib-rs takes (`message` puts an error's text
/// in the engine's memory); None for the others.
fn inflate_natively(mem: Mem, strm: u64, flush: u32, message: impl FnOnce(&'static str) -> u64) -> Option<i32> {
    use zlib_rs::{InflateError, InflateFlush, Status};
    let state = if strm == 0 { 0 } else { mem.u64(strm + STATE) };
    // zlib's stream errors as zlib gives them.
    if state == 0 || mem.u64(strm + NEXT_OUT) == 0 || (mem.u64(strm + NEXT_IN) == 0 && mem.u32(strm + AVAIL_IN) != 0) {
        return None;
    }
    let taken = STREAMS.locked().get_or_insert_default().remove(&state);
    let mut native = if mem.u32(state + MODE) == HEAD {
        let wrap = mem.u32(state + WRAP);
        if mem.u32(state + WBITS) != 15 || wrap > 1 {
            return None;
        }
        let zlib = wrap == 1;
        match taken {
            Some(mut native) if native.zlib == zlib => {
                native.inflate.reset(zlib);
                native
            }
            _ => Native { inflate: zlib_rs::Inflate::new(zlib, 15), zlib },
        }
    } else {
        taken?
    };
    // 1.2.3 tells only these apart.
    let flush = match flush {
        4 => InflateFlush::Finish,
        5 => InflateFlush::Block,
        _ => InflateFlush::NoFlush,
    };
    let (next_in, avail_in) = (mem.u64(strm + NEXT_IN), mem.u32(strm + AVAIL_IN));
    let (next_out, avail_out) = (mem.u64(strm + NEXT_OUT), mem.u32(strm + AVAIL_OUT));
    let (in0, out0) = (native.inflate.total_in(), native.inflate.total_out());
    let result = native.inflate.decompress(
        mem.bytes(next_in, avail_in as usize),
        mem.bytes_mut(next_out, avail_out as usize),
        flush,
    );
    let (read, wrote) = (native.inflate.total_in() - in0, native.inflate.total_out() - out0);
    mem.set_u64(strm + NEXT_IN, next_in + read);
    mem.set_u32(strm + AVAIL_IN, avail_in - read as u32);
    mem.set_u64(strm + TOTAL_IN, mem.u64(strm + TOTAL_IN) + read);
    mem.set_u64(strm + NEXT_OUT, next_out + wrote);
    mem.set_u32(strm + AVAIL_OUT, avail_out - wrote as u32);
    mem.set_u64(strm + TOTAL_OUT, mem.u64(strm + TOTAL_OUT) + wrote);
    let (code, mode) = match result {
        Ok(Status::Ok) => (0, TYPE),
        Ok(Status::BufError) => (-5, TYPE),
        Ok(Status::StreamEnd) => (1, DONE),
        Err(e) => {
            if let Some(text) = native.inflate.error_message() {
                mem.set_u64(strm + MSG, message(text));
            }
            let code = match e {
                InflateError::NeedDict { .. } => 2,
                InflateError::StreamError => -2,
                InflateError::DataError => -3,
                InflateError::MemError => -4,
            };
            (code, BAD)
        }
    };
    // Never HEAD again until the engine starts a stream there.
    mem.set_u32(state + MODE, mode);
    STREAMS.locked().get_or_insert_default().insert(state, native);
    Some(code)
}

/// Hook for zlib's `inflateEnd`: a native stream ends with it.
pub(crate) unsafe extern "C" fn inflate_end(env: *mut Env, strm: u64) -> u32 {
    if strm != 0 {
        let state = unsafe { Mem::of(env) }.u64(strm + STATE);
        if let Some(streams) = STREAMS.locked().as_mut() {
            streams.remove(&state);
        }
    }
    unsafe { orig::inflate_end(env, strm) }
}

/// `text` as a C string in the engine's memory, made once.
fn message(env: *mut Env, mem: Mem, text: &'static str) -> u64 {
    *MESSAGES.locked().get_or_insert_default().entry(text).or_insert_with(|| {
        let at = unsafe { orig::builtin_malloc(env, text.len() as u64 + 1) };
        if at != 0 {
            mem.write_str(at, text, text.len() + 1);
        }
        at
    })
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.strtod = Some(strtod);
    hooks.inflate = Some(inflate);
    hooks.inflate_end = Some(inflate_end);
}

#[cfg(test)]
mod tests {
    #[test]
    fn rust_parses_what_c_does() {
        // The forms the scan hands over (C's strtod takes all of them).
        for (text, want) in
            [("1.", 1.0), (".5", 0.5), ("+2.25", 2.25), ("-0.0012345", -0.0012345), ("1e3", 1000.0), ("2.5E-3", 0.0025)]
        {
            assert_eq!(text.parse::<f64>().unwrap(), want, "{text}");
        }
        assert!("-0.0".parse::<f64>().unwrap().is_sign_negative());
    }

    /// Raw and zlib-wrapped streams in pieces, and a corrupted zlib one:
    /// what the engine's z_stream reads back.
    #[test]
    fn inflates_into_the_engines_stream() {
        use super::*;
        let text: Vec<u8> = (0..200_000u32).flat_map(|i| format!("{} ", i % 997 * 31).into_bytes()).collect();
        for zlib in [false, true] {
            let mut deflate = zlib_rs::Deflate::new(6, zlib, 15);
            let mut packed = vec![0u8; text.len()];
            deflate.compress(&text, &mut packed, zlib_rs::DeflateFlush::Finish).unwrap();
            packed.truncate(deflate.total_out() as usize);
            // Raw streams have no check: corrupted, they inflate to anything.
            for corrupt in [false, true].into_iter().filter(|c| !c || zlib) {
                if corrupt {
                    let middle = packed.len() / 2;
                    packed[middle] ^= 0x55;
                }
                let mut memory = vec![0u8; 4 << 20];
                let mem = Mem::over(&mut memory);
                let (strm, state, input, output) = (0x100u64, 0x200u64, 0x1000u64, 0x200000u64);
                mem.bytes_mut(input, packed.len()).copy_from_slice(&packed);
                mem.set_u64(strm + STATE, state);
                mem.set_u32(state + MODE, HEAD);
                mem.set_u32(state + WRAP, zlib as u32);
                mem.set_u32(state + WBITS, 15);
                mem.set_u64(strm + NEXT_IN, input);
                mem.set_u64(strm + NEXT_OUT, output);
                let mut code = 0;
                // 8 KB of input at a time, 16 KB of room.
                while code == 0 || code == -5 {
                    let fed = mem.u64(strm + NEXT_IN) - input;
                    mem.set_u32(strm + AVAIL_IN, (packed.len() as u64 - fed).min(8192) as u32);
                    mem.set_u32(strm + AVAIL_OUT, 16384);
                    code = inflate_natively(mem, strm, 2, |_| 0x77).unwrap();
                    if code == -5 && mem.u32(strm + AVAIL_IN) == 0 && fed == packed.len() as u64 {
                        break;
                    }
                }
                let out = mem.u64(strm + TOTAL_OUT) as usize;
                if corrupt {
                    assert_eq!((code, mem.u64(strm + MSG), mem.u32(state + MODE)), (-3, 0x77, BAD), "zlib {zlib}");
                } else {
                    assert_eq!(code, 1, "zlib {zlib}");
                    assert_eq!(mem.u64(strm + TOTAL_IN), packed.len() as u64);
                    assert_eq!(mem.bytes(output, out), &text[..]);
                    assert_eq!(mem.u32(state + MODE), DONE);
                }
                STREAMS.locked().get_or_insert_default().remove(&state);
            }
        }
    }
}
