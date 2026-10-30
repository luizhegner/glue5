//! The game's natives, as the build script found them, and calling one from
//! Lua: the arguments go in a `rage::scrThread::Info` on the wasm stack, the
//! handler runs through `glue5_game::call_native` (a trap inside comes back
//! as an error), the results come back as Lua values.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::LazyLock;

use glue5_game::{Env, exports, orig};
use glue5_runtime::memory::Mem;
use mlua::{Function, Lua, MultiValue, Table, Value};

/// A value's type, as the mods see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Int,
    Float,
    Bool,
    /// A number, or a string the native gets the hash of.
    Hash,
    Str,
    Vec3,
    /// A number or a boolean, as the database knows no better.
    Any,
    Handle(Handle),
}

/// The kinds of handles: script handles of entities, and other indices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::enum_variant_names)] // ScrHandle: the database's name
pub enum Handle {
    Entity,
    Ped,
    Vehicle,
    Object,
    Player,
    Cam,
    Blip,
    Pickup,
    Interior,
    ScrHandle,
    FireId,
}

impl Handle {
    fn is_entity(self) -> bool {
        matches!(self, Handle::Entity | Handle::Ped | Handle::Vehicle | Handle::Object)
    }
}

/// Whether a parameter goes in, comes out (a pointer the native fills), or both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    In,
    Out,
    InOut,
}

#[derive(Debug)]
pub struct Param {
    pub kind: Kind,
    pub dir: Dir,
}

#[derive(Debug)]
pub struct Native {
    /// Index in [`NAMESPACES`].
    pub ns: u16,
    /// The name in Lua (`createVehicle`).
    pub lua: &'static str,
    /// The game's name (`CREATE_VEHICLE`).
    pub name: &'static str,
    /// The hash this game build registers it with.
    pub hash: u64,
    /// The natives database's hash, the one ScriptHookV's mods use (0: none).
    pub db_hash: u64,
    /// The handler's function table index.
    pub handler: u64,
    pub params: &'static [Param],
    pub ret: Option<Kind>,
    /// The natives database's notes.
    pub doc: &'static str,
}

/// A native of the game or of the database that the mods cannot call.
#[derive(Debug)]
pub struct Excluded {
    pub name: &'static str,
    /// The database's hash (0: none).
    pub db_hash: u64,
    /// Why, as the build script says it.
    pub why: &'static str,
}

use {Dir as D, Handle as H, Kind as K, Param as P};
include!(concat!(env!("OUT_DIR"), "/natives.rs"));

/// The natives' Luau definitions, for the type checker.
pub const DEFINITIONS: &str = include_str!(concat!(env!("OUT_DIR"), "/natives.d.luau"));

/// What the build found: how many natives there are, which were left out.
pub const SUMMARY: &str = env!("GLUE5_MODS_NATIVES_SUMMARY");

/// The handler of a native the mods may not call (`WAIT`).
pub fn reserved_handler(name: &str) -> Option<u64> {
    RESERVED.iter().find(|(n, _)| *n == name).map(|&(_, h)| h)
}

/// The native `lua` of namespace `ns` (NATIVES is sorted by both).
pub fn find(ns: usize, lua: &str) -> Option<usize> {
    let ns = ns as u16;
    NATIVES.binary_search_by(|n| (n.ns, n.lua).cmp(&(ns, lua))).ok()
}

/// What a name or a hash stands for: a native, one only `native.invoke`
/// calls, or one left out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    /// Index in [`NATIVES`].
    Native(usize),
    /// Index in [`RAW`].
    Raw(usize),
    /// Index in [`EXCLUDED`].
    Excluded(usize),
}

impl Target {
    fn native(self) -> Option<&'static Native> {
        match self {
            Target::Native(i) => Some(&NATIVES[i]),
            Target::Raw(i) => Some(&RAW[i]),
            Target::Excluded(_) => None,
        }
    }
}

/// The natives by the names and hashes ScriptHookV's mods use.
struct Lookup {
    /// By the game's name.
    names: HashMap<&'static str, Target>,
    /// By the database's hash.
    hashes: HashMap<u64, Target>,
    /// By the database's hash as a Luau number has it (the bits of a
    /// double, rounded past 2^53: no two hashes round alike, see the tests).
    numbers: HashMap<u64, Target>,
}

static LOOKUP: LazyLock<Lookup> = LazyLock::new(|| {
    let mut l = Lookup { names: HashMap::new(), hashes: HashMap::new(), numbers: HashMap::new() };
    let excluded = EXCLUDED.iter().enumerate().map(|(i, e)| (e.name, e.db_hash, Target::Excluded(i)));
    let natives = NATIVES.iter().enumerate().map(|(i, n)| (n.name, n.db_hash, Target::Native(i)));
    let raw = RAW.iter().enumerate().map(|(i, n)| (n.name, n.db_hash, Target::Raw(i)));
    for (name, hash, target) in excluded.chain(natives).chain(raw) {
        l.names.insert(name, target);
        if hash != 0 {
            l.hashes.insert(hash, target);
            l.numbers.insert((hash as f64).to_bits(), target);
        }
    }
    l
});

/// Why a native cannot be called at all, for people.
fn why_excluded(why: &str) -> &'static str {
    match why {
        "reserved" => "it is the script thread's own: a mod waits with wait() and ends when its code returns",
        "not in this build" => "this game build does not have it",
        _ => "the types of its parameters are unknown",
    }
}

