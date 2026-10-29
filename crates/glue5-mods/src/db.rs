//! `db`: a mod's SQLite database, for what `storage` is too plain for
//! (many records, queries, indexes). One file a mod, next to its storage in
//! the mods folder's `.glue5/storage` (`<mod>.db`): the player's data, not
//! the mod's. Opened at its first use, closed when the mod stops.
//!
//! Queries run on the mod's turn, as natives do: WAL with synchronous=NORMAL
//! makes a commit a write to the log without a flush to the disk (a few
//! hundred inserts in a transaction take about a millisecond). The guards
//! are the app's, not the mod's: the database cannot reach other files
//! (no ATTACH, no file-setting pragmas, no extensions), it grows to
//! [`LIMIT`] at most, and a query running past [`RUNAWAY`] is stopped.

use std::cell::{RefCell, RefMut};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{Function, IntoLua, Lua, MultiValue, Table, Value};
use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Batch, Connection, OpenFlags, Statement, ToSql};

/// How large a mod's database may grow.
const LIMIT: u64 = 256 << 20;
/// A query that runs this long is stopped (as a mod that runs this long
/// without waiting is).
const RUNAWAY: Duration = Duration::from_secs(2);

pub struct Db {
    path: Option<PathBuf>,
    /// How large it may grow ([`LIMIT`]).
    limit: u64,
    conn: RefCell<Option<Connection>>,
    /// When the running query is stopped.
    deadline: Arc<Mutex<Instant>>,
}

impl Db {
    /// Mod `module`'s database (the file is made at its first use).
    pub fn new(module: &str) -> Rc<Db> {
        Db::at(crate::dir().map(|d| d.join(".glue5/storage").join(format!("{module}.db"))), LIMIT)
    }

    /// The database in file `path` (none: in memory), up to `limit` bytes.
    fn at(path: Option<PathBuf>, limit: u64) -> Rc<Db> {
        Rc::new(Db { path, limit, conn: RefCell::default(), deadline: Arc::new(Mutex::new(Instant::now())) })
    }

