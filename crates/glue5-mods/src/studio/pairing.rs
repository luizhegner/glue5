//! Who may use the Studio: browsers and agents paired with the game.
//!
//! The pairing code the app shows is exchanged once for a token of the
//! client's own (`POST /pair`): 32 random bytes, which the client keeps
//! (a page in its storage, an agent in its configuration) and sends from
//! then on instead of the code. The game keeps only each token's SHA-256,
//! with a name, a scope and when it was last used, in the mods folder
//! (`.glue5/pairings.json`): pairings outlive the game's runs and a new
//! code, the app lists them and removes them (which also closes their
//! connections), and one unused for [`EXPIRY`] goes by itself.
//!
//! A pairing's scope is what it may do: `Studio` everything (files, eval,
//! the type checker, MCP), `Control` only the control page's part (the
//! mods, starting and stopping them, their menus), so that a phone paired
//! to use the menus is no key to the game's code.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// What a credential may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Everything: the Studio, coding agents.
    Studio,
    /// The control page: the mods, starting and stopping them, their menus.
    Control,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Studio => "studio",
            Scope::Control => "control",
        }
    }

    pub fn parse(name: &str) -> Option<Scope> {
        match name {
            "studio" => Some(Scope::Studio),
            "control" => Some(Scope::Control),
            _ => None,
        }
    }
}

/// A paired client, as the app lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pairing {
    pub id: String,
    /// What the client is ("Safari on iPhone", "Claude Code").
    pub name: String,
    pub scope: Scope,
    /// Seconds since 1970.
    pub created: u64,
    pub last_used: u64,
}

/// A pairing unused this long goes.
pub const EXPIRY: Duration = Duration::from_secs(90 * 24 * 3600);
/// Pairings at most (the oldest unused goes for a new one).
const MAX: usize = 64;
/// How often a pairing's last use is written down (it is kept exact in
/// memory).
const NOTE_USE_EVERY: u64 = 3600;

struct Entry {
    pairing: Pairing,
    /// The token's SHA-256, hex.
    hash: String,
    /// The last use written to the file.
    noted: u64,
}

struct Store {
    /// The file (none: kept in memory only).
    path: Option<PathBuf>,
    entries: Vec<Entry>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// 32 random bytes, in the codes' alphabet (52 characters).
fn new_token() -> String {
    let mut bytes = [0u8; 33];
    getrandom::fill(&mut bytes).expect("random");
    // 5 bits a character: 264 bits read, 260 used.
    let mut bits = 0u32;
    let mut held = 0;
    let mut out = String::with_capacity(52);
    for b in bytes {
        bits = (bits << 8) | u32::from(b);
        held += 8;
        while held >= 5 && out.len() < 52 {
            held -= 5;
            out.push(super::ALPHABET[((bits >> held) & 31) as usize] as char);
        }
    }
    out
}

fn new_id() -> String {
    new_token()[..10].to_string()
}

impl Store {
    fn load(path: Option<PathBuf>) -> Store {
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| {
                let text = |k: &str| e[k].as_str().map(str::to_string);
                let pairing = Pairing {
                    id: text("id")?,
                    name: text("name").unwrap_or_default(),
                    scope: Scope::parse(e["scope"].as_str()?)?,
                    created: e["created"].as_u64().unwrap_or(0),
                    last_used: e["lastUsed"].as_u64().unwrap_or(0),
                };
                let noted = pairing.last_used;
                Some(Entry { pairing, hash: text("hash")?, noted })
            })
            .collect();
        // Expired ones go at the next check or listing.
        Store { path, entries }
    }

    fn save(&mut self) {
        for e in &mut self.entries {
            e.noted = e.pairing.last_used;
        }
        let Some(path) = &self.path else { return };
        let list: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                let p = &e.pairing;
                json!({ "id": p.id, "name": p.name, "scope": p.scope.name(), "created": p.created, "lastUsed": p.last_used, "hash": e.hash })
            })
            .collect();
        let text = serde_json::to_string_pretty(&list).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            log::warn!("studio: {}: {e}", path.display());
        }
    }

    /// Those unused for [`EXPIRY`] go. Whether any did.
    fn expire(&mut self, now: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| now.saturating_sub(e.pairing.last_used) < EXPIRY.as_secs());
        self.entries.len() != before
    }

    fn pair(&mut self, name: &str, scope: Scope, now: u64) -> (String, Pairing) {
        if self.entries.len() >= MAX
            && let Some(oldest) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.pairing.last_used).map(|(i, _)| i)
        {
            self.entries.remove(oldest);
        }
        let token = new_token();
        let name: String = name.trim().chars().take(60).collect();
        let pairing = Pairing { id: new_id(), name, scope, created: now, last_used: now };
        self.entries.push(Entry { pairing: pairing.clone(), hash: hash(&token), noted: now });
        self.save();
        (token, pairing)
    }

    /// The pairing `token` is, if any: its use is noted.
    fn check(&mut self, token: &str, now: u64) -> Option<Pairing> {
        if self.expire(now) {
            self.save();
        }
        let h = hash(token);
        let e = self.entries.iter_mut().find(|e| e.hash == h)?;
        e.pairing.last_used = now;
        let pairing = e.pairing.clone();
        if now.saturating_sub(e.noted) >= NOTE_USE_EVERY {
            self.save();
        }
        Some(pairing)
    }

    fn forget(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.pairing.id != id);
        let gone = self.entries.len() != before;
        if gone {
            self.save();
        }
        gone
    }
}

