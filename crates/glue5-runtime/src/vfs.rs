//! The filesystem the engine sees, backed by real directories:
//!
//! | engine path | host |
//! |---|---|
//! | `/game/…` | the game's `data/` directory, read-only, case-insensitive (none when the game streams: the engine's HTTP device serves it) |
//! | `/userdata/…` | the per-user directory (saves, settings) |
//! | `/dev/null`, `/dev/urandom`, `/dev/random` | none: emulated (Windows has no `/dev`; the engine's network crypto reads `/dev/urandom`) |
//! | anything else | a scratch tree under the user directory |
//!
//! Errors are WASI errno values, which is what Emscripten's libc expects.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Errno = u16;

pub mod errno {
    use super::Errno;
    pub const ACCES: Errno = 2;
    pub const BADF: Errno = 8;
    pub const EXIST: Errno = 20;
    pub const INVAL: Errno = 28;
    pub const IO: Errno = 29;
    pub const ISDIR: Errno = 31;
    pub const MFILE: Errno = 33;
    pub const NOENT: Errno = 44;
    pub const NOSYS: Errno = 52;
    pub const NOTDIR: Errno = 54;
    pub const NOTEMPTY: Errno = 55;
    pub const NOTTY: Errno = 59;
    pub const PERM: Errno = 63;
    pub const RANGE: Errno = 68;
    pub const ROFS: Errno = 69;
    pub const SPIPE: Errno = 70;
}

pub mod oflag {
    pub const ACCMODE: u32 = 3;
    pub const WRONLY: u32 = 1;
    pub const RDWR: u32 = 2;
    pub const CREAT: u32 = 0o100;
    pub const EXCL: u32 = 0o200;
    pub const TRUNC: u32 = 0o1000;
    pub const APPEND: u32 = 0o2000;
    pub const DIRECTORY: u32 = 0o200000;
}

const S_IFCHR: u32 = 0o020000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

/// What `stat` reports, in Emscripten's terms.
pub struct Stat {
    pub dev: u32,
    pub ino: u64,
    pub mode: u32,
    pub size: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
}

enum Node {
    Stdin,
    Stdout,
    Stderr,
    File(File),
    Dir(PathBuf),
    Null,
    Random,
}

/// The device at an engine path.
fn device(path: &str) -> Option<Node> {
    match path {
        "/dev/null" => Some(Node::Null),
        "/dev/urandom" | "/dev/random" => Some(Node::Random),
        _ => None,
    }
}

const DEVICE_STAT: Stat =
    Stat { dev: 1, ino: 0, mode: S_IFCHR | 0o666, size: 0, atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH };

struct Open {
    node: Node,
    path: String,
    flags: u32,
    dir: Mutex<DirStream>,
    /// A file's offset, kept here rather than the host's: reads and writes
    /// all go to an offset, so `pread` leaves it alone on every platform
    /// (Windows moves a handle's own offset on positioned reads).
    pos: Mutex<u64>,
}

/// getdents64 state of an open directory.
#[derive(Default)]
struct DirStream {
    /// (name, d_type), listed on first use.
    entries: Option<Vec<(String, u8)>>,
    next: usize,
}

/// A bounded, content-free trace of filesystem probes related to GTA V saves.
struct SaveTrace {
    writer: BufWriter<File>,
    written: usize,
    capped: bool,
}

impl SaveTrace {
    const LIMIT: usize = 128 * 1024;

    fn record(&mut self, line: &str) {
        if self.capped {
            return;
        }
        let line = format!("{line}\n");
        if self.written + line.len() > Self::LIMIT {
            let _ = writeln!(self.writer, "save-vfs trace capped at {} bytes", Self::LIMIT);
            self.capped = true;
            return;
        }
        if self.writer.write_all(line.as_bytes()).is_ok() {
            self.written += line.len();
        }
    }
}

pub struct Vfs {
    game: Option<PathBuf>,
    /// Lowercase path under `game` -> real path, for case-insensitive lookups.
    game_index: HashMap<String, String>,
    user: PathBuf,
    scratch: PathBuf,
    save_trace: Mutex<Option<SaveTrace>>,
    fds: Mutex<Vec<Option<Arc<Open>>>>,
}

