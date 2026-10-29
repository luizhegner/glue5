//! GTA Online's jobs (races, deathmatches, missions, ...), from the disk:
//! what Rockstar's UGC service would answer.
//!
//! A job is its mission file, the JSON the scripts load into a job (its
//! `mission.gen` holds the name, the type, the players, the start). There
//! are three kinds:
//! - the game data's (`common/data/ugc`: a race and the tutorial mission),
//!   listed as Rockstar Created;
//! - the player's, made with the Creator and saved in the user directory's
//!   `jobs/` (where jobs from elsewhere can be put too): My Jobs;
//! - the other players' of the lobby, which come over by themselves
//!   (share.rs) into `jobs/shared/`.
//!
//! The player's and the shared ones are listed as Rockstar Verified, the
//! lobby's jobs. Queries get the jobs' headers (`{"mission":{"gen":...}}`,
//! what the scripts read to list them and to place their coronas), then
//! the job's file is asked for by its content id.

mod share;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use glue5_game::{Env, orig};

use crate::Locked;
use crate::cloud::{NET_STATUS_FAILED, NET_STATUS_SUCCEEDED, finish};
use crate::memory::Mem;

pub(crate) use share::{PORT, receive};

/// `rlUgcContentType`: jobs are "gta5mission"s.
const CONTENT_MISSION: u32 = 1;

/// `rlUgcMetadata`: its fields (as `rlUgcTaskBase::ReadMetadataValue` fills
/// them from Rockstar's answer) and its size.
const MD_SIZE: u64 = 416;
const MD_TYPE: u64 = 0;
const MD_CONTENT_ID: u64 = 4;
const MD_CATEGORY: u64 = 32;
const MD_NAME: u64 = 36;
const MD_CREATED: u64 = 104;
const MD_DESCRIPTION: u64 = 112;
const MD_DATA: u64 = 160;
const MD_LANGUAGE: u64 = 208;
const MD_PUBLISHED: u64 = 216;
const MD_ROOT_CONTENT_ID: u64 = 232;
const MD_UPDATED: u64 = 256;
const MD_USER_ID: u64 = 264;
const MD_USER_NAME: u64 = 328;
const MD_VERSION: u64 = 392;
const MD_FILE_VERSIONS: u64 = 396;
/// A content id's characters (22, as Rockstar's) and the names' room.
const ID_LEN: usize = 22;
const ID_CAP: usize = ID_LEN + 1;
const NAME_CAP: usize = 64;
/// `rlUgcRatings`, left at zero.
const RATINGS_SIZE: u64 = 64;
/// When the game's jobs were made, as far as the scripts know (May 2014).
const DATE: i64 = 1_400_000_000;

/// `rlUgcCategory`s.
const CATEGORY_ROCKSTAR: u32 = 1;
const CATEGORY_VERIFIED: u32 = 3;
/// `rlScLanguage` English.
const LANGUAGE_ENGLISH: u32 = 1;

/// `netCloudRequestGetUgcFile`: what it asks for, where the file goes,
/// its status.
const REQUEST_STATUS: u64 = 8;
const REQUEST_BUFFER: u64 = 800;
const REQUEST_CONTENT_ID: u64 = 856;
const REQUEST_FILE: u64 = 880;
const REQUEST_LANGUAGE: u64 = 888;

/// `CUserContentManager`: the content the Creator made last (its
/// `rlUgcMetadata`), the create's status and state, the update's and the
/// publish's.
const MANAGER_CREATED: u64 = 872;
const MANAGER_CREATE_STATUS: u64 = 1704;
const MANAGER_MODIFY_STATUS: u64 = 1720;
const MANAGER_CREATE_STATE: u64 = 1736;
const MANAGER_MODIFY_STATE: u64 = 1744;
/// Their states: done (`HasCreateFinished`).
const OPERATION_FINISHED: u32 = 2;
/// `rlUgc::SourceFile`: its file id, data and size; the mission's is file 0.
const SOURCE_FILE_SIZE: u64 = 24;
const SOURCE_FILE_DATA: u64 = 8;
const SOURCE_FILE_LEN: u64 = 16;
const MISSION_FILE: u32 = 0;
/// No job file is larger.
const MAX_FILE: usize = 4 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Game,
    Own,
    Shared,
}