    /// The connection, opened the first time; the running query's
    /// deadline starts now.
    fn conn(&self) -> rusqlite::Result<RefMut<'_, Connection>> {
        *crate::lock(&self.deadline) = Instant::now() + RUNAWAY;
        let mut slot = self.conn.borrow_mut();
        if slot.is_none() {
            *slot = Some(self.open()?);
        }
        Ok(RefMut::map(slot, |c| c.as_mut().unwrap()))
    }

    fn open(&self) -> rusqlite::Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match &self.path {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                Connection::open_with_flags(path, flags)?
            }
            None => Connection::open_in_memory_with_flags(flags)?,
        };
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        let page: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        conn.execute_batch(&format!("PRAGMA max_page_count = {}", self.limit / page.max(512) as u64))?;
        // Its own file only, and no larger.
        conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
            AuthAction::Pragma { pragma_name, pragma_value }
                if ["temp_store_directory", "data_store_directory"]
                    .iter()
                    .any(|p| pragma_name.eq_ignore_ascii_case(p))
                    || (pragma_value.is_some() && pragma_name.eq_ignore_ascii_case("max_page_count")) =>
            {
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }))?;
        let deadline = self.deadline.clone();
        conn.progress_handler(10_000, Some(move || Instant::now() >= *crate::lock(&deadline)))?;
        Ok(conn)
    }

    /// `db.exec(sql, ...)`: every statement in `sql`, the values bound to
    /// the only one; how many rows the last changed, and the last row
    /// inserted.
    fn exec(&self, sql: &str, args: &Args) -> mlua::Result<(i64, i64)> {
        let conn = self.conn().map_err(|e| error("db.exec", e))?;
        let mut batch = Batch::new(&conn, sql);
        let mut statements = 0;
        while let Some(mut stmt) = batch.next().map_err(|e| error("db.exec", e))? {
            statements += 1;
            if statements > 1 && !args.is_empty() {
                return Err(mlua::Error::runtime("db.exec: values go with one statement"));
            }
            bind(&mut stmt, args).map_err(|e| mlua::Error::runtime(format!("db.exec: {e}")))?;
            let mut rows = stmt.raw_query();
            while rows.next().map_err(|e| error("db.exec", e))?.is_some() {}
        }
        Ok((conn.changes() as i64, conn.last_insert_rowid()))
    }

    /// `db.query(sql, ...)`: its rows, as tables by column name.
    fn query(&self, lua: &Lua, sql: &str, args: &Args) -> mlua::Result<Table> {
        let conn = self.conn().map_err(|e| error("db.query", e))?;
        let mut stmt = conn.prepare_cached(sql).map_err(|e| error("db.query", e))?;
        bind(&mut stmt, args).map_err(|e| mlua::Error::runtime(format!("db.query: {e}")))?;
        let names: Vec<mlua::LuaString> =
            stmt.column_names().iter().map(|n| lua.create_string(n)).collect::<mlua::Result<_>>()?;
        let out = lua.create_table()?;
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next().map_err(|e| error("db.query", e))? {
            let t = lua.create_table_with_capacity(0, names.len())?;
            for (i, name) in names.iter().enumerate() {
                let v = match row.get_ref(i).map_err(|e| error("db.query", e))? {
                    ValueRef::Null => continue,
                    ValueRef::Integer(n) => (n as f64).into_lua(lua)?,
                    ValueRef::Real(n) => Value::Number(n),
                    ValueRef::Text(s) => Value::String(lua.create_string(s)?),
                    ValueRef::Blob(b) => Value::Buffer(lua.create_buffer(b)?),
                };
                t.raw_set(name, v)?;
            }
            out.raw_push(t)?;
        }
        Ok(out)
    }

    /// Adds `db` to a mod's VM.
    pub fn install(self: &Rc<Self>, lua: &Lua) -> mlua::Result<()> {
        let t = lua.create_table()?;
        let d = self.clone();
        let exec = lua.create_function(move |_, (sql, args): (mlua::LuaString, MultiValue)| {
            d.exec(&sql.to_str()?, &Args::from(args)?)
        })?;
        t.set("exec", exec.clone())?;
        let d = self.clone();
        t.set(
            "query",
            lua.create_function(move |lua, (sql, args): (mlua::LuaString, MultiValue)| {
                d.query(lua, &sql.to_str()?, &Args::from(args)?)
            })?,
        )?;
        // A savepoint: nested ones are parts of the outer one; `f` may
        // wait, the transaction lasts until it returns.
        let transaction: Function = lua
            .load(
                "local exec, pcall, error, pack, unpack = ...\n\
                 return function(f, ...)\n\
                 exec('SAVEPOINT glue5')\n\
                 local r = pack(pcall(f, ...))\n\
                 if r[1] then exec('RELEASE glue5') return unpack(r, 2, r.n) end\n\
                 -- (gone already when SQLite undid it all: a full database)\n\
                 if pcall(exec, 'ROLLBACK TO glue5') then exec('RELEASE glue5') end\n\
                 error(r[2], 0)\n\
                 end",
            )
            .set_name("=db.transaction")
            .call((
                exec,
                lua.globals().get::<Function>("pcall")?,
                lua.globals().get::<Function>("error")?,
                lua.globals().get::<Table>("table")?.get::<Function>("pack")?,
                lua.globals().get::<Table>("table")?.get::<Function>("unpack")?,
            ))?;
        t.set("transaction", transaction)?;
        lua.globals().set("db", t)
    }
}

impl Db {
    /// Closes it and deletes its files (the player cleared the mod's
    /// storage); the next use starts an empty one.
    pub fn clear(&self) {
        self.conn.borrow_mut().take();
        if let Some(path) = &self.path {
            remove(path);
        }
    }
}

/// Deletes mod `module`'s database (a running mod's VM closes it first,
/// [`Db::clear`]).
pub fn delete(module: &str) {
    if let Some(d) = crate::dir() {
        remove(&d.join(".glue5/storage").join(format!("{module}.db")));
    }
}

/// The database at `path`, with its log and shared memory.
fn remove(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        let _ = std::fs::remove_file(file);
    }
}

/// An error of SQLite's, as the mod reads it.
fn error(what: &str, e: rusqlite::Error) -> mlua::Error {
    let text = match &e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => {
            format!("ran over {} s and was stopped", RUNAWAY.as_secs())
        }
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DiskFull => {
            format!("the database is full (a mod's may take {} MB)", LIMIT >> 20)
        }
        rusqlite::Error::SqliteFailure(_, Some(m)) => m.clone(),
        e => e.to_string(),
    };
    mlua::Error::runtime(format!("{what}: {text}"))
}

/// A Luau value bound to a statement, as it is.
enum Param {
    Null,
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            Param::Null => ValueRef::Null,
            Param::Integer(n) => ValueRef::Integer(*n),
            Param::Real(n) => ValueRef::Real(*n),
            Param::Text(s) => ValueRef::Text(s),
            Param::Blob(b) => ValueRef::Blob(b),
        }))
    }
}

