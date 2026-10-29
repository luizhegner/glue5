//! The game's memory, ScriptHookV's and SHVDN's way: entities' addresses
//! (`getScriptHandleBaseAddress`), raw reads and writes, scratch memory to
//! pass to natives (`memory.alloc`).
//!
//! Addresses are the game's (wasm memory offsets): pointers read from the
//! game's objects are addresses too. Every access is checked against the
//! memory's current size, so nothing outside the game's memory is touched;
//! inside it nothing is protected: a wrong write can corrupt the game or
//! make it fail.

use std::cell::RefCell;
use std::collections::HashMap;

use glue5_game::{Env, orig};
use glue5_runtime::memory::Mem;
use mlua::{Lua, Value};

use crate::natives;

/// What a mod may have from `memory.alloc` at once.
const ALLOC_LIMIT: u64 = 16 << 20;
/// The longest string `readString` reads.
const MAX_STRING: u64 = 64 << 10;

thread_local! {
    /// The scratch memory of each run of a mod (its VM's instance): address, size.
    static ALLOCATIONS: RefCell<HashMap<u64, Vec<(u64, u64)>>> = RefCell::default();
}

/// The memory's size now (it grows).
fn size(env: *mut Env) -> u64 {
    unsafe { (*(*env).memory).size }
}

/// `address` as an address of `bytes` bytes inside memory of `size` bytes.
fn span(address: f64, bytes: u64, size: u64) -> Result<u64, String> {
    if address.fract() != 0.0 || !(1.0..size as f64).contains(&address) {
        return Err(if address == 0.0 { "address 0 (a null pointer)".into() } else { format!("{address} is not an address in the game's memory") });
    }
    let at = address as u64;
    if at + bytes > size {
        return Err(format!("{at} + {bytes} bytes is past the end of the game's memory"));
    }
    Ok(at)
}

/// A value Lua passes as an address of `bytes` bytes (a native's pointer
/// parameter), checked against the memory.
pub fn address(env: *mut Env, value: &Value, bytes: u64) -> Result<u64, String> {
    let n = match *value {
        Value::Integer(i) => i as f64,
        Value::Number(n) => n,
        _ => return Err("not an address (memory.alloc gives them, or read one from the game)".into()),
    };
    span(n, bytes, size(env))
}

/// Memory and the address of `bytes` bytes at `address`, in a mod's script.
fn at(what: &str, address: f64, bytes: u64) -> mlua::Result<(Mem, u64)> {
    let fail = |e: String| mlua::Error::runtime(format!("memory.{what}: {e}"));
    let env = natives::env().ok_or_else(|| fail("only in a mod's script".into()))?;
    let at = span(address, bytes, size(env)).map_err(fail)?;
    Ok((unsafe { Mem::of(env) }, at))
}