pub struct Host {
    path: PathBuf,
    read_only: bool,
}

impl Vfs {
    pub fn new(game_data: Option<PathBuf>, user: PathBuf) -> io::Result<Self> {
        raise_open_file_limit();
        let scratch = user.join("fsroot");
        fs::create_dir_all(&scratch)?;
        let save_trace = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(user.join("save-vfs.log"))
            .ok()
            .map(|file| SaveTrace { writer: BufWriter::new(file), written: 0, capped: false });
        let mut game_index = HashMap::new();
        match &game_data {
            Some(game) => {
                index(game, "", &mut game_index)?;
                log::info!("vfs: /game -> {} ({} entries), /userdata -> {}", game.display(), game_index.len(), user.display());
            }
            None => log::info!("vfs: /game streamed, /userdata -> {}", user.display()),
        }
        let std_fd = |node| {
            Some(Arc::new(Open { node, path: String::new(), flags: 0, dir: Mutex::default(), pos: Mutex::default() }))
        };
        Ok(Vfs {
            game: game_data,
            game_index,
            user,
            scratch,
            save_trace: Mutex::new(save_trace),
            fds: Mutex::new(vec![std_fd(Node::Stdin), std_fd(Node::Stdout), std_fd(Node::Stderr)]),
        })
    }

    /// Maps an absolute engine path to the host.
    pub fn resolve(&self, path: &str) -> Result<Host, Errno> {
        let path = normalize(path);
        // On Windows a component with a colon (`C:`, `file:stream`) would
        // leave the roots once joined.
        if cfg!(windows) && path.contains(':') {
            return Err(errno::NOENT);
        }
        if path == "/game" || path.starts_with("/game/") {
            let Some(game) = &self.game else {
                // The engine's own device serves /game; the browser build had nothing here either.
                log::warn!("vfs: {path} asked of the file system while the game streams");
                return Err(errno::NOENT);
            };
            // (Once: `/game/gameconfig.xml` is `gameconfig.xml`.)
            let rel = path.strip_prefix("/game").unwrap_or("").trim_start_matches('/');
            if rel.is_empty() {
                return Ok(Host { path: game.clone(), read_only: true });
            }
            let real = self.game_index.get(&rel.to_ascii_lowercase()).map_or(rel, String::as_str);
            let real = game.join(real);
            let path = crate::overlay::game_file(rel, &real, &self.scratch).unwrap_or(real);
            return Ok(Host { path, read_only: true });
        }
        if let Some(rel) = path.strip_prefix("/userdata")
            && (rel.is_empty() || rel.starts_with('/'))
        {
            return Ok(Host { path: self.user.join(rel.trim_start_matches('/')), read_only: false });
        }
        Ok(Host { path: self.scratch.join(path.trim_start_matches('/')), read_only: false })
    }

    /// Joins a path given relative to `dirfd` (`AT_FDCWD` = -100; cwd is `/`).
    pub fn at(&self, dirfd: i32, path: &str) -> Result<String, Errno> {
        if path.starts_with('/') {
            return Ok(path.to_string());
        }
        let dir = if dirfd == -100 { "/".to_string() } else { self.get(dirfd)?.path.clone() };
        Ok(if path.is_empty() { dir } else { format!("{dir}/{path}") })
    }

    fn trace_save(&self, operation: &str, path: &str, result: impl AsRef<str>) {
        if !is_save_trace_path(path) {
            return;
        }
        let line = format!("save-vfs op={operation} path={} {}", normalize(path), result.as_ref());
        log::info!("{line}");
        if let Some(trace) = self.save_trace.lock().unwrap().as_mut() {
            trace.record(&line);
        }
    }

