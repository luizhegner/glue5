//! `json.encode` and `json.decode`: Luau values as JSON text and back, for
//! mods (settings, the storage, later the network). A table whose keys are
//! 1..n is an array, an empty one too; other tables are objects, with
//! string keys (numbers are written as strings). `null` reads as `nil`.

use std::collections::BTreeMap;
use std::io::Write;

use mlua::{Lua, Table, Value};
use serde::Serialize;
use serde::ser::{Error as _, SerializeMap, SerializeSeq, Serializer};
use serde_json::Value as Json;

/// Tables nested deeper are refused (a table that holds itself).
const DEPTH: usize = 64;
/// The longest text `json.encode` makes, what `storage` keeps: a table that
/// holds another many times over would otherwise make text without end,
/// outside the mod's memory and time limits (this much takes some 60 ms
/// on an M1 Pro, in Rust, where the interrupt cannot stop it).
const MAX_TEXT: usize = 1 << 20;

/// Why a value has no JSON text.
#[derive(Debug)]
pub enum Error {
    /// Longer than the limit asked for.
    TooLong(usize),
    /// Not JSON: a function, a cycle, NaN...
    Value(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::TooLong(limit) => write!(f, "json: the text would be over {} KB", limit >> 10),
            Error::Value(e) => f.write_str(e),
        }
    }
}

/// The `json` table.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|_, (value, pretty): (Value, Option<bool>)| {
            encode(&value, pretty == Some(true), MAX_TEXT).map_err(|e| mlua::Error::runtime(e.to_string()))
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::LuaString| {
            let v: Json = serde_json::from_slice(&text.as_bytes())
                .map_err(|e| mlua::Error::runtime(format!("json.decode: {e}")))?;
            from_json(lua, &v)
        })?,
    )?;
    lua.globals().set("json", json)
}

/// A Luau value as JSON text of at most `limit` bytes. It is written as
/// it is walked, so the work stops with the text.
pub fn encode(value: &Value, pretty: bool, limit: usize) -> Result<String, Error> {
    let mut out = Limited { text: Vec::new(), limit };
    let written = if pretty {
        Node(value, 0).serialize(&mut serde_json::Serializer::pretty(&mut out))
    } else {
        Node(value, 0).serialize(&mut serde_json::Serializer::new(&mut out))
    };
    match written {
        Ok(()) => Ok(String::from_utf8(out.text).expect("JSON text is UTF-8")),
        Err(e) if e.is_io() => Err(Error::TooLong(limit)),
        Err(e) => Err(Error::Value(e.to_string())),
    }
}

/// The text so far; a write past the limit fails.
struct Limited {
    text: Vec<u8>,
    limit: usize,
}

impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.text.len() + bytes.len() > self.limit {
            return Err(std::io::Error::other("too long"));
        }
        self.text.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A value at a depth of tables.
struct Node<'a>(&'a Value, usize);

impl Serialize for Node<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Nil => s.serialize_unit(),
            Value::Boolean(b) => s.serialize_bool(*b),
            Value::Integer(i) => s.serialize_i64(*i),
            Value::Number(n) => number(s, *n),
            Value::String(text) => s.serialize_str(&text.to_str().map_err(|_| S::Error::custom("json: a string that is not UTF-8"))?),
            // Single precision: written with as many digits as it holds.
            Value::Vector(v) => {
                let mut seq = s.serialize_seq(Some(3))?;
                for c in [v.x(), v.y(), v.z()] {
                    seq.serialize_element(&Float(c.to_string().parse().unwrap_or(0.0)))?;
                }
                seq.end()
            }
            Value::Table(t) => table(s, t, self.1),
            other => Err(S::Error::custom(format!("json: a {} is not JSON", other.type_name()))),
        }
    }
}

struct Float(f64);

impl Serialize for Float {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        number(s, self.0)
    }
}

/// Whole numbers without a fraction (`3`, not `3.0`).
fn number<S: Serializer>(s: S, n: f64) -> Result<S::Ok, S::Error> {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        s.serialize_i64(n as i64)
    } else if n.is_finite() {
        s.serialize_f64(n)
    } else {
        Err(S::Error::custom("json: NaN and infinity are not JSON"))
    }
}

