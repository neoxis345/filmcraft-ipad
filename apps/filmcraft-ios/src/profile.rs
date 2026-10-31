//! The device profile: what FilmCraft may ask of this iPad, and how much less when it is hot, low
//! on memory or saving battery.
//!
//! An iPad has no swap and no fan. Past its memory limit iPadOS kills the app; when it heats up it
//! slows the chip down (thermal throttling), and in Low Power Mode it asks apps to do less. None of
//! that can harm the iPad, but each makes editing feel broken, so the host plans ahead:
//!
//! - **Memory.** The decoded-frame cache is sized from the RAM (a 4 GB iPad gets a fraction of the
//!   desktop's 768 MB); a memory warning from iPadOS shrinks it at once and frees idle buffers,
//!   and it grows back only after a quiet minute.
//! - **Picture resolution.** The Program monitor of an 11-inch screen is about 1200 pixels wide,
//!   so rendering every frame of a 4K sequence at full size would be wasted work. Playback renders
//!   at 1/2 (HD) or 1/4 (UHD and above) and the paused frame at full (HD) or 1/2 (UHD), one step
//!   lower again when the iPad is hot. A resolution the user picked in the monitor's menu is left
//!   alone.
//! - **Frame rate.** The interface draws up to 120 fps (the iPad Pro's ProMotion screen; the Swift
//!   side holds every other iPad to its 60 Hz), 60 when warm, 30 when hot or in Low Power Mode, and
//!   only when something changes.

use filmcraft_ui_egui::state::PlaybackRes;

/// The thermal state iPadOS reports (`ProcessInfo.thermalState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Thermal {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl Thermal {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Thermal::Fair,
            2 => Thermal::Serious,
            3 => Thermal::Critical,
            _ => Thermal::Nominal,
        }
    }
}

/// How long after the last memory warning the cache stays small (seconds).
const MEMORY_QUIET: f64 = 60.0;
/// The cache while memory is tight (MB).
const TIGHT_CACHE_MB: u32 = 48;

/// What the profile asks for now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Settings ▸ Memory ▸ frame cache (MB): an upper bound on what the user set.
    pub frame_cache_mb: u32,
    /// Monitors' Playback Resolution (while playing).
    pub playback: PlaybackRes,
    /// Monitors' Paused Resolution.
    pub paused: PlaybackRes,
    /// Most frames per second the interface draws.
    pub max_fps: u32,
}

fn lower(r: PlaybackRes) -> PlaybackRes {
    match r {
        PlaybackRes::Full => PlaybackRes::Half,
        PlaybackRes::Half => PlaybackRes::Quarter,
        PlaybackRes::Quarter => PlaybackRes::Eighth,
        PlaybackRes::Eighth | PlaybackRes::Sixteenth => PlaybackRes::Sixteenth,
    }
}

/// The device and what is happening to it.
#[derive(Clone, Debug)]
pub struct Governor {
    /// Physical memory (bytes).
    ram: u64,
    thermal: Thermal,
    low_power: bool,
    /// When the last memory warning came (seconds on the host clock).
    last_warning: Option<f64>,
}

impl Governor {
    pub fn new(ram_bytes: u64) -> Self {
        Self { ram: ram_bytes, thermal: Thermal::Nominal, low_power: false, last_warning: None }
    }

    pub fn set_thermal(&mut self, t: Thermal) {
        self.thermal = t;
    }

    pub fn thermal(&self) -> Thermal {
        self.thermal
    }

    pub fn set_low_power(&mut self, on: bool) {
        self.low_power = on;
    }

    pub fn memory_warning(&mut self, now: f64) {
        self.last_warning = Some(now);
    }

    /// Whether memory is tight now (a warning within the last minute).
    pub fn memory_tight(&self, now: f64) -> bool {
        self.last_warning.is_some_and(|t| now - t < MEMORY_QUIET)
    }