/// The native a hash (a number, or a `0x...` string) or a game name names,
/// as `native.invoke` takes them.
fn resolve(target: &Value) -> Result<&'static Native, String> {
    let l = &*LOOKUP;
    let (found, what) = match target {
        Value::Integer(_) | Value::Number(_) => {
            let n = number(target).unwrap_or_default();
            (l.numbers.get(&n.to_bits()), format!("with the hash {:#018X}", n as u64))
        }
        Value::String(s) => {
            let s = s.to_string_lossy();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(hex) => {
                    let hash = u64::from_str_radix(hex, 16).map_err(|_| format!("native.invoke: {s:?} is not a hash"))?;
                    (l.hashes.get(&hash), format!("with the hash {hash:#018X}"))
                }
                None => {
                    let name = s.to_ascii_uppercase();
                    (l.names.get(name.as_str()), format!("named {name}"))
                }
            }
        }
        _ => return Err("native.invoke: argument 1 must be a native's hash or name".into()),
    };
    match found {
        Some(&Target::Excluded(i)) => {
            let e = &EXCLUDED[i];
            Err(format!("native.invoke: {} is not available to mods: {}", e.name, why_excluded(e.why)))
        }
        Some(t) => Ok(t.native().expect("not excluded")),
        None => Err(format!("native.invoke: no native {what} (hashes are ScriptHookV's, from the natives database)")),
    }
}

/// The natives' tables, one per namespace (`vehicle.createVehicle`: each
/// native once, by its Luau name), and `native.invoke` (by a hash or the
/// game's name, for natives the tables leave out). `task`'s functions (the
/// scheduler) go in the TASK_ natives' table.
pub fn install(lua: &Lua, task: &Table) -> mlua::Result<()> {
    let g = lua.globals();
    for (ns, space) in NAMESPACES.iter().enumerate() {
        let table = namespace(lua, move |key| find(ns, key))?;
        if *space == "task" {
            for pair in task.pairs::<Value, Value>() {
                let (k, v) = pair?;
                table.raw_set(k, v)?;
            }
        }
        g.set(*space, table)?;
    }
    let native = lua.create_table()?;
    native.set(
        "invoke",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let n = resolve(&args.next().unwrap_or(Value::Nil)).map_err(mlua::Error::runtime)?;
            call_native(lua, n, args.collect(), true)
        })?,
    )?;
    lua.globals().set("native", native)
}

/// A namespace's table: a native's function is made the first time it is
/// looked up.
fn namespace(lua: &Lua, find: impl Fn(&str) -> Option<usize> + 'static) -> mlua::Result<Table> {
    let cache: RefCell<HashMap<String, Function>> = RefCell::default();
    let table = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(move |lua, (_, key): (Table, String)| {
            if let Some(f) = cache.borrow().get(&key) {
                return Ok(Value::Function(f.clone()));
            }
            let Some(index) = find(&key) else { return Ok(Value::Nil) };
            let f = lua.create_function(move |lua, args: MultiValue| call(lua, index, args))?;
            cache.borrow_mut().insert(key, f.clone());
            Ok(Value::Function(f))
        })?,
    )?;
    table.set_metatable(Some(meta))?;
    Ok(table)
}

thread_local! {
    /// The engine thread's environment while a mod runs: natives can only
    /// be called then.
    static ENV: Cell<*mut Env> = const { Cell::new(std::ptr::null_mut()) };
}

/// Runs `f` with natives callable through `env`; then frees the text
/// commands' labels the engine is done with.
pub fn with_env<R>(env: *mut Env, f: impl FnOnce() -> R) -> R {
    let old = ENV.replace(env);
    let r = f();
    // (The environment `ENV` holds is the engine thread's while it runs.)
    unsafe { release_labels(ENV.get()) };
    ENV.set(old);
    r
}

/// GTA Online: a mod's natives run as the session's own script (freemode,
/// which every player runs), so that what they make is networked as the
/// game's scripts' is (engine.rs). The engine keeps the running script
/// thread in thread-local storage: it is the session script's for the mod's
/// turn, the mod's own again after.
#[derive(Clone, Copy)]
pub(crate) struct Session {
    /// Where the engine keeps the running script thread.
    pub(crate) slot: u64,
    /// The mod's own thread, and the session script's.
    pub(crate) own: u64,
    pub(crate) host: u64,
}

thread_local! {
    static SESSION: Cell<Option<Session>> = const { Cell::new(None) };
    /// What each mod made as the session's script (entities, blips,
    /// pickups), by mod: given back when it stops, as the engine gives
    /// back a script's own when it ends.
    static MADE: RefCell<HashMap<String, Vec<(Handle, i32)>>> = RefCell::new(HashMap::new());
}

/// Runs `f` as the session's script, when there is one; the running thread
/// is the mod's own again after (a panic too).
pub(crate) fn as_session<R>(env: *mut Env, session: Option<Session>, f: impl FnOnce() -> R) -> R {
    let Some(s) = session else { return f() };
    struct Back(Mem, Session);
    impl Drop for Back {
        fn drop(&mut self) {
            self.0.set_u64(self.1.slot, self.1.own);
            SESSION.set(None);
        }
    }
    let mem = unsafe { Mem::of(env) };
    mem.set_u64(s.slot, s.host);
    SESSION.set(Some(s));
    let _back = Back(mem, s);
    f()
}

