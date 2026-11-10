//! Narrow fallback for a taxi's missing instructional confirmation.
//! A public reference of taxiservice.c checks group 2, control 176 when
//! accepting a destination. Require live game observations, not vehicle model.
//! Reference: https://github.com/YimMenu/GTA-V-Decompiled-Scripts/blob/master/decompiled_scripts/taxiservice.c
use std::time::Duration;

pub const CONFIRM_INPUT: u32 = 176;
pub const CONFIRM_GROUP: i32 = 2;
pub const QUERY_FRESH: Duration = Duration::from_millis(250);

pub fn offer(display_drawn: bool, passenger: bool, blocked: bool, query_age: Option<Duration>, already_shown: bool) -> bool {
    display_drawn && passenger && !blocked && !already_shown
        && query_age.is_some_and(|age| age < QUERY_FRESH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_active_taxi_passenger_recent_query_and_no_existing_confirmation() {
        let fresh = Some(Duration::from_millis(16));
        assert!(offer(true,true,false,fresh,false));
        assert!(!offer(false,true,false,fresh,false));
        assert!(!offer(true,false,false,fresh,false));
        assert!(!offer(true,true,true,fresh,false));
        assert!(!offer(true,true,false,None,false));
        assert!(!offer(true,true,false,Some(QUERY_FRESH),false));
        assert!(!offer(true,true,false,fresh,true));
    }
}