#[derive(Clone)]
struct Job {
    id: String,
    name: String,
    description: String,
    /// What the queries hand over: `{"mission":{"gen":...}}`.
    header: String,
    creator: String,
    kind: Kind,
    /// The game's: one file per language, `<path><language>_00.ugc`.
    path: PathBuf,
    /// The shared file's fingerprint (share.rs), its size, and when it was
    /// written.
    hash: [u8; 8],
    size: u32,
    modified: SystemTime,
}

static LIBRARY: Mutex<Option<Vec<Job>>> = Mutex::new(None);

fn library() -> MutexGuard<'static, Option<Vec<Job>>> {
    let mut library = LIBRARY.locked();
    if library.is_none() {
        let rt = crate::rt();
        let mut jobs = game_jobs(&crate::layout::data_dir(&rt.config.game_dir).join("common/data/ugc"));
        jobs.extend(dir_jobs(&own_dir(), Kind::Own));
        jobs.extend(dir_jobs(&shared_dir(), Kind::Shared));
        for job in &jobs {
            log::info!("jobs: {} \"{}\" ({:?}, {})", job.id, job.name, job.kind, job.creator);
        }
        *library = Some(jobs);
    }
    library
}

fn jobs() -> Vec<Job> {
    library().as_ref().unwrap().clone()
}

/// Adds a job, or replaces the one with its id.
fn add(job: Job) {
    let mut library = library();
    let jobs = library.as_mut().unwrap();
    jobs.retain(|j| j.id != job.id);
    log::info!("jobs: {} \"{}\" ({:?}, {})", job.id, job.name, job.kind, job.creator);
    jobs.push(job);
}

fn own_dir() -> PathBuf {
    crate::rt().config.user_dir.join("jobs")
}

fn shared_dir() -> PathBuf {
    own_dir().join("shared")
}

/// The game data's jobs: `<22-character id><language digit>_00.ugc`, the
/// English one (digit 0) for the header.
fn game_jobs(dir: &Path) -> Vec<Job> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut jobs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix("0_00.ugc").filter(|id| id.len() == ID_LEN) else { continue };
        if let Some(job) = read_job(&entry.path(), id, Kind::Game, "Rockstar") {
            jobs.push(Job { path: dir.join(id), ..job });
        }
    }
    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    jobs
}

/// The player's (or the lobby's) jobs: any `.json` mission file, its id its
/// name's if it is one, else made from its name.
fn dir_jobs(dir: &Path, kind: Kind) -> Vec<Job> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut jobs: Vec<Job> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_string_lossy().into_owned();
            let id = if is_content_id(&stem) { stem } else { content_id(&stem) };
            read_job(&e.path(), &id, kind, "")
        })
        .collect();
    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    jobs
}

fn is_content_id(s: &str) -> bool {
    s.len() == ID_LEN && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A content id for a name: 22 characters of base64url, as Rockstar's.
fn content_id(name: &str) -> String {
    use sha2::{Digest, Sha256};
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let hash = Sha256::digest(name.as_bytes());
    hash.iter().take(ID_LEN).map(|b| ALPHABET[(b & 63) as usize] as char).collect()
}

fn fingerprint(data: &[u8]) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)[..8].try_into().unwrap()
}

fn read_job(path: &Path, id: &str, kind: Kind, creator: &str) -> Option<Job> {
    // The time first: a file written meanwhile reads as changed (share.rs).
    let modified = modified(path);
    let data = std::fs::read(path).map_err(|e| log::warn!("jobs: {}: {e}", path.display())).ok()?;
    Some(Job { modified, ..parse_job(&data, id, kind, creator, path)? })
}

/// When a file was last written.
fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH)
}