/// Natives about the running script itself (its name, id, host,
/// participants): the mod's own, in GTA Online too.
fn about_this_script(name: &str) -> bool {
    name.contains("THIS_SCRIPT") || name.contains("THIS_THREAD") || name.starts_with("PARTICIPANT_ID")
}

/// Whether native `name` makes something for the running script that
/// returns as `h`: an entity, a blip, a pickup.
fn makes(name: &str, h: Handle) -> bool {
    let making = name.starts_with("CREATE_") || name.starts_with("CLONE_") || name.starts_with("ADD_BLIP_");
    making && (h.is_entity() || matches!(h, Handle::Blip | Handle::Pickup))
}

/// Gives back what mod `name` made as the session's script (it stopped):
/// its entities no longer needed, its blips and pickups removed, as the
/// engine does for a script that ends. Runs as `host` (the session script,
/// if it still runs: else the game took them back with it), then `active`
/// is the running thread again.
///
/// # Safety
/// `env` is the engine thread's, inside a script's update.
pub(crate) unsafe fn give_back(env: *mut Env, name: &str, slot: u64, host: Option<u64>, active: u64) {
    let Some(made) = MADE.with_borrow_mut(|m| m.remove(name)) else { return };
    let Some(host) = host else { return };
    let mem = unsafe { Mem::of(env) };
    mem.set_u64(slot, host);
    for (h, id) in made {
        let (exists, give) = match h {
            Handle::Blip => ("DOES_BLIP_EXIST", "REMOVE_BLIP"),
            Handle::Pickup => ("DOES_PICKUP_EXIST", "REMOVE_PICKUP"),
            _ => ("DOES_ENTITY_EXIST", "SET_ENTITY_AS_NO_LONGER_NEEDED"),
        };
        if unsafe { call_with(env, exists, id) } == Some(1) {
            unsafe { call_with(env, give, id) };
        }
    }
    mem.set_u64(slot, active);
}

/// The session script ended (or another began): what mods made as it went
/// with it.
pub(crate) fn forget_made() {
    MADE.with_borrow_mut(HashMap::clear);
}

/// Calls a native that takes one handle (by value, or by reference as
/// SET_ENTITY_AS_NO_LONGER_NEEDED does) from Rust; its int result.
unsafe fn call_with(env: *mut Env, name: &str, handle: i32) -> Option<i32> {
    static BY_NAME: LazyLock<HashMap<&str, &Native>> = LazyLock::new(|| NATIVES.iter().map(|n| (n.name, n)).collect());
    let n = *BY_NAME.get(name)?;
    let by_ref = n.params.first()?.dir != Dir::In;
    let mem = unsafe { Mem::of(env) };
    let top = unsafe { exports::emscripten_stack_get_current(env) }.ok()?;
    let r = (|| {
        let info = unsafe { exports::_emscripten_stack_alloc(env, 160) }.ok()?;
        mem.bytes_mut(info, 160).fill(0);
        let (result, params, cell) = (info + 128, info + 136, info + 144);
        mem.set_i32(cell, handle);
        mem.set_u64(params, if by_ref { cell } else { handle as u32 as u64 });
        mem.set_u64(info, result);
        mem.set_u32(info + 8, 1);
        mem.set_u64(info + 16, params);
        unsafe { glue5_game::call_native(env, n.handler, info) }.ok()?;
        Some(mem.i32(result))
    })();
    let _ = unsafe { exports::_emscripten_stack_restore(env, top) };
    r
}

/// Natives that keep their string past the call: a text command's label.
/// `CScriptTextConstruction::Begin*` keeps the pointer (when no command is
/// open), its END_TEXT_COMMAND_* reads the text through it; the strings of
/// the components in between are copied.
fn keeps_string(name: &str) -> bool {
    name.starts_with("BEGIN_TEXT_COMMAND_")
}

/// The open text command's kind (u32, 0: none) and its label (u64).
const TEXT_COMMAND: u64 = 23856328;
const TEXT_LABEL: u64 = 23856056;