/// The pairings of the mods folder open now (none open: in memory).
static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut slot = crate::lock(&STORE);
    let path = crate::dir().map(|d| d.join(".glue5/pairings.json"));
    // Loaded the first time, and again for another folder (tests, the app's).
    if slot.as_ref().is_none_or(|s| s.path != path) {
        *slot = Some(Store::load(path));
    }
    f(slot.as_mut().unwrap())
}

/// A new pairing named `name`: its token, given once to the client.
pub fn pair(name: &str, scope: Scope) -> (String, Pairing) {
    with(|s| s.pair(name, scope, now()))
}

/// The pairing a token is, if it is one (its use is noted).
pub fn check(token: &str) -> Option<Pairing> {
    with(|s| s.check(token, now()))
}

/// Whether pairing `id` is still there (a connection of it stays open).
pub fn valid(id: &str) -> bool {
    with(|s| s.entries.iter().any(|e| e.pairing.id == id))
}

/// The pairings, most recently used first.
pub fn list() -> Vec<Pairing> {
    let mut list: Vec<Pairing> = with(|s| {
        if s.expire(now()) {
            s.save();
        }
        s.entries.iter().map(|e| e.pairing.clone()).collect()
    });
    list.sort_by_key(|p| std::cmp::Reverse(p.last_used));
    list
}

/// Removes pairing `id`: its token no longer works, its connections close.
pub fn forget(id: &str) -> bool {
    with(|s| s.forget(id))
}

/// Removes every pairing.
pub fn forget_all() {
    with(|s| {
        s.entries.clear();
        s.save();
    });
}

/// What a client is, from its User-Agent: "Safari on iPhone".
pub fn device_name(user_agent: &str) -> String {
    let ua = user_agent;
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("OPR/") {
        "Opera"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "A browser"
    };
    let device = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Macintosh") {
        "Mac"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        ""
    };
    if device.is_empty() { browser.to_string() } else { format!("{browser} on {device}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("glue5-pairings-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn tokens_are_long_and_new() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 52);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| super::super::ALPHABET.contains(&c)));
    }

    /// A pairing outlives the store (the game's next run), its token is the
    /// only way to it (the file has its hash), forgetting it ends it.
    #[test]
    fn pairings_are_kept_checked_and_forgotten() {
        let path = file("kept");
        let mut s = Store::load(Some(path.clone()));
        let (token, p) = s.pair("Safari on iPhone", Scope::Control, 1000);
        assert!(!std::fs::read_to_string(&path).unwrap().contains(&token), "only the hash is kept");
        let mut again = Store::load(Some(path.clone()));
        let found = again.check(&token, 2000).unwrap();
        assert_eq!((found.id.as_str(), found.scope, found.last_used), (p.id.as_str(), Scope::Control, 2000));
        assert!(again.check("nope", 2000).is_none());
        assert!(again.check(&token[..51], 2000).is_none());
        assert!(again.forget(&p.id));
        assert!(Store::load(Some(path.clone())).check(&token, 3000).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// Unused for 90 days: gone. A use resets the clock.
    #[test]
    fn unused_pairings_expire() {
        let path = file("expiry");
        let mut s = Store::load(Some(path.clone()));
        let day = 24 * 3600;
        let (old, _) = s.pair("old", Scope::Studio, 0);
        let (used, _) = s.pair("used", Scope::Studio, 0);
        assert!(s.check(&used, 60 * day).is_some());
        assert!(s.check(&old, 91 * day).is_none());
        assert!(s.check(&used, 120 * day).is_some());
        assert_eq!(s.entries.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn names_from_user_agents() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        let mac_chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
        let android = "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36";
        assert_eq!(device_name(iphone), "Safari on iPhone");
        assert_eq!(device_name(mac_chrome), "Chrome on Mac");
        assert_eq!(device_name(android), "Chrome on Android");
        assert_eq!(device_name("curl/8"), "A browser");
    }
}