fn parse_job(data: &[u8], id: &str, kind: Kind, creator: &str, path: &Path) -> Option<Job> {
    let mut json: serde_json::Value =
        serde_json::from_slice(data).map_err(|e| log::warn!("jobs: {}: {e}", path.display())).ok()?;
    unlock(&mut json);
    let general = json.pointer("/mission/gen")?;
    let name = general.get("nm").and_then(|n| n.as_str()).unwrap_or(id).to_owned();
    // The description comes in pieces of 64 characters.
    let description = match general.get("dec") {
        Some(serde_json::Value::Array(parts)) => parts.iter().filter_map(|p| p.as_str()).collect(),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let creator = match general.get("ownerid").and_then(|o| o.as_str()) {
        Some(owner) if creator.is_empty() && !owner.is_empty() => owner.to_owned(),
        _ if creator.is_empty() => "Glue 5".to_owned(),
        _ => creator.to_owned(),
    };
    let header = serde_json::json!({ "mission": { "gen": general } }).to_string();
    Some(Job {
        id: id.to_owned(),
        name,
        description,
        header,
        creator,
        kind,
        path: path.to_owned(),
        hash: fingerprint(data),
        size: data.len() as u32,
        modified: UNIX_EPOCH,
    })
}

/// The rank a job needs: the game data's are the tutorial's (rank 9999,
/// which no list shows), open to anyone here.
const TUTORIAL_RANK: u64 = 9999;

fn unlock(json: &mut serde_json::Value) -> bool {
    match json.pointer_mut("/mission/gen/rank") {
        Some(rank) if rank.as_u64().is_some_and(|r| r >= TUTORIAL_RANK) => {
            *rank = 1.into();
            true
        }
        _ => false,
    }
}

/// A job's file as the scripts get it (unlocked).
fn job_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&data) else { return Ok(data) };
    Ok(if unlock(&mut json) { json.to_string().into_bytes() } else { data })
}

impl Job {
    /// The job's file in `language` (an `rlScLanguage`), else in English.
    fn path(&self, language: u32) -> PathBuf {
        if self.kind != Kind::Game {
            return self.path.clone();
        }
        let digit = language_digit(language);
        let path = PathBuf::from(format!("{}{digit}_00.ugc", self.path.display()));
        if path.is_file() { path } else { PathBuf::from(format!("{}0_00.ugc", self.path.display())) }
    }

    fn category(&self) -> u32 {
        if self.kind == Kind::Game { CATEGORY_ROCKSTAR } else { CATEGORY_VERIFIED }
    }
}

/// The game's language digit (its `.ugc` files' suffix) for an
/// `rlScLanguage`.
fn language_digit(language: u32) -> char {
    match language {
        2 => '1',  // French
        3 => '2',  // German
        4 => '3',  // Italian
        11 => '4', // Spanish
        9 => '5',  // Portuguese (Brazil)
        7 => '6',  // Polish
        10 => '7', // Russian
        6 => '8',  // Korean
        0 => '9',  // Chinese
        5 => 'A',  // Japanese
        12 => 'B', // Spanish (Mexico)
        _ => '0',  // English
    }
}

/// The value of `key:"..."` in a query's parameters.
fn param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    let at = params.find(&format!("{key}:\""))? + key.len() + 2;
    params[at..].split('"').next()
}

/// The quoted strings after `key:[` in a query's parameters.
fn param_list<'a>(params: &'a str, key: &str) -> Vec<&'a str> {
    let Some(at) = params.find(&format!("{key}:[")) else { return Vec::new() };
    let list = params[at + key.len() + 2..].split(']').next().unwrap_or("");
    list.split(',').map(|s| s.trim().trim_matches(['"', '\''])).filter(|s| !s.is_empty()).collect()
}

/// The jobs a query asks for.
fn answer(query: &str, params: &str) -> Vec<Job> {
    let jobs = jobs();
    let category = |c: u32| jobs.iter().filter(|j| j.category() == c).cloned().collect();
    match query {
        "GetContentByCategory" => match param(params, "category") {
            Some("rstar") => category(CATEGORY_ROCKSTAR),
            Some("verif") => category(CATEGORY_VERIFIED),
            _ => Vec::new(),
        },
        "GetMyContent" => jobs.iter().filter(|j| j.kind == Kind::Own).cloned().collect(),
        "GetContentByContentId" | "GetLatestVersionByContentId" => {
            let ids = param_list(params, "contentids");
            jobs.iter().filter(|j| ids.contains(&j.id.as_str())).cloned().collect()
        }
        _ => Vec::new(),
    }
}