impl Param {
    fn from(v: &Value) -> Result<Param, String> {
        Ok(match v {
            Value::Nil => Param::Null,
            Value::Boolean(b) => Param::Integer(i64::from(*b)),
            Value::Integer(n) => Param::Integer(*n),
            // Whole numbers as integers (SQLite's affinity makes them reals
            // in a REAL column).
            Value::Number(n) if n.fract() == 0.0 && n.abs() <= 9007199254740992.0 => Param::Integer(*n as i64),
            Value::Number(n) => Param::Real(*n),
            Value::String(s) => Param::Text(s.as_bytes().to_vec()),
            Value::Buffer(b) => Param::Blob(b.to_vec()),
            other => {
                return Err(format!(
                    "a {} cannot be stored (nil, booleans, numbers, strings, buffers)",
                    other.type_name()
                ));
            }
        })
    }
}

/// The values after the SQL: one by one, or a table of them by name
/// (`:name`, `@name`, `$name`) and position (`?`).
enum Args {
    List(Vec<Value>),
    Table(Table),
}

impl Args {
    fn from(args: MultiValue) -> mlua::Result<Args> {
        let mut args: Vec<Value> = args.into_iter().collect();
        Ok(match args.as_slice() {
            [Value::Table(_)] => match args.pop() {
                Some(Value::Table(t)) => Args::Table(t),
                _ => unreachable!(),
            },
            _ => Args::List(args),
        })
    }

    fn is_empty(&self) -> bool {
        matches!(self, Args::List(l) if l.is_empty())
    }
}

