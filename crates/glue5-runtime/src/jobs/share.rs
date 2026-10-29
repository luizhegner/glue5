//! The lobby's jobs: each player's go over to the others by themselves.
//!
//! Every few seconds a player announces the jobs it made or got (an id, a
//! fingerprint of the file, its size, its version: when its author saved
//! it) on a port of the lobby's virtual LAN that the engine does not use; a
//! player without one (or with an older version) asks the announcer for
//! its pieces, a few at a time, again when a piece does not come. Received
//! whole and matching its fingerprint, the file goes to `jobs/shared/` and
//! into the library: when someone hosts a job, the others have it already,
//! as the scripts expect (they find a job's header and its file at once).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Job, Kind, MAX_FILE, add, fingerprint, is_content_id, library, modified, parse_job, read_job, shared_dir};

use crate::Locked;

/// The port on the virtual LAN (the engine's sockets take 6672 and 50000 up).
pub(crate) const PORT: u16 = 47900;
const BROADCAST: u32 = u32::MAX;
const ANNOUNCE: u8 = b'A';
const GET: u8 = b'G';
const PIECE: u8 = b'P';
/// A piece's size, inside a datagram.
const PIECE_SIZE: usize = 1024;
/// A job in an announcement (its id, fingerprint, size and version), and
/// jobs in one.
const ENTRY: usize = 22 + 8 + 4 + 8;
const PER_ANNOUNCEMENT: usize = 28;
const ANNOUNCE_EVERY: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(100);
/// Pieces asked for at a time, and when one is asked for again.
const WINDOW: usize = 16;
const RETRY: Duration = Duration::from_millis(600);
/// A download no piece came for in this long is dropped (its announcer
/// may well go on announcing it).
const GIVE_UP: Duration = Duration::from_secs(30);
/// A version from later than this is a wrong clock's, or a lie.
const AHEAD: Duration = Duration::from_secs(24 * 3600);
/// Jobs downloaded at once, and at once from one player (the others wait
/// for a later announcement).
const DOWNLOADS_AT_ONCE: usize = 8;
const DOWNLOADS_PER_PLAYER: usize = 2;

struct Download {
    hash: [u8; 8],
    version: u64,
    from: u32,
    pieces: Vec<Option<Vec<u8>>>,
    asked: Vec<Option<Instant>>,
    /// When it started, or its last piece came.
    progress: Instant,
}

static DOWNLOADS: Mutex<Option<HashMap<String, Download>>> = Mutex::new(None);

pub(super) fn start() {
    let started = crate::qos::spawn("jobs", crate::qos::Qos::Utility, || {
        let mut announced = Instant::now() - ANNOUNCE_EVERY;
        loop {
            std::thread::sleep(TICK);
            if crate::p2p::get().is_none() {
                continue;
            }
            if announced.elapsed() >= ANNOUNCE_EVERY {
                announced = Instant::now();
                announce();
            }
            ask();
        }
    });
    if let Err(e) = started {
        log::warn!("jobs: sharing: {e}");
    }
}

fn send(to: u32, payload: &[u8]) {
    if let Some(p2p) = crate::p2p::get() {
        p2p.send(PORT, to, PORT, payload);
    }
}

/// The jobs the lobby gets from this player: its own and those it got.
fn shareable(job: &Job) -> bool {
    job.kind != Kind::Game && job.size as usize <= MAX_FILE
}