/// Hook for `rage::rlUgc::QueryContent(int, rlUgcContentType, char const*,
/// char const*, int, int, int*, int*, atDelegate<...> const&,
/// sysMemAllocator*, netStatus*)`: the jobs on the disk, handed over one by
/// one to the delegate as Rockstar's answer would be, the query done at
/// once, found or not: FM_maintain_cloud_header_data goes through its
/// queries one after the other, each asked again until it succeeds, and a
/// job's launch waits for the last.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn ugc_query_content(
    env: *mut Env,
    _gamer: u32,
    content_type: u32,
    query: u64,
    params: u64,
    offset: u32,
    count: u32,
    found: u64,
    total: u64,
    delegate: u64,
    _allocator: u64,
    status: u64,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    let (query, params) = (m.str(query), m.str(params));
    let jobs = if content_type == CONTENT_MISSION { answer(&query, &params) } else { Vec::new() };
    let page: Vec<_> = jobs.iter().skip(offset as usize).take(count as usize).collect();
    log::debug!("jobs: {query} {params}: {} of {}", page.len(), jobs.len());
    let ratings = unsafe { orig::builtin_malloc(env, RATINGS_SIZE) };
    m.bytes_mut(ratings, RATINGS_SIZE as usize).fill(0);
    for (i, job) in page.iter().enumerate() {
        let md = metadata(env, m, job, LANGUAGE_ENGLISH);
        unsafe { orig::ugc_result(env, delegate, i as u32, md + MD_CONTENT_ID, md, ratings, 0, 0, 0, 0) };
        unsafe {
            orig::grow_buffer_clear(env, md + MD_DATA);
            orig::grow_buffer_clear(env, md + MD_DESCRIPTION);
            orig::builtin_free(env, md);
        }
    }
    unsafe { orig::builtin_free(env, ratings) };
    if found != 0 {
        m.set_u32(found, page.len() as u32);
    }
    if total != 0 {
        m.set_u32(total, jobs.len() as u32);
    }
    finish(m, status, NET_STATUS_SUCCEEDED);
    1
}

/// A job's `rlUgcMetadata`, in the engine's heap.
fn metadata(env: *mut Env, m: Mem, job: &Job, language: u32) -> u64 {
    let md = unsafe { orig::builtin_malloc(env, MD_SIZE) };
    m.bytes_mut(md, MD_SIZE as usize).fill(0);
    unsafe { orig::ugc_metadata_new(env, md) };
    m.set_u32(md + MD_TYPE, CONTENT_MISSION);
    m.write_str(md + MD_CONTENT_ID, &job.id, ID_CAP);
    m.write_str(md + MD_ROOT_CONTENT_ID, &job.id, ID_CAP);
    m.set_u32(md + MD_CATEGORY, job.category());
    m.write_str(md + MD_NAME, &job.name, NAME_CAP);
    for date in [MD_CREATED, MD_PUBLISHED, MD_UPDATED] {
        m.set_i64(md + date, DATE);
    }
    m.set_u32(md + MD_LANGUAGE, language);
    m.write_str(md + MD_USER_ID, &job.creator, NAME_CAP);
    m.write_str(md + MD_USER_NAME, &job.creator, NAME_CAP);
    m.set_u32(md + MD_VERSION, 1);
    m.set_u32(md + MD_FILE_VERSIONS, 1);
    append(env, m, md + MD_DATA, job.header.as_bytes());
    append(env, m, md + MD_DESCRIPTION, job.description.as_bytes());
    md
}

/// Appends `data` to the `datGrowBuffer` at `buffer`.
fn append(env: *mut Env, m: Mem, buffer: u64, data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    let at = unsafe { orig::builtin_malloc(env, data.len() as u64) };
    if at == 0 {
        return false;
    }
    m.bytes_mut(at, data.len()).copy_from_slice(data);
    let appended = unsafe { orig::grow_buffer_append(env, buffer, at, data.len() as u32) } != 0;
    unsafe { orig::builtin_free(env, at) };
    appended
}

