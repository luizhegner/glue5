//! The game scripts' globals, as ScriptHookV's `getGlobalPtr` reaches them
//! (`globals.int(i)`, `globals.setInt(i, v)`...), checked against the blocks
//! the game has loaded; and `game.version()`.
//!
//! A global's index is 24 bits: the block (`rage::scrProgram`'s 64 global
//! blocks) above bit 18, the value in it below, as the script VM's
//! `GLOBAL_U24` takes it (`scrThread::Run`). Values are 8 bytes (`scrValue`
//! on wasm64); ints and floats are the first 4, which is all the VM reads
//! and writes of them (its `IADD` and the like).

use glue5_runtime::memory::Mem;
use mlua::Lua;

use crate::natives;

/// The blocks' addresses, u64 each (what `scrThread::IsValidGlobal` walks).
const BLOCKS: u64 = 15408128;
/// The blocks' sizes in values, u32 each.
const SIZES: u64 = 15408640;
const BLOCK_COUNT: u32 = 64;
const BLOCK_BITS: u32 = 18;

/// The longest string read at once.
const MAX_STRING: u64 = 256;

/// The address of `bytes` bytes at global `index` and the bytes from there
/// to the end of its block, given each block's address and size (in
/// values); an error if they are not all in a block the game has loaded.
fn locate(index: f64, bytes: u64, block: impl Fn(u32) -> (u64, u32)) -> Result<(u64, u64), String> {
    let max = f64::from(BLOCK_COUNT << BLOCK_BITS);
    if index.fract() != 0.0 || !(0.0..max).contains(&index) {
        return Err(format!("{index} is not a global's index (0 to {})", max as u32 - 1));
    }
    let index = index as u32;
    let (at, size) = block(index >> BLOCK_BITS);
    let offset = u64::from(index & ((1 << BLOCK_BITS) - 1));
    if at == 0 || size == 0 {
        return Err(format!("{index}: block {} of the globals is not loaded", index >> BLOCK_BITS));
    }
    let room = (u64::from(size) - offset.min(u64::from(size))) * 8;
    if bytes > room {
        return Err(format!("{index}: past the end of block {} of the globals ({size} values)", index >> BLOCK_BITS));
    }
    Ok((at + offset * 8, room))
}

/// Memory, the address of `bytes` bytes at `index` and the room from there
/// to the block's end, in a mod's script.
fn at(what: &str, index: f64, bytes: u64) -> mlua::Result<(Mem, u64, u64)> {
    let env = natives::env().ok_or_else(|| mlua::Error::runtime(format!("globals.{what}: only in a mod's script")))?;
    let mem = unsafe { Mem::of(env) };
    let block = |b: u32| (mem.u64(BLOCKS + 8 * u64::from(b)), mem.u32(SIZES + 4 * u64::from(b)));
    let (address, room) = locate(index, bytes, block).map_err(|e| mlua::Error::runtime(format!("globals.{what}: {e}")))?;
    Ok((mem, address, room))
}

/// An int as natives take them: from i32's minimum to u32's maximum.
fn int(v: f64) -> Option<i32> {
    (v.fract() == 0.0 && v >= i32::MIN as f64 && v <= u32::MAX as f64).then_some(v as i64 as i32)
}

/// Installs `globals` and `game`.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.create_table()?;
    globals.set(
        "int",
        lua.create_function(|_, i: f64| {
            let (mem, a, _) = at("int", i, 4)?;
            Ok(mem.i32(a))
        })?,
    )?;
    globals.set(
        "float",
        lua.create_function(|_, i: f64| {
            let (mem, a, _) = at("float", i, 4)?;
            Ok(f32::from_bits(mem.u32(a)))
        })?,
    )?;
    globals.set(
        "bool",
        lua.create_function(|_, i: f64| {
            let (mem, a, _) = at("bool", i, 4)?;
            Ok(mem.i32(a) != 0)
        })?,
    )?;
    globals.set(
        "string",
        lua.create_function(|lua, i: f64| {
            let (mem, a, room) = at("string", i, 1)?;
            let bytes = mem.bytes(a, room.min(MAX_STRING) as usize);
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            lua.create_string(&bytes[..end])
        })?,
    )?;
    globals.set(
        "setInt",
        lua.create_function(|_, (i, v): (f64, f64)| {
            let v = int(v).ok_or_else(|| mlua::Error::runtime(format!("globals.setInt: {v} is not an int")))?;
            let (mem, a, _) = at("setInt", i, 4)?;
            mem.set_i32(a, v);
            Ok(())
        })?,
    )?;
    globals.set(
        "setFloat",
        lua.create_function(|_, (i, v): (f64, f64)| {
            let (mem, a, _) = at("setFloat", i, 4)?;
            mem.set_u32(a, (v as f32).to_bits());
            Ok(())
        })?,
    )?;
    globals.set(
        "setBool",
        lua.create_function(|_, (i, v): (f64, bool)| {
            let (mem, a, _) = at("setBool", i, 4)?;
            mem.set_i32(a, v as i32);
            Ok(())
        })?,
    )?;
    globals.set(
        "setString",
        lua.create_function(|_, (i, text, size): (f64, mlua::LuaString, Option<f64>)| {
            let text = text.as_bytes();
            let bad = |e: String| mlua::Error::runtime(format!("globals.setString: {e}"));
            if text.contains(&0) {
                return Err(bad("the text has a NUL".into()));
            }
            let need = text.len() as u64 + 1;
            if let Some(size) = size
                && need as f64 > size
            {
                return Err(bad(format!("{} bytes do not fit in {size}", text.len())));
            }
            let (mem, a, _) = at("setString", i, need)?;
            mem.bytes_mut(a, text.len()).copy_from_slice(&text);
            mem.set_u8(a + text.len() as u64, 0);
            Ok(())
        })?,
    )?;
    globals.set(
        "valid",
        lua.create_function(|_, i: f64| Ok(at("valid", i, 4).is_ok()))?,
    )?;
    lua.globals().set("globals", globals)?;

    let game = lua.create_table()?;
    game.set("version", lua.create_function(|_, ()| Ok(glue5_game::GAME_BUILD))?)?;
    lua.globals().set("game", game)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Block 0 at 1000 with 10 values, block 1 not loaded, block 2 at 5000
    /// with 3.
    fn blocks(b: u32) -> (u64, u32) {
        match b {
            0 => (1000, 10),
            2 => (5000, 3),
            _ => (0, 0),
        }
    }

    #[test]
    fn bounds() {
        assert_eq!(locate(0.0, 4, blocks), Ok((1000, 80)));
        assert_eq!(locate(9.0, 8, blocks), Ok((1072, 8)));
        assert!(locate(10.0, 4, blocks).unwrap_err().contains("past the end"));
        // A string from the last value: 8 bytes fit, 9 do not.
        assert!(locate(9.0, 9, blocks).is_err());
        assert!(locate(f64::from(1 << 18), 4, blocks).unwrap_err().contains("not loaded"));
        assert_eq!(locate(f64::from((2 << 18) + 2), 4, blocks), Ok((5016, 8)));
        assert!(locate(f64::from((2 << 18) + 3), 4, blocks).is_err());
        for bad in [-1.0, 0.5, f64::from(64u32 << 18), f64::NAN, f64::INFINITY] {
            assert!(locate(bad, 4, blocks).unwrap_err().contains("not a global's index"), "{bad}");
        }
    }
}