    fn trace_unit(&self, operation: &str, path: &str, result: Result<(), Errno>) -> Result<(), Errno> {
        match result {
            Ok(()) => {
                self.trace_save(operation, path, "result=ok");
                Ok(())
            }
            Err(error) => {
                self.trace_save(operation, path, format!("result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub(crate) fn flush_save_trace(&self) {
        if let Some(trace) = self.save_trace.lock().unwrap().as_mut() {
            let _ = trace.writer.flush();
        }
    }

    pub fn open(&self, path: &str, flags: u32) -> Result<i32, Errno> {
        let result = self.open_inner(path, flags);
        match result {
            Ok(fd) => {
                self.trace_save("open", path, format!("flags=0x{flags:x} result=fd:{fd}"));
                Ok(fd)
            }
            Err(error) => {
                self.trace_save("open", path, format!("flags=0x{flags:x} result=errno:{error}"));
                Err(error)
            }
        }
    }

    fn open_inner(&self, path: &str, flags: u32) -> Result<i32, Errno> {
        if let Some(node) = device(&normalize(path)) {
            return self.add(node, path, flags);
        }
        let host = self.resolve(path)?;
        let write = flags & oflag::ACCMODE != 0 || flags & (oflag::CREAT | oflag::TRUNC) != 0;
        if write && host.read_only {
            return Err(errno::ROFS);
        }
        let node = match fs::metadata(&host.path) {
            Ok(meta) if meta.is_dir() => {
                if write {
                    return Err(errno::ISDIR);
                }
                Node::Dir(host.path)
            }
            Ok(_) if flags & oflag::DIRECTORY != 0 => return Err(errno::NOTDIR),
            Ok(_) if flags & oflag::CREAT != 0 && flags & oflag::EXCL != 0 => return Err(errno::EXIST),
            Err(error) if flags & oflag::CREAT == 0 => {
                if is_save_trace_path(path) {
                    self.trace_save(
                        "open-host-metadata",
                        path,
                        format!("host_error={:?}; surfaced_errno:{}", error.kind(), errno::NOENT),
                    );
                }
                return Err(errno::NOENT);
            }
            _ => {
                if let Some(parent) = host.path.parent()
                    && !host.read_only
                {
                    let _ = fs::create_dir_all(parent);
                }
                let acc = flags & oflag::ACCMODE;
                let file = OpenOptions::new()
                    .read(acc != oflag::WRONLY)
                    .write(acc == oflag::WRONLY || acc == oflag::RDWR)
                    .append(flags & oflag::APPEND != 0)
                    .truncate(flags & oflag::TRUNC != 0)
                    .create(flags & oflag::CREAT != 0)
                    .open(&host.path)
                    .map_err(io_errno)?;
                Node::File(file)
            }
        };
        self.add(node, path, flags)
    }

    /// Gives an opened node the lowest free descriptor.
    fn add(&self, node: Node, path: &str, flags: u32) -> Result<i32, Errno> {
        let open = Arc::new(Open { node, path: normalize(path), flags, dir: Mutex::default(), pos: Mutex::default() });
        let mut fds = self.fds.lock().unwrap();
        let fd = match fds.iter().skip(3).position(Option::is_none) {
            Some(i) => i + 3,
            None => {
                fds.push(None);
                fds.len() - 1
            }
        };
        if fd > 65535 {
            return Err(errno::MFILE);
        }
        fds[fd] = Some(open);
        Ok(fd as i32)
    }

    pub fn close(&self, fd: i32) -> Result<(), Errno> {
        let mut fds = self.fds.lock().unwrap();
        match fds.get_mut(fd as usize) {
            Some(slot @ Some(_)) => {
                *slot = None;
                Ok(())
            }
            _ => Err(errno::BADF),
        }
    }

    fn get(&self, fd: i32) -> Result<Arc<Open>, Errno> {
        let fds = self.fds.lock().unwrap();
        fds.get(fd as usize).and_then(Clone::clone).ok_or(errno::BADF)
    }

    pub fn read(&self, fd: i32, buf: &mut [u8], offset: Option<u64>) -> Result<usize, Errno> {
        let open = self.get(fd)?;
        let path = open.path.clone();
        let start = offset.unwrap_or_else(|| *open.pos.lock().unwrap());
        let result = match &open.node {
            Node::File(f) => match offset {
                Some(off) => read_at(f, buf, off).map_err(io_errno),
                None => {
                    let mut pos = open.pos.lock().unwrap();
                    let result = read_at(f, buf, *pos).map_err(io_errno);
                    if let Ok(n) = result {
                        *pos += n as u64;
                    }
                    result
                }
            },
            Node::Stdin | Node::Null => Ok(0),
            Node::Random => getrandom::fill(buf).map(|()| buf.len()).map_err(|_| errno::IO),
            Node::Dir(_) => Err(errno::ISDIR),
            Node::Stdout | Node::Stderr => Err(errno::BADF),
        };
        match result {
            Ok(bytes) => {
                self.trace_save("read", &path, format!("offset={start} result=bytes:{bytes}"));
                Ok(bytes)
            }
            Err(error) => {
                self.trace_save("read", &path, format!("offset={start} result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub fn write(&self, fd: i32, buf: &[u8], offset: Option<u64>) -> Result<usize, Errno> {
        let open = self.get(fd)?;
        let path = open.path.clone();
        let start = offset.unwrap_or_else(|| *open.pos.lock().unwrap());
        let result = match &open.node {
            Node::File(f) => match offset {
                Some(off) => write_at(f, buf, off).map_err(io_errno),
                None => {
                    let mut pos = open.pos.lock().unwrap();
                    if open.flags & oflag::APPEND != 0 {
                        // The host appends (the handle is opened so).
                        (&*f).write(buf).map_err(io_errno).and_then(|n| {
                            *pos = f.metadata().map_err(io_errno)?.len();
                            Ok(n)
                        })
                    } else {
                        let result = write_at(f, buf, *pos).map_err(io_errno);
                        if let Ok(n) = result {
                            *pos += n as u64;
                        }
                        result
                    }
                }
            },
            // The engine's printf output goes to its log (never blocks).
            Node::Stdout | Node::Stderr => {
                crate::gamelog::write(buf);
                Ok(buf.len())
            }
            Node::Null | Node::Random => Ok(buf.len()),
            Node::Dir(_) => Err(errno::ISDIR),
            Node::Stdin => Err(errno::BADF),
        };
        match result {
            Ok(bytes) => {
                self.trace_save("write", &path, format!("offset={start} result=bytes:{bytes}"));
                Ok(bytes)
            }
            Err(error) => {
                self.trace_save("write", &path, format!("offset={start} result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub fn seek(&self, fd: i32, offset: i64, whence: u32) -> Result<u64, Errno> {
        let open = self.get(fd)?;
        match &open.node {
            Node::File(f) => {
                let mut pos = open.pos.lock().unwrap();
                let from = match whence {
                    0 => 0,
                    1 => *pos as i64,
                    2 => f.metadata().map_err(io_errno)?.len() as i64,
                    _ => return Err(errno::INVAL),
                };
                let new = from.checked_add(offset).filter(|&p| p >= 0).ok_or(errno::INVAL)?;
                *pos = new as u64;
                Ok(*pos)
            }
            Node::Dir(_) => {
                // Only rewinding is meaningful (offsets are getdents64 record offsets).
                let mut dir = open.dir.lock().unwrap();
                if whence == 0 {
                    dir.next = (offset.max(0) as usize) / DIRENT_SIZE;
                    if offset == 0 {
                        dir.entries = None;
                    }
                }
                Ok((dir.next * DIRENT_SIZE) as u64)
            }
            Node::Null | Node::Random => Ok(0),
            _ => Err(errno::SPIPE),
        }
    }

    pub fn truncate(&self, fd: i32, len: u64) -> Result<(), Errno> {
        let open = self.get(fd)?;
        let result = match &open.node {
            Node::File(f) => f.set_len(len).map_err(io_errno),
            _ => Err(errno::INVAL),
        };
        match result {
            Ok(()) => {
                self.trace_save("truncate", &open.path, format!("length={len} result=ok"));
                Ok(())
            }
            Err(error) => {
                self.trace_save("truncate", &open.path, format!("length={len} result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub fn flags(&self, fd: i32) -> Result<u32, Errno> {
        Ok(self.get(fd)?.flags)
    }

    pub fn is_tty(&self, fd: i32) -> Result<bool, Errno> {
        Ok(matches!(self.get(fd)?.node, Node::Stdin | Node::Stdout | Node::Stderr))
    }

    pub fn fstat(&self, fd: i32) -> Result<Stat, Errno> {
        let open = self.get(fd)?;
        let result = match &open.node {
            Node::File(f) => f.metadata().map(|meta| stat_of(&meta)).map_err(io_errno),
            Node::Dir(p) => fs::metadata(p).map(|meta| stat_of(&meta)).map_err(io_errno),
            _ => Ok(Stat { ino: fd as u64, ..DEVICE_STAT }),
        };
        match result {
            Ok(stat) => {
                self.trace_save("fstat", &open.path, format!("result=size:{} mode:0x{:x}", stat.size, stat.mode));
                Ok(stat)
            }
            Err(error) => {
                self.trace_save("fstat", &open.path, format!("result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub fn stat(&self, path: &str, follow: bool) -> Result<Stat, Errno> {
        let result = if device(&normalize(path)).is_some() {
            Ok(DEVICE_STAT)
        } else {
            self.resolve(path).and_then(|host| {
                let meta = if follow { fs::metadata(&host.path) } else { fs::symlink_metadata(&host.path) };
                meta.map(|meta| stat_of(&meta)).map_err(io_errno)
            })
        };
        match result {
            Ok(stat) => {
                self.trace_save("stat", path, format!("follow={follow} result=size:{} mode:0x{:x}", stat.size, stat.mode));
                Ok(stat)
            }
            Err(error) => {
                self.trace_save("stat", path, format!("follow={follow} result=errno:{error}"));
                Err(error)
            }
        }
    }

    pub fn mkdir(&self, path: &str) -> Result<(), Errno> {
        let result = self.writable(path).and_then(|host| fs::create_dir(host).map_err(io_errno));
        self.trace_unit("mkdir", path, result)
    }

    pub fn unlink(&self, path: &str) -> Result<(), Errno> {
        let result = self.writable(path).and_then(|host| {
            // A folder is EISDIR, as on Linux (remove() then takes rmdir):
            // macOS and Windows refuse it with a permission error.
            if fs::symlink_metadata(&host).is_ok_and(|m| m.is_dir()) {
                return Err(errno::ISDIR);
            }
            fs::remove_file(host).map_err(io_errno)
        });
        self.trace_unit("unlink", path, result)
    }

    pub fn rmdir(&self, path: &str) -> Result<(), Errno> {
        let result = self.writable(path).and_then(|host| fs::remove_dir(host).map_err(io_errno));
        self.trace_unit("rmdir", path, result)
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), Errno> {
        let result = self.writable(from).and_then(|from_host| {
            self.writable(to).and_then(|to_host| {
                if let Some(parent) = to_host.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::rename(from_host, to_host).map_err(io_errno)
            })
        });
        if is_save_trace_path(from) || is_save_trace_path(to) {
            let path = if is_save_trace_path(from) { from } else { to };
            match result {
                Ok(()) => self.trace_save("rename", path, format!("from={} to={} result=ok", normalize(from), normalize(to))),
                Err(error) => self.trace_save(
                    "rename",
                    path,
                    format!("from={} to={} result=errno:{error}", normalize(from), normalize(to)),
                ),
            }
        }
        result
    }

    fn writable(&self, path: &str) -> Result<PathBuf, Errno> {
        let host = self.resolve(path)?;
        if host.read_only { Err(errno::ROFS) } else { Ok(host.path) }
    }

    /// Fills `buf` with `struct dirent` records (Emscripten layout, 280 bytes
    /// each); returns the bytes written.
    pub fn getdents(&self, fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
        let open = self.get(fd)?;
        let path = open.path.clone();
        let result = (|| {
            let Node::Dir(path) = &open.node else { return Err(errno::NOTDIR) };
            let mut dir = open.dir.lock().unwrap();
            if dir.entries.is_none() {
                let mut list = vec![(".".to_string(), 4), ("..".to_string(), 4)];
                for entry in fs::read_dir(path).map_err(io_errno)? {
                    let entry = entry.map_err(io_errno)?;
                    let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { 4 } else { 8 };
                    list.push((entry.file_name().to_string_lossy().into_owned(), kind));
                }
                let names = list
                    .iter()
                    .skip(2)
                    .map(|(name, _)| name.as_str())
                    .filter(|name| is_relevant_save_entry(path, name))
                    .take(64)
                    .collect::<Vec<_>>()
                    .join(",");
                self.trace_save(
                    "getdents-list",
                    &open.path,
                    format!("result=entries:{} relevant_names=[{names}]", list.len().saturating_sub(2)),
                );
                dir.entries = Some(list);
            }
            let (list, pos) = (dir.entries.as_ref().unwrap().clone(), dir.next);
            let mut written = 0;
            let mut idx = pos;
            while idx < list.len() && written + DIRENT_SIZE <= buf.len() {
                let rec = &mut buf[written..written + DIRENT_SIZE];
                rec.fill(0);
                rec[0..8].copy_from_slice(&(idx as u64 + 1).to_le_bytes());
                rec[8..16].copy_from_slice(&(((idx + 1) * DIRENT_SIZE) as u64).to_le_bytes());
                rec[16..18].copy_from_slice(&(DIRENT_SIZE as u16).to_le_bytes());
                rec[18] = list[idx].1;
                let name = list[idx].0.as_bytes();
                let n = name.len().min(255);
                rec[19..19 + n].copy_from_slice(&name[..n]);
                written += DIRENT_SIZE;
                idx += 1;
            }
            dir.next = idx;
            Ok(written)
        })();
        match result {
            Ok(bytes) => {
                self.trace_save("getdents", &path, format!("result=bytes:{bytes}"));
                Ok(bytes)
            }
            Err(error) => {
                self.trace_save("getdents", &path, format!("result=errno:{error}"));
                Err(error)
            }
        }
    }
}

const DIRENT_SIZE: usize = 280;

fn index(root: &Path, rel: &str, out: &mut HashMap<String, String>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        out.insert(path.to_ascii_lowercase(), path.clone());
        if entry.file_type()?.is_dir() {
            index(root, &path, out)?;
        }
    }
    Ok(())
}

/// Absolute, without `.`/`..`/empty components.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

fn is_profile_id(name: &str) -> bool {
    name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_story_save_name(name: &str) -> bool {
    let name = name.strip_suffix(".bak").or_else(|| name.strip_suffix(".BAK")).unwrap_or(name);
    name.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("SGTA5"))
        && name.get(5..).is_some_and(|digits| digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Keep diagnostics focused on likely GTA V profile/save paths, including
/// alternate roots the engine might probe instead of `/userdata/Documents`.
fn is_save_trace_path(path: &str) -> bool {
    let path = normalize(path).to_ascii_lowercase();
    path.contains("rockstar games")
        || path.contains("/gta v")
        || path.contains("/gtav")
        || path.contains("/profile")
        || path.contains("saved games")
        || path.contains("sgta5")
        || path.split('/').any(is_profile_id)
}

fn is_relevant_save_entry(directory: &Path, name: &str) -> bool {
    name.eq_ignore_ascii_case("Profiles")
        || is_profile_id(name)
        || is_story_save_name(name)
        || name.eq_ignore_ascii_case("pc_settings.bin")
        || directory.to_string_lossy().to_ascii_lowercase().contains("profile")
}

fn stat_of(meta: &fs::Metadata) -> Stat {
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev() as u32, meta.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (1, 0);
    let mode = if meta.is_dir() { S_IFDIR | 0o755 } else { S_IFREG | 0o644 };
    Stat {
        dev,
        ino,
        mode,
        size: if meta.is_dir() { 4096 } else { meta.len() },
        atime: meta.accessed().unwrap_or(UNIX_EPOCH),
        mtime: meta.modified().unwrap_or(UNIX_EPOCH),
        ctime: meta.modified().unwrap_or(UNIX_EPOCH),
    }
}

/// The engine keeps hundreds of the game's packfiles open, a host file each;
/// a process started from the Finder or the home screen may have 256 open at
/// most (launchd's default), and the engine fails at load ("short read" on
/// a packfile it could not open). Raised to what the system allows.
fn raise_open_file_limit() {
    #[cfg(unix)]
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        // macOS takes no more than OPEN_MAX (10240) under an unlimited hard limit.
        let want = limit.rlim_max.min(10240);
        if limit.rlim_cur < want {
            let was = limit.rlim_cur;
            limit.rlim_cur = want;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                log::info!("open files: up to {want} (was {was})");
            } else {
                log::warn!("open files: still {was} at most: {}", io::Error::last_os_error());
            }
        }
    }
}

pub fn io_errno(e: io::Error) -> Errno {
    use io::ErrorKind::*;
    match e.kind() {
        NotFound => errno::NOENT,
        PermissionDenied => errno::ACCES,
        AlreadyExists => errno::EXIST,
        InvalidInput => errno::INVAL,
        NotADirectory => errno::NOTDIR,
        IsADirectory => errno::ISDIR,
        DirectoryNotEmpty => errno::NOTEMPTY,
        ReadOnlyFilesystem => errno::ROFS,
        _ => errno::IO,
    }
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}
#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(f, buf, off)
}
#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}
#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(f, buf, off)
}

/// Seconds and nanoseconds since the epoch.
pub fn epoch(t: SystemTime) -> (i64, u64) {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as u64),
        Err(_) => (0, 0),
    }
}

#[test]
fn normalizes() {
    assert_eq!(normalize("/game/./x64//audio/../data"), "/game/x64/data");
    assert_eq!(normalize("\\game\\x64"), "/game/x64");
    assert_eq!(normalize("/.."), "/");
}

#[test]
fn save_trace_filters_expected_and_alternate_profile_paths() {
    assert!(is_save_trace_path("/userdata/Documents/Rockstar Games/GTA V/Profiles/0F74F4C4"));
    assert!(is_save_trace_path("/userdata/0F74F4C4/SGTA50000"));
    assert!(!is_save_trace_path("/userdata/cache/shaders/index.json"));
}

#[test]
fn devices() {
    let dir = std::env::temp_dir().join(format!("glue5-vfs-devices-{}", std::process::id()));
    fs::create_dir_all(dir.join("game")).unwrap();
    let vfs = Vfs::new(Some(dir.join("game")), dir.join("user")).unwrap();
    let mut buf = [0u8; 64];
    let fd = vfs.open("/dev/urandom", 0).unwrap();
    assert_eq!(vfs.read(fd, &mut buf, None), Ok(64));
    assert_ne!(buf, [0; 64]);
    let fd = vfs.open("/dev/null", oflag::RDWR).unwrap();
    assert_eq!(vfs.write(fd, b"x", None), Ok(1));
    assert_eq!(vfs.read(fd, &mut buf, None), Ok(0));
    assert_eq!(vfs.stat("/dev/random", true).unwrap().mode, S_IFCHR | 0o666);
    drop(vfs);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn imported_story_profile_is_discoverable_through_userdata() {
    const PROFILE: &str = "0F74F4C4";
    let dir = std::env::temp_dir().join(format!("glue5-vfs-profile-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let game = dir.join("game");
    let user = dir.join("user");
    let profile = user.join(format!("Documents/Rockstar Games/GTA V/Profiles/{PROFILE}"));
    fs::create_dir_all(&game).unwrap();
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("SGTA50000"), vec![0u8; 4096]).unwrap();

    let vfs = Vfs::new(Some(game), user.clone()).unwrap();
    let engine_profile = format!("/userdata/Documents/Rockstar Games/GTA V/Profiles/{PROFILE}");
    let mapped = vfs.resolve(&engine_profile).unwrap();
    assert!(!mapped.read_only);
    assert_eq!(mapped.path, profile);

    let profiles_path = "/userdata/Documents/Rockstar Games/GTA V/Profiles";
    let fd = vfs.open(profiles_path, oflag::DIRECTORY).unwrap();
    let mut dirs = vec![0u8; DIRENT_SIZE * 64];
    let bytes = vfs.getdents(fd, &mut dirs).unwrap();
    let names = dirs[..bytes]
        .chunks_exact(DIRENT_SIZE)
        .map(|record| {
            let end = record[19..].iter().position(|&byte| byte == 0).unwrap_or(255);
            String::from_utf8_lossy(&record[19..19 + end]).into_owned()
        })
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == PROFILE));

    let save_path = format!("{engine_profile}/SGTA50000");
    let save_fd = vfs.open(&save_path, 0).unwrap();
    assert_eq!(vfs.fstat(save_fd).unwrap().size, 4096);
    drop(vfs);

    let trace = fs::read_to_string(user.join("save-vfs.log")).unwrap();
    assert!(trace.contains("op=getdents-list"));
    assert!(trace.contains(PROFILE));
    assert!(trace.contains("SGTA50000"));
    assert!(!trace.contains("private save data"));
    let _ = fs::remove_dir_all(dir);
}