/// Hook for `rage::netCloudRequestGetUgcFile::DoRequest()`: a job's file
/// (file 0; photos, 1 and 2, there are none) from the disk.
pub(crate) unsafe extern "C" fn ugc_get_file(env: *mut Env, request: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    let id = m.str(request + REQUEST_CONTENT_ID);
    let status = request + REQUEST_STATUS;
    let job = library().as_ref().unwrap().iter().find(|j| j.id == id).cloned();
    let data = match job {
        Some(job) if m.u32(request + REQUEST_FILE) == MISSION_FILE => {
            let path = job.path(m.u32(request + REQUEST_LANGUAGE));
            job_file(&path).map_err(|e| log::warn!("jobs: {}: {e}", path.display())).ok()
        }
        _ => None,
    };
    match data {
        Some(data) if append(env, m, request + REQUEST_BUFFER, &data) => {
            log::info!("jobs: {id}'s file ({} bytes)", data.len());
            finish(m, status, NET_STATUS_SUCCEEDED);
        }
        _ => {
            log::info!("jobs: no file {id}");
            finish(m, status, NET_STATUS_FAILED);
        }
    }
    1
}

/// Hook for `rage::netCloudRequestGetMemberFile::CanStartRequest() const`
/// (a job's file request's too): it waits for Rockstar's credentials, and
/// the files come from the disk.
pub(crate) unsafe extern "C" fn ugc_can_start(_env: *mut Env, _request: u64) -> u32 {
    1
}

/// The mission file among the Creator's source files.
fn mission_file(m: Mem, files: u64, count: u32) -> Option<Vec<u8>> {
    (0..count as u64).map(|i| files + i * SOURCE_FILE_SIZE).find_map(|f| {
        let len = m.u32(f + SOURCE_FILE_LEN) as usize;
        let data = m.u64(f + SOURCE_FILE_DATA);
        (m.u32(f) == MISSION_FILE && data != 0 && len <= MAX_FILE).then(|| m.bytes(data, len).to_vec())
    })
}

/// Saves the Creator's job as `id`: in `jobs/`, the library, the lobby.
fn save(id: &str, data: &[u8]) -> bool {
    let dir = own_dir();
    let path = dir.join(format!("{id}.json"));
    if let Err(e) = crate::write_file(&path, data) {
        log::error!("jobs: saving {}: {e}", path.display());
        return false;
    }
    let creator = crate::rt().config.online.as_ref().map_or_else(String::new, |o| o.name.clone());
    match parse_job(data, id, Kind::Own, &creator, &path) {
        Some(job) => {
            let job = Job { modified: modified(&path), ..job };
            log::info!("jobs: saved \"{}\" as {}", job.name, path.display());
            add(job);
            true
        }
        None => false,
    }
}

/// Hook for `CUserContentManager::Create(char const*, char const*,
/// rlUgc::SourceFile const*, unsigned int, char const*, char const*,
/// rlUgcContentType, bool)`: the Creator saves a new job, to the disk, done
/// at once.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn ugc_create(
    env: *mut Env,
    manager: u64,
    name: u64,
    description: u64,
    files: u64,
    count: u32,
    tags: u64,
    data: u64,
    content_type: u32,
    publish: u32,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    if content_type != CONTENT_MISSION {
        return unsafe { orig::ugc_create(env, manager, name, description, files, count, tags, data, content_type, publish) };
    }
    let title = m.str(name);
    let id = content_id(&format!("{title} {} {:?}", gamer_tag(), std::time::SystemTime::now()));
    let saved = mission_file(m, files, count).is_some_and(|file| save(&id, &file));
    let created = manager + MANAGER_CREATED;
    // Its fields, around the two buffers: emptied as theirs (zeroed, they
    // would lose their allocator and grow step, the engine's next append
    // overflowing).
    m.bytes_mut(created, MD_DESCRIPTION as usize).fill(0);
    m.bytes_mut(created + MD_LANGUAGE, (MD_SIZE - MD_LANGUAGE) as usize).fill(0);
    unsafe {
        orig::grow_buffer_clear(env, created + MD_DESCRIPTION);
        orig::grow_buffer_clear(env, created + MD_DATA);
    }
    m.set_u32(created + MD_TYPE, CONTENT_MISSION);
    m.write_str(created + MD_CONTENT_ID, &id, ID_CAP);
    m.write_str(created + MD_NAME, &title, NAME_CAP);
    finish(m, manager + MANAGER_CREATE_STATUS, if saved { NET_STATUS_SUCCEEDED } else { NET_STATUS_FAILED });
    m.set_u32(manager + MANAGER_CREATE_STATE, OPERATION_FINISHED);
    1
}

