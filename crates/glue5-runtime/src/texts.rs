//! Texts the game data lacks: its scripts are newer and name labels of
//! later DLC packs, which show as "<LABEL> missing". The ones GTA Online's
//! jobs and activities show on the way (hosting, coronas, the job lists),
//! the pause menu's sessions, and the graphics settings the menu's data
//! lacks (`overlay.rs`), are given here, in English.

use std::collections::HashMap;
use std::sync::Mutex;

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// The pause menu's friends list with no friend in it (friends/game.rs):
/// its warning's text and title.
pub(crate) const NO_FRIENDS: &str = "GLUE5_NO_FRIENDS";
pub(crate) const NO_FRIENDS_TITLE: &str = "NOFRIENDS_TITLE";

/// Labels and their texts.
const TEXTS: &[(&str, &str)] = &[
    (NO_FRIENDS, "You have no friends yet. Add them on the Friends page: share your friend code, or ask a player you have played with."),
    (NO_FRIENDS_TITLE, "No Friends"),
    ("FACES_WARNH2", "Alert"),
    ("QM_NO_1", "No session of this Job was found to join. Would you like to host it?"),
    ("COR_YOUR_CHAR", "Your Character"),
    ("UI_FLOW_OP_NEWSESH", "Find New Session"),
    ("UI_FLOW_OP_NEWSESH_M", "Find New Session"),
    ("UI_FLOW_OP_NEWSESH_D", "Leave this session and play GTA Online in a new one."),
    ("UI_FLOW_SO_PBLCSESH", "Public Session"),
    ("UI_FLOW_SO_PBLCSESH_M", "Public Session"),
    ("UI_FLOW_SO_PBLCSESH_D", "Head straight into GTA Online."),
    ("UI_FLOW_SO_INVT", "Invite Only Session"),
    ("UI_FLOW_SO_INVT_M", "Invite Only Session"),
    ("UI_FLOW_SO_INVT_D", "Other players can only join if you invite them to play in this session."),
    ("UI_FLOW_SO_CRWSESH", "Crew Session"),
    ("UI_FLOW_SO_CRWSESH_M", "Crew Session"),
    (
        "UI_FLOW_SO_CRWSESH_D",
        "Head into GTA Online with your Crew and other Crews. Choose how many Crews to play with in this session.",
    ),
    ("UI_FLOW_SO_CLSDCRW", "Closed Crew Session"),
    ("UI_FLOW_SO_CLSDCRW_M", "Closed Crew Session"),
    ("UI_FLOW_SO_CLSDCRW_D", "Head into GTA Online with your Crew members only."),
    ("UI_FLOW_SO_CLSDFRND", "Closed Friend Session"),
    ("UI_FLOW_SO_CLSDFRND_M", "Closed Friend Session"),
    ("UI_FLOW_SO_CLSDFRND_D", "Head into GTA Online with your friends only."),
    ("UI_FLOW_SO_SOLO", "Solo Session"),
    ("UI_FLOW_SO_SOLO_M", "Solo Session"),
    (
        "UI_FLOW_SO_SOLO_D",
        "Head into GTA Online by yourself. Nobody else can join this session and available activities will be limited.",
    ),
    ("PM_EXIT_GAME", "Exit Game"),
    ("PM_ARENA", "Arena War"),
    ("PM_RC_STUNT", "Stunt Races"),
    ("RC_TARGASSAULT", "Target Assault Races"),
    ("FMMC_RSTAR_KOT", "King of the Hill"),
    ("PM_TNEWVS", "Adversary Modes"),
    ("FMMC_SR", "Stunt Race"),
    ("FMMC_OWR", "Open Wheel Race"),
    ("MATC_DPADRIGHT", "Press ~INPUT_CONTEXT~ to start."),
    ("PM_JOIN_AREN", "Join Arena War"),
    // Settings › Graphics and Advanced Graphics, as the PC game names them.
    ("GFX_POP_VARIETY", "Population Variety"),
    ("GFX_REFLECTION_MSAA", "Reflection MSAA"),
    ("GFX_PARTICLEQ", "Particles Quality"),
    ("GFX_GRASSQ", "Grass Quality"),
    ("GFX_MB_STRENGTH", "Motion Blur Strength"),
    ("GFX_DOF", "In-Game Depth Of Field Effects"),
    ("PM_PANE_ADVGFX", "Advanced Graphics"),
    ("GFX_LONG_SHADOWS", "Long Shadows"),
    ("GFX_ULTRA_SHADOWS", "High Resolution Shadows"),
    ("GFX_HD_FLIGHT", "High Detail Streaming While Flying"),
    ("GFX_MAX_LOD", "Extended Distance Scaling"),
    ("GFX_SHADOWS_DIST", "Extended Shadows Distance"),
];

/// The engine's label hash (`rage::atStringHash`): Jenkins' one-at-a-time,
/// case-insensitive.
pub(crate) fn label_hash(label: &str) -> u32 {
    let mut h: u32 = 0;
    for b in label.bytes() {
        h = h.wrapping_add(u32::from(b.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// The texts in the engine's heap, by hash (made when first asked for).
static PLACED: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);

/// Hook for `CTextFile::GetInternal(unsigned int)`: the game data's text,
/// else one of these.
pub(crate) unsafe extern "C" fn text_get_internal(env: *mut Env, hash: u32) -> u64 {
    let text = unsafe { orig::text_get_internal(env, hash) };
    if text != 0 {
        return text;
    }
    // Labels' hashes once: misses come every frame (labels the data lacks).
    static HASHES: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    let hashes = HASHES.get_or_init(|| TEXTS.iter().map(|(label, _)| label_hash(label)).collect());
    let Some((_, ours)) = TEXTS.iter().zip(hashes).find(|(_, h)| **h == hash).map(|(t, _)| t) else { return 0 };
    let mut placed = PLACED.lock().unwrap();
    let placed = placed.get_or_insert_default();
    if let Some(&at) = placed.get(&hash) {
        return at;
    }
    let at = unsafe { orig::builtin_malloc(env, ours.len() as u64 + 1) };
    if at != 0 {
        unsafe { Mem::of(env) }.write_str(at, ours, ours.len() + 1);
        placed.insert(hash, at);
    }
    at
}

pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.text_get_internal = Some(text_get_internal);
}

#[cfg(test)]
mod tests {
    #[test]
    fn label_hashes() {
        // Known joaat values.
        assert_eq!(super::label_hash("a"), 0xca2e9442);
        assert_eq!(super::label_hash("ADDER"), super::label_hash("adder"));
        assert_eq!(super::label_hash("adder"), 0xb779a091);
    }
}