fn bind(stmt: &mut Statement<'_>, args: &Args) -> Result<(), String> {
    let count = stmt.parameter_count();
    match args {
        Args::List(values) => {
            if values.len() != count {
                return Err(format!("the statement takes {count} values, not {}", values.len()));
            }
            for (i, v) in values.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, Param::from(v)?).map_err(|e| e.to_string())?;
            }
        }
        Args::Table(t) => {
            for i in 1..=count {
                let v: Value = match stmt.parameter_name(i) {
                    Some(name) if !name.starts_with('?') => t.raw_get(&name[1..]),
                    _ => t.raw_get(i),
                }
                .map_err(|e| e.to_string())?;
                stmt.raw_bind_parameter(i, Param::from(&v)?).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// The definitions of `db`, for the type checker.
pub const DEFINITIONS: &str = "
-- A row of db.query: its columns by name (a NULL is a missing key).
export type DbRow = { [string]: any }

-- The mod's SQLite database (SQLite's SQL, its JSON and full-text functions included), for many records
-- and queries: one file kept for this player, as storage is, 256 MB at most. Values go in as `?`, `:name`
-- (`@name`, `$name`): one by one after the SQL, or in a table. nil is NULL, booleans 1 and 0, strings
-- text, buffers blobs (they come back as buffers). Queries run on the mod's turn: keep large work in
-- transactions (a few hundred inserts in one take about a millisecond); one that runs 2 s is stopped.
declare db: {
\t-- Runs the statements in `sql` (values: one statement only); how many rows the last one changed,
\t-- and the rowid of the last row inserted.
\texec: (sql: string, ...any) -> (number, number),
\t-- The rows of a statement (SELECT, or one with RETURNING).
\tquery: (sql: string, ...any) -> { DbRow },
\t-- Runs f(...) in a transaction (a savepoint: they nest): kept if it returns, undone if it fails
\t-- (and the error goes on). It may wait: the transaction lasts until it returns.
\ttransaction: <R...>(f: (...any) -> R..., ...any) -> R...,
}
";

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with(db: &Rc<Db>) -> Lua {
        let lua = Lua::new();
        db.install(&lua).unwrap();
        lua
    }

    #[test]
    fn round_trips() {
        let lua = lua_with(&Db::at(None, LIMIT));
        let out: (f64, f64, String, bool, f64, String) = lua
            .load(
                r#"
                db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB, flag INTEGER)")
                local n, id = db.exec("INSERT INTO t (name, score, data, flag) VALUES (?, ?, ?, ?)", "a\0b", 1.5, buffer.fromstring("xy"), true)
                assert(n == 1 and id == 1)
                db.exec("INSERT INTO t (name, score) VALUES (:name, @score)", { name = "c", score = 3 })
                local rows = db.query("SELECT * FROM t ORDER BY id")
                assert(#rows == 2 and rows[2].data == nil)
                local r = rows[1]
                return #rows, r.score, r.name, buffer.tostring(r.data) == "xy", r.flag, rows[2].name
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(out, (2.0, 1.5, "a\0b".into(), true, 1.0, "c".into()));
    }

    #[test]
    fn transactions_undo_and_nest() {
        let lua = lua_with(&Db::at(None, LIMIT));
        let n: f64 = lua
            .load(
                r#"
                db.exec("CREATE TABLE t (x)")
                local ok = pcall(db.transaction, function()
                    db.exec("INSERT INTO t VALUES (1)")
                    error("no")
                end)
                assert(not ok)
                local v = db.transaction(function(a)
                    db.exec("INSERT INTO t VALUES (?)", a)
                    pcall(db.transaction, function() db.exec("INSERT INTO t VALUES (3)") error("inner") end)
                    return a
                end, 2)
                assert(v == 2)
                return db.query("SELECT count(*) AS n, sum(x) AS s FROM t")[1].n
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(n, 1.0);
    }

    #[test]
    fn errors_are_the_mods() {
        let lua = lua_with(&Db::at(None, LIMIT));
        let err = |code: &str| lua.load(code).exec().unwrap_err().to_string();
        assert!(err("db.exec('SELEC 1')").contains("db.exec: near \"SELEC\""));
        assert!(err("db.query('SELECT ?', 1, 2)").contains("takes 1 values, not 2"));
        assert!(err("db.exec('SELECT ?; SELECT ?', 1)").contains("one statement"));
        assert!(err("db.exec('PRAGMA max_page_count = 100000000')").contains("db.exec"));
        assert!(err("db.exec('SELECT ?', print)").contains("a function cannot be stored"));
        // Nothing but its own file.
        assert!(err("db.exec(\"ATTACH 'other.db' AS o\")").contains("db.exec"));
        assert!(err("db.exec(\"VACUUM INTO 'copy.db'\")").contains("db.exec"));
        assert!(err("db.exec(\"PRAGMA temp_store_directory = '/tmp'\")").contains("db.exec"));
        assert!(err("db.query(\"SELECT load_extension('x')\")").contains("db.query"));
        // A query that would never end.
        let start = Instant::now();
        let e =
            err("db.query('WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c')");
        assert!(e.contains("ran over 2 s"), "{e}");
        assert!(start.elapsed() < RUNAWAY + Duration::from_secs(1));
    }

    /// The database is a file of its own, which stays at its limit; the
    /// time a mod's typical work takes on its turn (`--ignored --nocapture`
    /// prints it).
    #[test]
    fn file_limit_and_timing() {
        let dir = std::env::temp_dir().join(format!("glue5-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("m.db");
        let db = Db::at(Some(path.clone()), 1 << 20);
        let lua = lua_with(&db);
        let time = |code: &str| {
            let start = Instant::now();
            lua.load(code).exec().unwrap();
            start.elapsed()
        };
        time(
            "db.exec('CREATE TABLE cars (id INTEGER PRIMARY KEY, model TEXT, x REAL, y REAL, z REAL, owner TEXT, at INTEGER)')",
        );
        let one_by_one = time(
            "for i = 1, 300 do db.exec('INSERT INTO cars (model, x, y, z, owner, at) VALUES (?, ?, ?, ?, ?, ?)', 'adder', i, i * 2, 30.5, 'me', i) end",
        );
        let in_transaction = time(
            "db.transaction(function() for i = 1, 300 do db.exec('INSERT INTO cars (model, x, y, z, owner, at) VALUES (?, ?, ?, ?, ?, ?)', 'zentorno', i, i * 2, 30.5, 'me', i) end end)",
        );
        let select =
            time("local rows = db.query('SELECT * FROM cars WHERE model = ?', 'zentorno') assert(#rows == 300)");
        let update = time("db.exec('UPDATE cars SET x = x + 1 WHERE owner = ?', 'me')");
        eprintln!(
            "300 inserts one by one {one_by_one:?}, in a transaction {in_transaction:?}; select 300 {select:?}; update 600 {update:?}"
        );
        assert!(path.exists() && dir.join("m.db-wal").exists());
        // Past the limit: full, not larger.
        let e = lua
            .load("db.transaction(function() for i = 1, 30 do db.exec('INSERT INTO cars (owner) VALUES (?)', string.rep('x', 100000)) end end)")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(e.contains("the database is full"), "{e}");
        assert!(std::fs::metadata(&path).unwrap().len() <= 1 << 20);
        // Cleared: closed, its files gone, an empty one at the next use.
        db.clear();
        assert!(!path.exists() && !dir.join("m.db-wal").exists());
        let n: f64 =
            lua.load("db.exec('CREATE TABLE cars (id)') return #db.query('SELECT * FROM cars')").eval().unwrap();
        assert_eq!(n, 0.0);
        drop(lua);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
