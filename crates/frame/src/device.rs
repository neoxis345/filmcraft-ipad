//! The memory of the device FilmCraft runs on, for the caches that size themselves to it.
//!
//! The iPadOS budgets in `codecs`, `frame` and `export` are tuned for a 4 GB iPad (A14). iPads
//! that still get updates go from 3 GB (iPad 8th and 9th generation, iPad mini 5, iPad Air 3) to
//! 16 GB (iPad Pro M4 / M5): a 3 GB one is killed by iPadOS well before a 4 GB one, and a 16 GB
//! one plays more smoothly with more decoded frames kept. The host tells this module the physical
//! memory once, before anything decodes ([`set_memory`]); every budget then goes through
//! [`scaled`]. Unset (the desktop, tests), budgets are left as they are.

use std::sync::atomic::{AtomicU64, Ordering};

static RAM: AtomicU64 = AtomicU64::new(0);

const GB: u64 = 1 << 30;

/// The device's physical memory in bytes (0: unknown, budgets unscaled).
pub fn set_memory(bytes: u64) {
    RAM.store(bytes, Ordering::Relaxed);
}

/// The physical memory [`set_memory`] recorded (0 when never set).
pub fn memory() -> u64 {
    RAM.load(Ordering::Relaxed)
}

/// How much of a 4 GB iPad's budget this much memory affords, in percent. iPadOS reports less than
/// the marketing size (a 3 GB iPad about 2.75 GiB, a 4 GB one about 3.6 GiB), hence the margins.
pub fn percent_for(ram: u64) -> u64 {
    match ram {
        0 => 100,
        r if r <= 3 * GB + GB / 4 => 60,
        r if r <= 4 * GB + GB / 2 => 100,
        r if r <= 6 * GB + GB / 2 => 150,
        r if r <= 8 * GB + GB / 2 => 200,
        _ => 300,
    }
}

/// `base`, a budget tuned for a 4 GB iPad, scaled to this device: never below `floor`, never above
/// `ceiling` (the desktop's value).
pub fn scaled(base: usize, floor: usize, ceiling: usize) -> usize {
    scaled_for(memory(), base, floor, ceiling)
}

/// [`scaled`] for a given amount of memory.
pub fn scaled_for(ram: u64, base: usize, floor: usize, ceiling: usize) -> usize {
    let pct = usize::try_from(percent_for(ram)).unwrap_or(100);
    let v = base.saturating_mul(pct) / 100;
    // `floor` above `ceiling` (a caller's mistake) must not make `clamp` panic
    v.max(floor).min(ceiling.max(floor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ipad_still_updated_gets_a_budget_for_its_memory() {
        let base = 288 << 20;
        let desktop = 1 << 30;
        // what iPadOS reports for 3, 4, 6, 8 and 16 GB iPads
        let shrunk = scaled_for(2_900_000_000, base, 64 << 20, desktop);
        let same = scaled_for(3_700_000_000, base, 64 << 20, desktop);
        let more = scaled_for(5_800_000_000, base, 64 << 20, desktop);
        let double = scaled_for(7_900_000_000, base, 64 << 20, desktop);
        let most = scaled_for(16 * GB, base, 64 << 20, desktop);
        assert!(shrunk < same && same < more && more < double && double <= most, "{shrunk} {same} {more} {double} {most}");
        assert_eq!(same, base, "a 4 GB iPad keeps the tuned budget");
        assert_eq!(most, base * 3);
        assert_eq!(scaled_for(16 * GB, 512 << 20, 0, desktop), desktop, "never more than the desktop");
        assert_eq!(scaled_for(0, base, 1, desktop), base, "unknown memory: unchanged");
    }

    #[test]
    fn counts_keep_their_floor_and_odd_bounds_do_not_panic() {
        assert_eq!(scaled_for(2 * GB, 2, 1, 8), 1, "60 % of 2 decoders rounds down to the floor");
        assert_eq!(scaled_for(16 * GB, 4, 1, 8), 8);
        assert_eq!(scaled_for(16 * GB, usize::MAX, 0, usize::MAX), usize::MAX / 100, "no overflow");
        assert_eq!(scaled_for(16 * GB, 4, 10, 2), 10, "floor above ceiling: the floor");
    }
}