/// A job's version: when its file was written (seconds since 1970), by its
/// author, or by this player as its author's (`on_piece`).
fn version(modified: SystemTime) -> u64 {
    modified.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether an announced job is wanted, given the one here (its kind,
/// fingerprint and version): the player's own and the game's stay as they
/// are, one shared before only for a newer version (two versions in the
/// lobby went back and forth for ever).
fn wanted(have: Option<(Kind, [u8; 8], u64)>, hash: &[u8; 8], version: u64) -> bool {
    have.is_none_or(|(kind, h, v)| kind == Kind::Shared && h != *hash && version > v)
}

fn announce() {
    // Ids, fingerprints and sizes only (the headers are up to MBs each).
    let jobs: Vec<(String, [u8; 8], u32, u64)> = library()
        .as_ref()
        .unwrap()
        .iter()
        .filter(|j| shareable(j))
        .map(|j| (j.id.clone(), j.hash, j.size, version(j.modified)))
        .collect();
    for chunk in jobs.chunks(PER_ANNOUNCEMENT) {
        let mut d = vec![ANNOUNCE];
        for (id, hash, size, version) in chunk {
            d.extend_from_slice(id.as_bytes());
            d.extend_from_slice(hash);
            d.extend_from_slice(&size.to_be_bytes());
            d.extend_from_slice(&version.to_be_bytes());
        }
        send(BROADCAST, &d);
    }
}

/// Asks for the pieces missing, a window at a time.
fn ask() {
    let now = Instant::now();
    let mut downloads = DOWNLOADS.locked();
    let Some(downloads) = downloads.as_mut() else { return };
    downloads.retain(|id, d| {
        if now.duration_since(d.progress) > GIVE_UP {
            log::info!("jobs: {id} from {}: gave up", std::net::Ipv4Addr::from(d.from));
            return false;
        }
        true
    });
    for (id, d) in downloads.iter_mut() {
        let waiting = d.asked.iter().zip(&d.pieces).filter(|(a, p)| p.is_none() && a.is_some_and(|t| now.duration_since(t) < RETRY)).count();
        let mut room = WINDOW.saturating_sub(waiting);
        for i in 0..d.pieces.len() {
            if room == 0 {
                break;
            }
            if d.pieces[i].is_some() || d.asked[i].is_some_and(|t| now.duration_since(t) < RETRY) {
                continue;
            }
            let mut g = vec![GET];
            g.extend_from_slice(id.as_bytes());
            g.extend_from_slice(&d.hash);
            g.extend_from_slice(&(i as u32).to_be_bytes());
            send(d.from, &g);
            d.asked[i] = Some(now);
            room -= 1;
        }
    }
}

/// A datagram on the jobs' port, from `from` (vnet.rs).
pub(crate) fn receive(from: u32, d: &[u8]) {
    match d.first() {
        Some(&ANNOUNCE) => on_announce(from, &d[1..]),
        Some(&GET) if d.len() >= 1 + 22 + 8 + 4 => {
            let (id, hash) = (&d[1..23], &d[23..31]);
            let index = u32::from_be_bytes(d[31..35].try_into().unwrap()) as usize;
            on_get(from, id, hash, index);
        }
        Some(&PIECE) if d.len() >= 1 + 22 + 8 + 8 => {
            let (id, hash) = (&d[1..23], &d[23..31]);
            let index = u32::from_be_bytes(d[31..35].try_into().unwrap()) as usize;
            let count = u32::from_be_bytes(d[35..39].try_into().unwrap()) as usize;
            on_piece(from, id, hash, index, count, &d[39..]);
        }
        _ => {}
    }
}

fn on_announce(from: u32, d: &[u8]) {
    // (The player's own or the game's first, when a shared one has its id.)
    let mut have: HashMap<String, (Kind, [u8; 8], u64)> = HashMap::new();
    for j in library().as_ref().unwrap() {
        let this = (j.kind, j.hash, version(j.modified));
        have.entry(j.id.clone()).and_modify(|h| if j.kind != Kind::Shared { *h = this }).or_insert(this);
    }
    let latest = version(SystemTime::now() + AHEAD);
    let mut downloads = DOWNLOADS.locked();
    let downloads = downloads.get_or_insert_default();
    for e in d.as_chunks::<ENTRY>().0 {
        let Ok(id) = std::str::from_utf8(&e[..22]) else { continue };
        let hash: [u8; 8] = e[22..30].try_into().unwrap();
        let size = u32::from_be_bytes(e[30..34].try_into().unwrap()) as usize;
        let version = u64::from_be_bytes(e[34..42].try_into().unwrap());
        if !is_content_id(id) || size == 0 || size > MAX_FILE || version > latest {
            continue;
        }
        if !wanted(have.get(id).copied(), &hash, version) {
            continue;
        }
        // A few at a time, from each player and in all: each takes room
        // for its pieces (MBs from announcements alone otherwise).
        let full = downloads.len() >= DOWNLOADS_AT_ONCE
            || downloads.values().filter(|d| d.from == from).count() >= DOWNLOADS_PER_PLAYER;
        match downloads.get(id) {
            // The one coming, or a newer one.
            Some(d) if d.hash == hash || d.version >= version => {}
            None if full => {}
            _ => {
                let count = size.div_ceil(PIECE_SIZE);
                log::info!("jobs: {id} ({size} bytes) from {}", std::net::Ipv4Addr::from(from));
                let (pieces, asked) = (vec![None; count], vec![None; count]);
                downloads.insert(id.to_owned(), Download { hash, version, from, pieces, asked, progress: Instant::now() });
            }
        }
    }
}

fn on_get(from: u32, id: &[u8], hash: &[u8], index: usize) {
    let Ok(id) = std::str::from_utf8(id) else { return };
    // Looked up in place: pieces are asked for up to 160 times a second.
    let Some((path, size, modified)) = library()
        .as_ref()
        .unwrap()
        .iter()
        .find(|j| shareable(j) && j.id == id && j.hash == hash)
        .map(|j| (j.path.clone(), j.size, j.modified))
    else {
        return;
    };
    // Read from the file alone: its fingerprint in the library holds while
    // it keeps the size and time it had (players asking for pieces of
    // different jobs at once made whole files read and hashed again and
    // again). A file written since is read again, another version of its
    // job.
    let Ok(mut file) = std::fs::File::open(&path) else { return };
    if !unchanged(&file, size, modified) {
        changed(&path);
        return;
    }
    let Some(piece) = read_piece(&mut file, size, index) else { return };
    let count = (size as usize).div_ceil(PIECE_SIZE);
    let mut p = vec![PIECE];
    p.extend_from_slice(id.as_bytes());
    p.extend_from_slice(hash);
    p.extend_from_slice(&(index as u32).to_be_bytes());
    p.extend_from_slice(&(count as u32).to_be_bytes());
    p.extend_from_slice(&piece);
    send(from, &p);
}

/// Whether a job's file is as it was read: its size and time.
fn unchanged(file: &std::fs::File, size: u32, modified: SystemTime) -> bool {
    file.metadata().is_ok_and(|m| m.len() == u64::from(size) && m.modified().ok() == Some(modified))
}

/// Piece `index` of a file of `size` bytes.
fn read_piece(file: &mut std::fs::File, size: u32, index: usize) -> Option<Vec<u8>> {
    let start = index.checked_mul(PIECE_SIZE).filter(|s| *s < size as usize)?;
    let mut piece = vec![0; PIECE_SIZE.min(size as usize - start)];
    file.seek(SeekFrom::Start(start as u64)).ok()?;
    file.read_exact(&mut piece).ok()?;
    Some(piece)
}

/// The job of a file written since it was read: read again, or dropped
/// when it is no longer one.
fn changed(path: &Path) {
    let job = library().as_ref().unwrap().iter().find(|j| j.path == path).map(|j| (j.id.clone(), j.kind, j.creator.clone()));
    let Some((id, kind, creator)) = job else { return };
    match read_job(path, &id, kind, &creator) {
        Some(job) => add(job),
        None => {
            log::info!("jobs: {id} is no longer a job");
            library().as_mut().unwrap().retain(|j| j.id != id);
        }
    }
}

fn on_piece(from: u32, id: &[u8], hash: &[u8], index: usize, count: usize, piece: &[u8]) {
    let Ok(id) = std::str::from_utf8(id) else { return };
    let mut guard = DOWNLOADS.locked();
    let Some(downloads) = guard.as_mut() else { return };
    let Some(d) = downloads.get_mut(id) else { return };
    if d.hash != hash || d.from != from || count != d.pieces.len() || index >= count {
        return;
    }
    d.progress = Instant::now();
    d.pieces[index] = Some(piece.to_vec());
    if d.pieces.iter().any(Option::is_none) {
        return;
    }
    let d = downloads.remove(id).unwrap();
    drop(guard);
    let data: Vec<u8> = d.pieces.into_iter().flatten().flatten().collect();
    if fingerprint(&data) != d.hash {
        log::warn!("jobs: {id} from {}: not what was announced", std::net::Ipv4Addr::from(from));
        return;
    }
    let path = shared_dir().join(format!("{id}.json"));
    // Kept only if it is a job.
    let Some(job) = parse_job(&data, id, Kind::Shared, "", &path) else { return };
    if let Err(e) = crate::write_file(&path, &data) {
        log::warn!("jobs: saving {}: {e}", path.display());
        return;
    }
    // Its author's version, which this player announces in turn.
    let written = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(UNIX_EPOCH + Duration::from_secs(d.version)));
    if let Err(e) = written {
        log::warn!("jobs: {}'s version: {e}", path.display());
    }
    add(Job { modified: modified(&path), ..job });
    make_room();
}

/// The lobby's jobs kept, at most: anyone in a public lobby can announce
/// some, one after the other.
const MAX_SHARED: usize = 256;
const MAX_SHARED_BYTES: u64 = 64 << 20;

/// Drops the first shared jobs (the oldest added) while there are too many.
fn make_room() {
    let mut dropped = Vec::new();
    {
        let mut library = library();
        let jobs = library.as_mut().unwrap();
        loop {
            let shared = jobs.iter().filter(|j| j.kind == Kind::Shared);
            let (count, bytes) = shared.fold((0, 0), |(c, b), j| (c + 1, b + u64::from(j.size)));
            if count <= MAX_SHARED && bytes <= MAX_SHARED_BYTES {
                break;
            }
            let first = jobs.iter().position(|j| j.kind == Kind::Shared).unwrap();
            dropped.push(jobs.remove(first));
        }
    }
    for job in dropped {
        log::info!("jobs: {} \"{}\" dropped (room for newer ones)", job.id, job.name);
        let _ = std::fs::remove_file(&job.path);
    }
}

// An announcement and a piece fit a datagram the Internet carries whole.
const _: () = assert!(1 + PER_ANNOUNCEMENT * ENTRY < 1200 && 1 + 22 + 8 + 8 + PIECE_SIZE < 1200);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_job_is_replaced_by_a_newer_version_only() {
        let (v1, v2) = ([1; 8], [2; 8]);
        assert!(wanted(None, &v1, 100));
        // Two versions in the lobby: the newer stays with everyone.
        assert!(wanted(Some((Kind::Shared, v1, 100)), &v2, 200));
        assert!(!wanted(Some((Kind::Shared, v2, 200)), &v1, 100));
        assert!(!wanted(Some((Kind::Shared, v2, 200)), &v1, 200));
        assert!(!wanted(Some((Kind::Shared, v2, 200)), &v2, 300));
        // The player's own and the game's: never.
        assert!(!wanted(Some((Kind::Own, v1, 100)), &v2, 200));
        assert!(!wanted(Some((Kind::Game, v1, 0)), &v2, 200));
    }

    #[test]
    fn pieces_are_read_while_the_file_is_as_it_was() {
        let path = std::env::temp_dir().join(format!("glue5-job-{}-{:x}.json", std::process::id(), crate::p2p::random()));
        let data: Vec<u8> = (0..2500u32).map(|i| i as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let (size, when) = (data.len() as u32, modified(&path));
        let mut file = std::fs::File::open(&path).unwrap();
        assert!(unchanged(&file, size, when));
        assert_eq!(read_piece(&mut file, size, 0).as_deref(), Some(&data[..1024]));
        assert_eq!(read_piece(&mut file, size, 2).as_deref(), Some(&data[2048..]));
        assert_eq!(read_piece(&mut file, size, 3), None);
        assert_eq!(read_piece(&mut file, size, usize::MAX), None);
        // Written again: another version.
        std::fs::write(&path, &data[..2000]).unwrap();
        assert!(!unchanged(&std::fs::File::open(&path).unwrap(), size, when));
        let _ = std::fs::remove_file(&path);
    }
}