/// Installs `memory` for the run `instance` of a mod.
pub fn install(lua: &Lua, instance: u64) -> mlua::Result<()> {
    let t = lua.create_table()?;
    t.set(
        "entity",
        lua.create_function(|_, handle: Option<i32>| {
            let env = natives::env().ok_or_else(|| mlua::Error::runtime("memory.entity: only in a mod's script"))?;
            Ok(handle.and_then(|h| natives::entity_address(unsafe { Mem::of(env) }, h)))
        })?,
    )?;
    macro_rules! access {
        ($read:literal, $write:literal, $bytes:expr, $ty:ty, |$mem:ident, $a:ident| $get:expr, |$m2:ident, $b:ident, $v:ident| $set:expr) => {
            t.set(
                $read,
                lua.create_function(|_, address: f64| {
                    let ($mem, $a) = at($read, address, $bytes)?;
                    Ok($get)
                })?,
            )?;
            t.set(
                $write,
                lua.create_function(|_, (address, $v): (f64, $ty)| {
                    let ($m2, $b) = at($write, address, $bytes)?;
                    $set;
                    Ok(())
                })?,
            )?;
        };
    }
    access!("readU8", "writeU8", 1, f64, |m, a| m.u8(a), |m, a, v| m.set_u8(a, v as u8));
    access!("readU16", "writeU16", 2, f64, |m, a| m.u16(a), |m, a, v| m.set_u16(a, v as u16));
    access!("readU32", "writeU32", 4, f64, |m, a| m.u32(a), |m, a, v| m.set_u32(a, v as u32));
    access!("readInt", "writeInt", 4, f64, |m, a| m.i32(a), |m, a, v| m.set_i32(a, v as i64 as i32));
    access!("readU64", "writeU64", 8, f64, |m, a| m.u64(a) as f64, |m, a, v| m.set_u64(a, v as u64));
    access!("readPtr", "writePtr", 8, f64, |m, a| m.u64(a) as f64, |m, a, v| m.set_u64(a, v as u64));
    access!("readFloat", "writeFloat", 4, f32, |m, a| f32::from_bits(m.u32(a)), |m, a, v| m.set_u32(a, v.to_bits()));
    access!(
        "readVector3",
        "writeVector3",
        12,
        mlua::Vector,
        |m, a| mlua::Vector::new(f32::from_bits(m.u32(a)), f32::from_bits(m.u32(a + 4)), f32::from_bits(m.u32(a + 8))),
        |m, a, v| {
            m.set_u32(a, v.x().to_bits());
            m.set_u32(a + 4, v.y().to_bits());
            m.set_u32(a + 8, v.z().to_bits())
        }
    );
    t.set(
        "readString",
        lua.create_function(|lua, address: f64| {
            let (mem, a) = at("readString", address, 1)?;
            let env = natives::env().expect("checked by at");
            let bytes = mem.bytes(a, (size(env) - a).min(MAX_STRING) as usize);
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            lua.create_string(&bytes[..end])
        })?,
    )?;
    t.set(
        "writeString",
        lua.create_function(|_, (address, text): (f64, mlua::LuaString)| {
            let text = text.as_bytes();
            let (mem, a) = at("writeString", address, text.len() as u64 + 1)?;
            mem.bytes_mut(a, text.len()).copy_from_slice(&text);
            mem.set_u8(a + text.len() as u64, 0);
            Ok(())
        })?,
    )?;
    // Bytes at once, as Luau's buffers (the buffer library reads them).
    t.set(
        "read",
        lua.create_function(|lua, (address, bytes): (f64, f64)| {
            if bytes.fract() != 0.0 || !(0.0..=ALLOC_LIMIT as f64).contains(&bytes) {
                return Err(mlua::Error::runtime(format!("memory.read: {bytes} bytes: from 0 to {ALLOC_LIMIT}")));
            }
            let (mem, a) = at("read", address, bytes as u64)?;
            lua.create_buffer(mem.bytes(a, bytes as usize))
        })?,
    )?;
    t.set(
        "write",
        lua.create_function(|_, (address, data): (f64, mlua::Buffer)| {
            let (mem, a) = at("write", address, data.len() as u64)?;
            mem.bytes_mut(a, data.len()).copy_from_slice(&data.to_vec());
            Ok(())
        })?,
    )?;
    t.set(
        "alloc",
        lua.create_function(move |_, bytes: f64| {
            let fail = |e: String| mlua::Error::runtime(format!("memory.alloc: {e}"));
            let env = natives::env().ok_or_else(|| fail("only in a mod's script".into()))?;
            if bytes.fract() != 0.0 || !(1.0..=ALLOC_LIMIT as f64).contains(&bytes) {
                return Err(fail(format!("{bytes} bytes: from 1 to {ALLOC_LIMIT}")));
            }
            let bytes = bytes as u64;
            let held: u64 = ALLOCATIONS.with_borrow(|a| a.get(&instance).map_or(0, |v| v.iter().map(|&(_, n)| n).sum()));
            if held + bytes > ALLOC_LIMIT {
                return Err(fail(format!("a mod may hold {} MB at once ({held} bytes now)", ALLOC_LIMIT >> 20)));
            }
            let at = unsafe { orig::builtin_malloc(env, bytes) };
            if at == 0 {
                return Err(fail("the game has no memory left".into()));
            }
            unsafe { Mem::of(env) }.bytes_mut(at, bytes as usize).fill(0);
            ALLOCATIONS.with_borrow_mut(|a| a.entry(instance).or_default().push((at, bytes)));
            Ok(at)
        })?,
    )?;
    t.set(
        "free",
        lua.create_function(move |_, address: f64| {
            let env = natives::env().ok_or_else(|| mlua::Error::runtime("memory.free: only in a mod's script"))?;
            let found = ALLOCATIONS.with_borrow_mut(|a| {
                let list = a.get_mut(&instance)?;
                let i = list.iter().position(|&(at, _)| at as f64 == address)?;
                Some(list.swap_remove(i).0)
            });
            let at = found.ok_or_else(|| mlua::Error::runtime(format!("memory.free: {address} is not from this mod's memory.alloc")))?;
            unsafe { orig::builtin_free(env, at) };
            Ok(())
        })?,
    )?;
    lua.globals().set("memory", t)
}

/// Frees what the run `instance` of a mod still holds from `memory.alloc`
/// (it has stopped).
///
/// # Safety
/// `env` is the engine thread's.
pub unsafe fn release(env: *mut Env, instance: u64) {
    for (at, _) in ALLOCATIONS.with_borrow_mut(|a| a.remove(&instance)).unwrap_or_default() {
        unsafe { orig::builtin_free(env, at) };
    }
}

#[cfg(test)]
mod tests {
    use super::span;

    #[test]
    fn bounds() {
        assert_eq!(span(1024.0, 8, 4096), Ok(1024));
        assert_eq!(span(4088.0, 8, 4096), Ok(4088));
        assert!(span(4089.0, 8, 4096).unwrap_err().contains("past the end"));
        assert!(span(0.0, 4, 4096).unwrap_err().contains("null"));
        for bad in [-8.0, 0.5, 4096.0, 1e30, f64::NAN, f64::INFINITY] {
            assert!(span(bad, 1, 4096).unwrap_err().contains("not an address"), "{bad}");
        }
    }
}