thread_local! {
    /// The labels given to text commands, in the game's heap: kept while
    /// the engine may read them.
    static LABELS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// A copy of `bytes` (and a NUL) that outlives the call, for a text
/// command's label.
unsafe fn keep_label(env: *mut Env, bytes: &[u8]) -> Option<u64> {
    unsafe { release_labels(env) };
    let at = unsafe { orig::builtin_malloc(env, bytes.len() as u64 + 1) };
    if at == 0 {
        return None;
    }
    let mem = unsafe { Mem::of(env) };
    mem.bytes_mut(at, bytes.len()).copy_from_slice(bytes);
    mem.set_u8(at + bytes.len() as u64, 0);
    LABELS.with_borrow_mut(|l| l.push(at));
    Some(at)
}

/// Frees the labels the engine no longer reads: all but the open
/// command's.
unsafe fn release_labels(env: *mut Env) {
    if LABELS.with_borrow(Vec::is_empty) {
        return;
    }
    let mem = unsafe { Mem::of(env) };
    let open = (mem.u32(TEXT_COMMAND) != 0).then(|| mem.u64(TEXT_LABEL));
    let done: Vec<u64> = LABELS.with_borrow_mut(|l| {
        let (keep, done) = l.drain(..).partition(|&at| Some(at) == open);
        *l = keep;
        done
    });
    for at in done {
        unsafe { orig::builtin_free(env, at) };
    }
}

/// Jenkins' one-at-a-time hash of the lowercase string (`GET_HASH_KEY`).
pub fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_add(b.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// The script handles' pool (`rage::fwScriptGuid`), u64 at this address.
const GUID_POOL: u64 = 12318120;

/// The type of the live entity `handle` names (3 vehicle, 4 ped, 5
/// object, `CEntity` +40), none if it names none.
fn entity_type(mem: Mem, handle: i32) -> Option<u8> {
    entity_address(mem, handle).map(|entity| mem.u8(entity + 40))
}

/// The address of the live entity (`CEntity`) `handle` names, none if it
/// names none: what `fwScriptGuid::GetBaseFromGuid` checks, without its
/// `unreachable` for an index past the pool.
pub fn entity_address(mem: Mem, handle: i32) -> Option<u64> {
    if handle == -1 || handle == 0 {
        return None;
    }
    let pool = mem.u64(GUID_POOL);
    if pool == 0 {
        return None;
    }
    let index = (handle >> 8) as u32;
    if index >= mem.u32(pool + 32) || mem.u8(mem.u64(pool + 24) + u64::from(index)) != handle as u8 {
        return None;
    }
    let guid = mem.u64(pool + 16) + u64::from(mem.u32(pool + 36)) * u64::from(index);
    if guid == 0 {
        return None;
    }
    match mem.u64(guid + 8) {
        0 => None,
        entity => Some(entity),
    }
}

fn type_name(t: u8) -> &'static str {
    match t {
        3 => "Vehicle",
        4 => "Ped",
        5 => "Object",
        _ => "Entity",
    }
}

/// The current environment, for what mods call from Rust (world.rs).
pub fn env() -> Option<*mut Env> {
    let e = ENV.get();
    (!e.is_null()).then_some(e)
}

/// The native running now (null: none), for the failure note.
static RUNNING: std::sync::atomic::AtomicPtr<Native> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// The native running now, for the failure note (`vehicle.createVehicle`).
pub fn running() -> Option<String> {
    let n = RUNNING.load(std::sync::atomic::Ordering::Relaxed);
    (!n.is_null()).then(|| qualified(unsafe { &*n }))
}

/// A trap happened in a native: the engine may be in a bad state.
pub static TAINTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Calls a native that takes nothing and gives an int (`GET_GAME_TIMER`),
/// from Rust, in the current script context; the reserved ones too.
///
/// # Safety
/// `env` is the engine thread's, inside a script's update.
pub unsafe fn call_int(env: *mut Env, name: &str) -> Option<i32> {
    // By name, once: every script's WAIT asks for some (thousands a second).
    static NO_PARAMS: std::sync::LazyLock<std::collections::HashMap<&str, u64>> = std::sync::LazyLock::new(|| {
        let reserved = RESERVED.iter().map(|&(n, h)| (n, h));
        reserved.chain(NATIVES.iter().filter(|n| n.params.is_empty()).map(|n| (n.name, n.handler))).collect()
    });
    let handler = *NO_PARAMS.get(name)?;
    let mem = unsafe { Mem::of(env) };
    let top = unsafe { exports::emscripten_stack_get_current(env) }.ok()?;
    let r = (|| {
        let info = unsafe { exports::_emscripten_stack_alloc(env, 160) }.ok()?;
        mem.bytes_mut(info, 160).fill(0);
        let result = info + 128;
        mem.set_u64(info, result);
        mem.set_u64(info + 16, info + 136);
        unsafe { glue5_game::call_native(env, handler, info) }.ok()?;
        Some(mem.i32(result))
    })();
    let _ = unsafe { exports::_emscripten_stack_restore(env, top) };
    r
}

/// The full name of native `n`, for messages (`vehicle.createVehicle`;
/// the game's name for one only `native.invoke` calls).
pub fn qualified(n: &Native) -> String {
    match NAMESPACES.get(n.ns as usize) {
        Some(space) if NATIVES.as_ptr_range().contains(&(n as *const Native)) => format!("{space}.{}", n.lua),
        _ => n.name.to_string(),
    }
}

/// The longest string a native takes from Lua.
const MAX_STRING: usize = 16 << 10;

/// Calls native `index` with Lua's arguments.
pub fn call(lua: &Lua, index: usize, args: MultiValue) -> mlua::Result<MultiValue> {
    call_native(lua, &NATIVES[index], args, false)
}

/// Calls native `n` with Lua's arguments; `raw`: ScriptHookV's way, with
/// pointers as addresses (nothing comes back through them as results).
fn call_native(lua: &Lua, n: &'static Native, args: MultiValue, raw: bool) -> mlua::Result<MultiValue> {
    let env = ENV.get();
    if env.is_null() {
        return Err(mlua::Error::runtime(format!("{}: natives can only run in a mod's script", qualified(n))));
    }
    let mem = unsafe { Mem::of(env) };
    let trap = |e| mlua::Error::runtime(format!("{}: {e}", qualified(n)));
    let top = unsafe { exports::emscripten_stack_get_current(env) }.map_err(trap)?;
    let result = (|| {
        let alloc = |size: u64| -> mlua::Result<u64> {
            let at = unsafe { exports::_emscripten_stack_alloc(env, size.next_multiple_of(16)) }.map_err(trap)?;
            mem.bytes_mut(at, size as usize).fill(0);
            Ok(at)
        };
        let slots: u64 = n.params.iter().map(|p| if p.kind == K::Vec3 && p.dir == D::In { 3 } else { 1 }).sum();
        let info = alloc(128)?;
        let params = alloc(slots.max(1) * 8)?;
        let result = alloc(32)?;
        let mut outputs = Vec::new();
        // Buffers passed for pointers: copied in, and back after the call.
        let mut buffers: Vec<(u64, mlua::Buffer)> = Vec::new();
        let mut buffer = |b: mlua::Buffer, i: usize| -> mlua::Result<u64> {
            if b.len() > MAX_STRING {
                let e = format!("{}: argument {}: a buffer of at most {} KB (or memory.alloc)", qualified(n), i + 1, MAX_STRING >> 10);
                return Err(mlua::Error::runtime(e));
            }
            // Room for a value or a vector at least: one value's pointer may
            // be written as 8 bytes, or 24.
            let at = alloc(b.len().max(24) as u64)?;
            mem.bytes_mut(at, b.len()).copy_from_slice(&b.to_vec());
            buffers.push((at, b));
            Ok(at)
        };
        let mut args = args.into_iter();
        let mut slot = params;
        for (i, p) in n.params.iter().enumerate() {
            let bad = |what: &str| {
                mlua::Error::runtime(format!("{}: argument {} must be {what}", qualified(n), i + 1))
            };
            if raw && p.dir != D::In {
                let at = match args.next().unwrap_or(Value::Nil) {
                    Value::Nil => 0,
                    Value::Buffer(b) => buffer(b, i)?,
                    // A value of a vector is three.
                    v => crate::memory::address(env, &v, if p.kind == K::Vec3 { 24 } else { 8 }).map_err(|e| mlua::Error::runtime(format!("{}: argument {}: {e}", qualified(n), i + 1)))?,
                };
                mem.set_u64(slot, at);
                slot += 8;
                continue;
            }
            let value = if p.dir == D::Out { Value::Nil } else { args.next().unwrap_or(Value::Nil) };
            let target = if p.dir == D::In {
                slot
            } else {
                // A pointer to a buffer the native fills: scrValues, 8 bytes each.
                let buf = alloc(24)?;
                mem.set_u64(slot, buf);
                outputs.push((buf, p.kind));
                buf
            };
            if p.dir == D::Out {
                slot += 8;
                continue;
            }
            match p.kind {
                K::Int => mem.set_i32(target, int(&value).ok_or_else(|| bad("a number"))?),
                K::Any => match value {
                    Value::Boolean(b) => mem.set_i32(target, b as i32),
                    Value::Nil => {}
                    ref v => mem.set_i32(target, int(v).ok_or_else(|| bad("a number or a boolean"))?),
                },
                K::Float => mem.set_u32(target, (number(&value).ok_or_else(|| bad("a number"))? as f32).to_bits()),
                K::Bool => match value {
                    Value::Boolean(b) => mem.set_i32(target, b as i32),
                    _ => return Err(bad("a boolean")),
                },
                K::Hash => match &value {
                    Value::String(s) => mem.set_u32(target, joaat(&s.to_str()?)),
                    v => mem.set_u32(target, number(v).ok_or_else(|| bad("a hash or a name"))? as i64 as u32),
                },
                K::Str => match &value {
                    Value::Buffer(b) if raw => mem.set_u64(target, buffer(b.clone(), i)?),
                    // A buffer of the mod's.
                    Value::Integer(_) | Value::Number(_) if raw => {
                        mem.set_u64(target, crate::memory::address(env, &value, 1).map_err(|e| mlua::Error::runtime(format!("{}: argument {}: {e}", qualified(n), i + 1)))?)
                    }
                    Value::String(s) => {
                        let bytes = s.as_bytes();
                        if bytes.contains(&0) {
                            return Err(bad("a string without NUL"));
                        }
                        // It goes on the engine thread's stack, which has
                        // no room for megabytes (no native takes them).
                        if bytes.len() > MAX_STRING {
                            return Err(bad(&format!("a string of at most {} KB", MAX_STRING >> 10)));
                        }
                        let at = if keeps_string(n.name) {
                            unsafe { keep_label(env, &bytes) }.ok_or_else(|| bad("a string the game has memory for"))?
                        } else {
                            let at = alloc(bytes.len() as u64 + 1)?;
                            mem.bytes_mut(at, bytes.len()).copy_from_slice(&bytes);
                            at
                        };
                        mem.set_u64(target, at);
                    }
                    Value::Nil => {}
                    _ => return Err(bad("a string")),
                },
                K::Vec3 => {
                    let v = match value {
                        Value::Vector(v) => [v.x(), v.y(), v.z()],
                        // ScriptHookV's way: x, y, z.
                        ref x if number(x).is_some() => {
                            let (y, z) = (args.next().unwrap_or(Value::Nil), args.next().unwrap_or(Value::Nil));
                            match (number(x), number(&y), number(&z)) {
                                (Some(x), Some(y), Some(z)) => [x as f32, y as f32, z as f32],
                                _ => return Err(bad("a vector, or three numbers")),
                            }
                        }
                        _ => return Err(bad("a vector")),
                    };
                    for (k, c) in v.into_iter().enumerate() {
                        mem.set_u32(target + 8 * k as u64, c.to_bits());
                    }
                }
                K::Handle(h) => {
                    let id = match value {
                        Value::Nil => 0,
                        ref v => int(v).ok_or_else(|| bad(&format!("a {h:?}")))?,
                    };
                    // DOES_ENTITY_EXIST and the like are how a mod checks one.
                    let checks = n.name.starts_with("DOES_") && n.name.ends_with("_EXIST");
                    // None, where the native allows it (the engine finds no
                    // entity for 0, and a pool always has an index 0).
                    if id == 0 && p.dir == D::In {
                        mem.set_i32(target, 0);
                        slot += 8;
                        continue;
                    }
                    // One passed by reference (DELETE_ENTITY, ..._AS_NO_LONGER_NEEDED)
                    // that is gone already: nothing to do (the engine would
                    // trap on a handle past its pool).
                    if h.is_entity() && p.dir == D::InOut && entity_type(mem, id).is_none() {
                        return Ok(MultiValue::new());
                    }
                    if h.is_entity() && p.dir != D::Out && !checks {
                        let want = match h {
                            H::Vehicle => Some(3),
                            H::Ped => Some(4),
                            H::Object => Some(5),
                            _ => None,
                        };
                        match entity_type(mem, id) {
                            None => {
                                let got = if value.is_nil() { "nil".to_string() } else { id.to_string() };
                                return Err(bad(&format!("an existing {h:?} (got {got})")));
                            }
                            Some(t) if want.is_some_and(|w| w != t) => {
                                return Err(bad(&format!("a {h:?} (got a {})", type_name(t))));
                            }
                            _ => {}
                        }
                    }
                    mem.set_i32(target, id);
                }
            }
            slot += if p.kind == K::Vec3 && p.dir == D::In { 24 } else { 8 };
        }
        // A mod's script thread is ended by its mod, not by a native
        // (engine.rs); the game's own threads may be.
        let mods_thread = match n.name {
            "TERMINATE_THREAD" => crate::engine::is_mod_thread(mem.u32(params)),
            // By name, as the engine compares them (case aside).
            "TERMINATE_ALL_SCRIPTS_WITH_THIS_NAME" => {
                mem.str(mem.u64(params)).to_ascii_lowercase().starts_with(crate::engine::PREFIX)
            }
            _ => false,
        };
        if mods_thread {
            return Err(mlua::Error::runtime(format!(
                "{}: that is a mod's script thread; a mod ends when its code returns, or from the Mods page",
                qualified(n)
            )));
        }
        // A mod's script is no network script of the game's (they are
        // listed in its script data): the engine stops the game when one
        // asks to be one.
        if n.name.starts_with("NETWORK_SET_THIS_SCRIPT_IS_NETWORK_SCRIPT") || n.name == "NETWORK_TRY_TO_SET_THIS_SCRIPT_IS_NETWORK_SCRIPT" {
            return Err(mlua::Error::runtime(format!(
                "{}: a mod's script cannot be a network script (the game stops for it); what it makes is this player's own",
                qualified(n)
            )));
        }
        // The engine fails when a pool is full: refused before that
        // (CREATE_PED, CLONE_PED, CREATE_VEHICLE...).
        if n.name.starts_with("CREATE_") || n.name.starts_with("CLONE_") {
            let pool = match n.ret {
                Some(K::Handle(H::Vehicle)) => Some(crate::world::Pool::Vehicles),
                Some(K::Handle(H::Ped)) => Some(crate::world::Pool::Peds),
                Some(K::Handle(H::Object)) => Some(crate::world::Pool::Objects),
                _ => None,
            };
            if let Some(pool) = pool
                && crate::world::free(mem, pool) < 8
            {
                return Err(mlua::Error::runtime(format!(
                    "{}: the game has no room for more ({pool:?} are full); delete some first",
                    qualified(n)
                )));
            }
        }
        mem.set_u64(info, result);
        mem.set_u32(info + 8, slots as u32);
        mem.set_u64(info + 16, params);
        mem.set_i32(info + 24, 0);
        RUNNING.store((n as *const Native).cast_mut(), std::sync::atomic::Ordering::Relaxed);
        // As the session's script, but about the running script: the mod's.
        let own = SESSION.get().filter(|_| about_this_script(n.name));
        if let Some(s) = own {
            mem.set_u64(s.slot, s.own);
        }
        let called = unsafe { glue5_game::call_native(env, n.handler, info) };
        if let Some(s) = own {
            mem.set_u64(s.slot, s.host);
        }
        RUNNING.store(std::ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = called {
            TAINTED.store(true, std::sync::atomic::Ordering::Relaxed);
            return Err(mlua::Error::runtime(format!("{} stopped with a {e}", qualified(n))));
        }
        for (at, b) in buffers {
            b.write_bytes(0, mem.bytes(at, b.len()));
        }
        // Vectors the native took by reference go back where they came from.
        for k in 0..mem.i32(info + 24).clamp(0, 4) as u64 {
            let orig = mem.u64(info + 32 + 8 * k);
            let buf = info + 64 + 16 * k;
            for c in 0..3 {
                mem.set_u32(orig + 8 * c, mem.u32(buf + 4 * c));
            }
        }
        // Made as the session's script: given back when the mod stops.
        if SESSION.get().is_some()
            && let Some(K::Handle(h)) = n.ret
            && makes(n.name, h)
            && mem.i32(result) != 0
        {
            let name = crate::lock(&crate::RUNNING_MOD).clone();
            MADE.with_borrow_mut(|m| m.entry(name).or_default().push((h, mem.i32(result))));
        }
        let mut out = Vec::new();
        if let Some(k) = n.ret {
            out.push(read(lua, mem, result, k)?);
        }
        for (buf, k) in outputs {
            out.push(read(lua, mem, buf, k)?);
        }
        Ok(MultiValue::from_vec(out))
    })();
    unsafe { exports::_emscripten_stack_restore(env, top) }.map_err(trap)?;
    result
}

fn number(v: &Value) -> Option<f64> {
    match *v {
        Value::Integer(i) => Some(i as f64),
        Value::Number(n) => Some(n),
        _ => None,
    }
}

fn int(v: &Value) -> Option<i32> {
    let n = number(v)?;
    (n.fract() == 0.0 && n >= i32::MIN as f64 && n <= u32::MAX as f64).then_some(n as i64 as i32)
}

/// A value the native left at `at`.
fn read(lua: &Lua, mem: Mem, at: u64, k: Kind) -> mlua::Result<Value> {
    Ok(match k {
        K::Int | K::Any => Value::Integer(mem.i32(at).into()),
        K::Hash => Value::Integer(mem.u32(at).into()),
        K::Float => Value::Number(f32::from_bits(mem.u32(at)).into()),
        K::Bool => Value::Boolean(mem.i32(at) != 0),
        K::Str => match mem.u64(at) {
            0 => Value::Nil,
            p => Value::String(lua.create_string(mem.str(p))?),
        },
        K::Vec3 => {
            let c = |i: u64| f32::from_bits(mem.u32(at + 8 * i));
            Value::Vector(mlua::Vector::new(c(0), c(1), c(2)))
        }
        // Player 0 is the local player; for the others 0 is none.
        K::Handle(H::Player) => Value::Integer(mem.i32(at).into()),
        K::Handle(_) => match mem.i32(at) {
            0 => Value::Nil,
            id => Value::Integer(id.into()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_rules() {
        // About the running script: the mod's own, as the session's too.
        assert!(about_this_script("GET_THIS_SCRIPT_NAME"));
        assert!(about_this_script("NETWORK_IS_HOST_OF_THIS_SCRIPT"));
        assert!(about_this_script("GET_ID_OF_THIS_THREAD"));
        assert!(about_this_script("PARTICIPANT_ID_TO_INT"));
        assert!(!about_this_script("CREATE_VEHICLE"));
        // What a native makes for the script, given back when the mod stops.
        assert!(makes("CREATE_VEHICLE", Handle::Vehicle));
        assert!(makes("CLONE_PED", Handle::Ped));
        assert!(makes("ADD_BLIP_FOR_COORD", Handle::Blip));
        assert!(makes("CREATE_PICKUP", Handle::Pickup));
        assert!(!makes("GET_CLOSEST_VEHICLE", Handle::Vehicle));
        assert!(!makes("CREATE_CAM", Handle::Cam));
    }

    fn named(lua: &Lua, s: &str) -> Value {
        Value::String(lua.create_string(s).unwrap())
    }

    /// ScriptHookV's hashes (nativedb's) to this build's natives, the pairs
    /// as natives.json and game.wasm's table have them.
    #[test]
    fn hashes() {
        let lua = Lua::new();
        for (db, ours, name) in [
            (0xAF35D0D2583051B0u64, 0xBA715A7BEBA5A1F9u64, "CREATE_VEHICLE"),
            (0xD80958FC74E988A6, 0xE2D3D51028F0428A, "PLAYER_PED_ID"),
            (0x3FEF770D40960D5A, 0x6B62510F212526DC, "GET_ENTITY_COORDS"),
        ] {
            // As Luau has the hex literal: a double, rounded.
            for v in [Value::Number(db as f64), named(&lua, &format!("{db:#X}")), named(&lua, name), named(&lua, &name.to_lowercase())] {
                let n = resolve(&v).unwrap_or_else(|e| panic!("{v:?}: {e}"));
                assert_eq!((n.name, n.hash, n.db_hash), (name, ours, db));
            }
        }
        // No two hashes are the same double.
        assert_eq!(LOOKUP.numbers.len(), LOOKUP.hashes.len());
        assert!(LOOKUP.hashes.len() > 6000, "{}", LOOKUP.hashes.len());
    }

    /// Natives the namespaces leave out: `native.invoke` calls them, but for
    /// the script thread's own and those this build lacks.
    #[test]
    fn left_out() {
        let lua = Lua::new();
        for name in ["TASK_SCRIPTED_ANIMATION", "COPY_SCRIPT_STRUCT", "START_SAVE_DATA", "GET_EVENT_DATA"] {
            let n = resolve(&named(&lua, name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(RAW.as_ptr_range().contains(&(n as *const Native)), "{name}");
            assert_eq!(qualified(n), name);
        }
        let n = resolve(&Value::Number(0x126EF75F1E17ABE5u64 as f64)).unwrap();
        assert_eq!(n.name, "TASK_SCRIPTED_ANIMATION");
        let e = resolve(&named(&lua, "WAIT")).unwrap_err();
        assert!(e.contains("not available") && e.contains("wait()"), "{e}");
        assert!(resolve(&Value::Number(0x1234 as f64)).unwrap_err().contains("no native with the hash 0x0000000000001234"));
        assert!(resolve(&named(&lua, "NOT_A_NATIVE")).unwrap_err().contains("no native named NOT_A_NATIVE"));
        assert!(resolve(&named(&lua, "0xZZ")).unwrap_err().contains("not a hash"));
        assert!(resolve(&Value::Boolean(true)).is_err());
    }

    /// A native takes what its handler reads (the script signature), not
    /// what its typed command or the database say where they differ.
    #[test]
    fn script_arity() {
        let native = |name: &str| NATIVES.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("{name}"));
        let floats = |name: &str| native(name).params.iter().map(|p| p.kind == K::Float).collect::<Vec<_>>();
        // Both share a helper of 11 values; the handlers read 10 and 6.
        let (f, t) = (false, true);
        assert_eq!(floats("DRAW_SCALEFORM_MOVIE"), [f, t, t, t, t, f, f, f, f, f]);
        assert_eq!(floats("DRAW_SCALEFORM_MOVIE_FULLSCREEN"), [f; 6]);
        assert_eq!(native("REQUEST_SCALEFORM_MOVIE").params.len(), 1);
        assert_eq!(native("SET_BLIP_NAME_TO_PLAYER_NAME").params.len(), 2);
        // The database's five, the handler's four.
        assert_eq!(native("NETWORK_SPENT_TAXI").params.len(), 4);
        // A vector the signature cannot tell from an output: the database's.
        let out = native("GET_COORDS_OF_PROJECTILE_TYPE_WITHIN_DISTANCE").params.iter().find(|p| p.kind == K::Vec3);
        assert_eq!(out.map(|p| p.dir), Some(D::Out));
        // A structure's pointer the database calls a value: native.invoke only.
        assert!(!NATIVES.iter().any(|n| n.name == "PLAYSTATS_MISSION_ENDED"));
        assert!(RAW.iter().any(|n| n.name == "PLAYSTATS_MISSION_ENDED"));
    }

    /// Each native once, by its Luau name (`vehicle.createVehicle`); the
    /// game's names only through `native.invoke`.
    #[test]
    fn luau_names_only() {
        let lua = Lua::new();
        install(&lua, &lua.create_table().unwrap()).unwrap();
        let kind = |code: &str| lua.load(code).eval::<String>().unwrap();
        assert_eq!(kind("return type(vehicle.createVehicle)"), "function");
        assert_eq!(kind("return type(VEHICLE)"), "nil");
        assert_eq!(kind("return type(vehicle.CREATE_VEHICLE)"), "nil");
        let e = lua.load("vehicle.createVehicle('adder', vector.create(0, 0, 0), 0, false, false, false)").exec().unwrap_err();
        assert!(e.to_string().contains("vehicle.createVehicle: natives can only run in a mod's script"), "{e}");
        let e = lua.load("native.invoke(0xAF35D0D2583051B0, 'adder', 0, 0, 0, 0, false, false, false)").exec().unwrap_err();
        assert!(e.to_string().contains("natives can only run"), "{e}");
        let e = lua.load("native.invoke('WAIT', 0)").exec().unwrap_err();
        assert!(e.to_string().contains("not available"), "{e}");
    }

    /// The checker types the natives by their Luau names (the game's are
    /// unknown globals) and the ScriptHookV-style globals.
    #[test]
    fn natives_typed() {
        crate::checker::Checker::with(|c| {
            let file = "shv-test/main.luau";
            c.set(file, Some("--!strict\nlocal me = player.playerPedId()\nvehicle.setVehicleFixed(me)\n"));
            let text = c.check(file).to_string();
            assert!(text.contains("Ped") && text.contains("Vehicle"), "{text}");
            c.set(file, Some("--!strict\nVEHICLE.SET_VEHICLE_FIXED(nil)\n"));
            assert!(c.check(file).to_string().contains("VEHICLE"));
            let ok = "--!strict\nlocal car: Vehicle? = vehicle.createVehicle('adder', vector.create(0, 0, 0), 0, false, false, false)\n\
                      local r = native.invoke('0xAF35D0D2583051B0', 'adder', 0, 0, 0, 0, false, false, false)\n\
                      local n: number = globals.int(262145) + memory.readInt(memory.alloc(8))\n\
                      local v: string = game.version()\n\
                      globals.setString(1, 'x', 16)\nmemory.writeVector3(1, vector.create(1, 2, 3))\nprint(car, r, n, v, memory.entity(car))\n";
            c.set(file, Some(ok));
            assert_eq!(c.check(file).to_string(), "[]");
            let hover = c.hover(file, 1, 26);
            assert!(hover["type"].as_str().unwrap().contains("Vehicle"), "{hover}");
            c.set(file, None);
        })
        .unwrap();
        assert!(crate::checker::documentation("@glue5/global/vehicle.createVehicle").unwrap().contains("CREATE_VEHICLE"));
    }
}
