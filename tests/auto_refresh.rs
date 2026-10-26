//! The periodic-refresh door: `refresh_s` wired to the loop without a timer
//! thread — the loop asks every tick, the single-flight spawn stays the only
//! place a fetch starts.

use std::time::{Duration, Instant};

use gtasks::app::auto_refresh_due;

#[test]
fn zero_means_off_even_when_idle_long() {
    let last = Instant::now() - Duration::from_secs(3_600);
    assert!(!auto_refresh_due(last, Instant::now(), 0, false));
}

#[test]
fn busy_lane_never_opens() {
    let last = Instant::now() - Duration::from_secs(3_600);
    assert!(!auto_refresh_due(last, Instant::now(), 300, true));
}

#[test]
fn full_interval_opens_when_free() {
    let now = Instant::now();
    let last = now - Duration::from_secs(300);
    assert!(auto_refresh_due(last, now, 300, false));
}

#[test]
fn partial_interval_stays_shut() {
    let now = Instant::now();
    let last = now - Duration::from_secs(299);
    assert!(!auto_refresh_due(last, now, 300, false));
}