/// Hook for `CUserContentManager::UpdateContent(char const* id, char const*
/// name, char const* description, char const* data, rlUgc::SourceFile
/// const*, unsigned int, char const* tags, rlUgcContentType)`: the Creator
/// saves a job again.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe extern "C" fn ugc_update(
    env: *mut Env,
    manager: u64,
    id: u64,
    name: u64,
    description: u64,
    data: u64,
    files: u64,
    count: u32,
    tags: u64,
    content_type: u32,
) -> u32 {
    let m = unsafe { Mem::of(env) };
    if content_type != CONTENT_MISSION {
        return unsafe {
            orig::ugc_update(env, manager, id, name, description, data, files, count, tags, content_type)
        };
    }
    let id = m.str(id);
    // A file missing is the header alone changing: nothing to save.
    let saved = is_content_id(&id) && mission_file(m, files, count).is_none_or(|file| save(&id, &file));
    finish(m, manager + MANAGER_MODIFY_STATUS, if saved { NET_STATUS_SUCCEEDED } else { NET_STATUS_FAILED });
    m.set_u32(manager + MANAGER_MODIFY_STATE, OPERATION_FINISHED);
    1
}

/// Hook for `CUserContentManager::Publish(char const*, char const*,
/// rlUgcContentType, char const*)`: the player's jobs are the lobby's
/// already (share.rs).
pub(crate) unsafe extern "C" fn ugc_publish(env: *mut Env, manager: u64, _id: u64, _base: u64, _kind: u32, _http: u64) -> u32 {
    let m = unsafe { Mem::of(env) };
    finish(m, manager + MANAGER_MODIFY_STATUS, NET_STATUS_SUCCEEDED);
    m.set_u32(manager + MANAGER_MODIFY_STATE, OPERATION_FINISHED);
    1
}

fn gamer_tag() -> String {
    crate::rt().config.online.as_ref().map_or_else(String::new, |o| o.name.clone())
}

/// This module's hooks, online.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.ugc_query_content = Some(ugc_query_content);
    hooks.ugc_get_file = Some(ugc_get_file);
    hooks.ugc_can_start = Some(ugc_can_start);
    hooks.ugc_create = Some(ugc_create);
    hooks.ugc_update = Some(ugc_update);
    hooks.ugc_publish = Some(ugc_publish);
    share::start();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parameters() {
        let p = r#"{lang:['en','fr'], category:"rstar"}"#;
        assert_eq!(param(p, "category"), Some("rstar"));
        assert_eq!(param_list(p, "lang"), ["en", "fr"]);
        assert_eq!(param_list(r#"{contentids:["fkg-TJjyiEqDtjKLRAdAMg"]}"#, "contentids"), ["fkg-TJjyiEqDtjKLRAdAMg"]);
    }

    #[test]
    fn content_ids() {
        let id = content_id("My race");
        assert!(is_content_id(&id));
        assert_eq!(id, content_id("My race"));
        assert!(!is_content_id("short"));
    }

    #[test]
    fn tutorial_jobs_are_unlocked() {
        let mut json = serde_json::json!({ "mission": { "gen": { "rank": 9999, "nm": "Race" } } });
        assert!(unlock(&mut json));
        assert_eq!(json.pointer("/mission/gen/rank"), Some(&1.into()));
        assert!(!unlock(&mut json));
    }
}