fn table<S: Serializer>(s: S, t: &Table, depth: usize) -> Result<S::Ok, S::Error> {
    if depth >= DEPTH {
        return Err(S::Error::custom("json: tables nested too deep (does a table hold itself?)"));
    }
    let len = t.raw_len();
    let mut pairs = Vec::new();
    for pair in t.pairs::<Value, Value>() {
        pairs.push(pair.map_err(|e| S::Error::custom(e.to_string()))?);
    }
    // 1..n and nothing else: an array.
    if pairs.len() == len {
        let mut seq = s.serialize_seq(Some(len))?;
        for i in 1..=len {
            let v = t.raw_get::<Value>(i).map_err(|e| S::Error::custom(e.to_string()))?;
            seq.serialize_element(&Node(&v, depth + 1))?;
        }
        return seq.end();
    }
    // Keys in order (a key written twice, `1` and `"1"`: the last one).
    let mut out = BTreeMap::new();
    for (k, v) in pairs {
        let key = match &k {
            Value::String(s) => s.to_str().map_err(|_| S::Error::custom("json: a key that is not UTF-8"))?.to_string(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            other => return Err(S::Error::custom(format!("json: a {} key (keys are strings or numbers)", other.type_name()))),
        };
        out.insert(key, v);
    }
    let mut map = s.serialize_map(Some(out.len()))?;
    for (k, v) in &out {
        map.serialize_entry(k, &Node(v, depth + 1))?;
    }
    map.end()
}

/// JSON as a Luau value (`null` is `nil`).
pub fn from_json(lua: &Lua, v: &Json) -> mlua::Result<Value> {
    Ok(match v {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, from_json(lua, item)?)?;
            }
            Value::Table(t)
        }
        Json::Object(map) => {
            let t = lua.create_table_with_capacity(0, map.len())?;
            for (k, item) in map {
                t.raw_set(k.as_str(), from_json(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    #[test]
    fn round_trip() {
        let lua = Lua::new();
        super::install(&lua).unwrap();
        let out: String = lua
            .load(
                r#"
                local t = json.decode('{"a": [1, 2.5, null, "x"], "b": {"c": true}, "d": null, "e": []}')
                assert(t.a[1] == 1 and t.a[2] == 2.5 and t.a[3] == nil and t.a[4] == "x")
                assert(t.b.c == true and t.d == nil and #t.e == 0)
                assert(json.encode({1, 2, 3}) == "[1,2,3]")
                assert(json.encode({}) == "[]")
                assert(json.encode({x = 1}) == '{"x":1}')
                assert(json.encode({[10] = "a"}) == '{"10":"a"}')
                assert(json.encode(vector.create(1.1, -2, 0.5)) == "[1.1,-2,0.5]")
                local ok, err = pcall(json.encode, {f = print})
                assert(not ok and string.find(tostring(err), "function"))
                local loop = {}
                loop.self = loop
                ok, err = pcall(json.encode, loop)
                assert(not ok and string.find(tostring(err), "too deep"))
                ok, err = pcall(json.decode, "{oops")
                assert(not ok and string.find(tostring(err), "json.decode"))
                return json.encode(t.b)
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(out, r#"{"c":true}"#);
    }

    /// Tables that hold another many times over, or themselves: an error
    /// once the text is too long or too deep, quickly.
    #[test]
    fn bounded() {
        let lua = Lua::new();
        super::install(&lua).unwrap();
        let start = std::time::Instant::now();
        lua.load(
            r#"
            -- 2^40 leaves if written out.
            local t = { "leaf" }
            for _ = 1, 40 do t = { t, t } end
            local ok, err = pcall(json.encode, t)
            assert(not ok and string.find(tostring(err), "over 1024 KB"), tostring(err))
            ok, err = pcall(json.encode, t, true)
            assert(not ok and string.find(tostring(err), "over 1024 KB"), tostring(err))
            -- A cycle through two tables.
            local a, b = {}, {}
            a.b, b.a = b, a
            ok, err = pcall(json.encode, a)
            assert(not ok and string.find(tostring(err), "too deep"), tostring(err))
            -- Shared, not too much: fine.
            local shared = { 1, 2 }
            assert(json.encode({ x = shared, y = shared }) == '{"x":[1,2],"y":[1,2]}')
            -- One long string.
            ok, err = pcall(json.encode, string.rep("x", 2 * 1024 * 1024))
            assert(not ok and string.find(tostring(err), "over"), tostring(err))
            -- Keys in order; NaN refused.
            assert(json.encode({ b = 1, a = 2, [3] = 3 }) == '{"3":3,"a":2,"b":1}')
            ok, err = pcall(json.encode, { 0 / 0 })
            assert(not ok and string.find(tostring(err), "NaN"), tostring(err))
            "#,
        )
        .exec()
        .unwrap();
        assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
        let small = super::encode(&mlua::Value::Boolean(true), false, 3);
        assert!(matches!(small, Err(super::Error::TooLong(3))));
    }
}