    /// The frame cache this much RAM affords (MB).
    pub fn cache_for_ram(&self) -> u32 {
        const GB: u64 = 1 << 30;
        match self.ram {
            0 => 160,
            r if r <= 3 * GB => 128,
            r if r <= 4 * GB + GB / 2 => 192,
            r if r <= 6 * GB + GB / 2 => 384,
            _ => 768,
        }
    }

    /// The plan for a sequence of `width` × `height` pixels (None: no sequence open) at `now`.
    pub fn plan(&self, frame: Option<(u32, u32)>, now: f64) -> Plan {
        let pixels = frame.map_or(0u64, |(w, h)| u64::from(w).saturating_mul(u64::from(h)));
        // up to a little over 1080p is "HD"; DCI 2K and above count as UHD
        let uhd = pixels > 2_300_000;
        let (mut playback, mut paused) = if uhd { (PlaybackRes::Quarter, PlaybackRes::Half) } else { (PlaybackRes::Half, PlaybackRes::Full) };
        if self.ram > 0 && self.ram <= (2u64 << 30) {
            playback = lower(playback);
        }
        if self.thermal >= Thermal::Serious {
            playback = lower(playback);
            paused = lower(paused);
        }
        if self.thermal == Thermal::Critical {
            playback = lower(playback);
        }
        let frame_cache_mb = if self.memory_tight(now) { TIGHT_CACHE_MB } else { self.cache_for_ram() };
        let max_fps = match self.thermal {
            _ if self.low_power => 30,
            Thermal::Serious | Thermal::Critical => 30,
            Thermal::Fair => 60,
            Thermal::Nominal => 120,
        };
        Plan { frame_cache_mb, playback, paused, max_fps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn an_ipad_10_gets_a_small_cache_and_hd_settings() {
        let g = Governor::new(4 * GB);
        let p = g.plan(Some((1920, 1080)), 0.0);
        assert_eq!(p.frame_cache_mb, 192);
        assert_eq!((p.playback, p.paused), (PlaybackRes::Half, PlaybackRes::Full));
        assert_eq!(p.max_fps, 120, "as fast as the screen goes (the Swift side caps a 60 Hz iPad)");
        let mut warm = g.clone();
        warm.set_thermal(Thermal::Fair);
        assert_eq!(warm.plan(Some((1920, 1080)), 0.0).max_fps, 60);
    }

    #[test]
    fn uhd_renders_smaller_and_heat_lowers_it_again() {
        let mut g = Governor::new(4 * GB);
        let cool = g.plan(Some((3840, 2160)), 0.0);
        assert_eq!((cool.playback, cool.paused), (PlaybackRes::Quarter, PlaybackRes::Half));
        g.set_thermal(Thermal::Serious);
        let hot = g.plan(Some((3840, 2160)), 0.0);
        assert_eq!((hot.playback, hot.paused), (PlaybackRes::Eighth, PlaybackRes::Quarter));
        assert_eq!(hot.max_fps, 30);
        g.set_thermal(Thermal::Critical);
        assert_eq!(g.plan(Some((3840, 2160)), 0.0).playback, PlaybackRes::Sixteenth);
    }

    #[test]
    fn a_memory_warning_shrinks_the_cache_for_a_minute() {
        let mut g = Governor::new(4 * GB);
        g.memory_warning(10.0);
        assert_eq!(g.plan(None, 20.0).frame_cache_mb, TIGHT_CACHE_MB);
        assert_eq!(g.plan(None, 10.0 + MEMORY_QUIET + 1.0).frame_cache_mb, 192);
    }

    #[test]
    fn low_power_halves_the_frame_rate_and_odd_sizes_do_not_overflow() {
        let mut g = Governor::new(8 * GB);
        g.set_low_power(true);
        let p = g.plan(Some((u32::MAX, u32::MAX)), 0.0);
        assert_eq!(p.max_fps, 30);
        assert_eq!(p.frame_cache_mb, 768);
        assert_eq!(Governor::new(0).cache_for_ram(), 160);
        assert_eq!(Thermal::from_raw(99), Thermal::Nominal);
    }
}
