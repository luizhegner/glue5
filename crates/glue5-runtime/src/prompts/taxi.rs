//! Taxi actions observed in the game's live instructional bar and input queries.
//! Reference: https://github.com/YimMenu/GTA-V-Decompiled-Scripts/blob/master/decompiled_scripts/taxiservice.c
use std::time::{Duration, Instant};

pub const CONFIRM_INPUT: u32 = 176;
pub const CONFIRM_GROUP: i32 = 2;
pub const STOP_INPUT: u32 = 177;
pub const HURRY_INPUT: u32 = 179;
pub const INPUTS: [u32; 3] = [CONFIRM_INPUT, STOP_INPUT, HURRY_INPUT];
pub const QUERY_FRESH: Duration = Duration::from_millis(250);

/// The script's actual checks, not a guess based on a vehicle model.
pub struct Queries([Option<Instant>; 3]);

impl Queries {
    pub const fn new() -> Self {
        Self([None; 3])
    }

    pub fn note(&mut self, group: i32, input: u32, now: Instant) {
        if group == CONFIRM_GROUP {
            if let Some(index) = INPUTS.iter().position(|i| *i == input) {
                self.0[index] = Some(now);
            }
        }
    }

    pub fn age(&self, input: u32, now: Instant) -> Option<Duration> {
        let index = INPUTS.iter().position(|i| *i == input)?;
        self.0[index].map(|at| now.saturating_duration_since(at))
    }

    pub fn recent(&self, input: u32, now: Instant) -> bool {
        self.age(input, now).is_some_and(|age| age < QUERY_FRESH)
    }
}

/// Associate the icon actually returned by GET_CONTROL_INSTRUCTIONAL_BUTTON
/// with its input. This preserves the game's translated label even for a
/// non-clickable keyboard slot, whose SET_DATA_SLOT uses the sentinel 361.
pub struct Tokens(Vec<(String, u32, Instant)>);

impl Tokens {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    pub fn note(&mut self, group: i32, input: u32, token: &str, now: Instant) {
        if group != CONFIRM_GROUP
            || !INPUTS.contains(&input)
            || token.len() > 96
            || !(token.starts_with("b_") || token.starts_with("t_"))
        {
            return;
        }
        self.0.retain(|(t, i, at)| now.saturating_duration_since(*at) < QUERY_FRESH && !(t == token && *i == input));
        if self.0.len() >= 12 {
            self.0.remove(0);
        }
        self.0.push((token.to_owned(), input, now));
    }

    pub fn input(&self, icons: &[String], now: Instant) -> Option<u32> {
        let mut found = None;
        for (token, input, at) in &self.0 {
            if now.saturating_duration_since(*at) >= QUERY_FRESH || !icons.contains(token) {
                continue;
            }
            if found.is_some_and(|previous| previous != *input) {
                return None;
            }
            found = Some(*input);
        }
        found
    }
}

pub fn offer(
    display_drawn: bool,
    passenger: bool,
    blocked: bool,
    query_age: Option<Duration>,
    already_shown: bool,
) -> bool {
    display_drawn && passenger && !blocked && !already_shown && query_age.is_some_and(|age| age < QUERY_FRESH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_active_taxi_passenger_recent_query_and_no_existing_confirmation() {
        let fresh = Some(Duration::from_millis(16));
        assert!(offer(true, true, false, fresh, false));
        assert!(!offer(false, true, false, fresh, false));
        assert!(!offer(true, false, false, fresh, false));
        assert!(!offer(true, true, true, fresh, false));
        assert!(!offer(true, true, false, None, false));
        assert!(!offer(true, true, false, Some(QUERY_FRESH), false));
        assert!(!offer(true, true, false, fresh, true));
    }

    #[test]
    fn taxi_queries_are_group_specific_independent_and_expire() {
        let now = Instant::now();
        let mut queries = Queries::new();
        queries.note(0, HURRY_INPUT, now);
        assert!(!queries.recent(HURRY_INPUT, now));
        queries.note(2, HURRY_INPUT, now);
        assert!(queries.recent(HURRY_INPUT, now));
        assert!(!queries.recent(CONFIRM_INPUT, now));
        assert!(!queries.recent(HURRY_INPUT, now + QUERY_FRESH));
    }

    #[test]
    fn keyboard_and_pad_slots_keep_their_logical_action_without_translation_matching() {
        let now = Instant::now();
        for token in ["t_KEY_X", "b_32"] {
            let mut tokens = Tokens::new();
            tokens.note(2, HURRY_INPUT, token, now);
            let icons = vec![token.to_owned()];
            assert_eq!(tokens.input(&icons, now), Some(HURRY_INPUT));
            assert_eq!(tokens.input(&icons, now + QUERY_FRESH), None);
            tokens.note(2, STOP_INPUT, token, now);
            assert_eq!(tokens.input(&icons, now), None, "ambiguous mappings must not be guessed");
        }
    }
}
