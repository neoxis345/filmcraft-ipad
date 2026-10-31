//! FilmCraft for iPadOS: the Rust half of the app.
//!
//! The Swift half (`packaging/ios/FilmCraft`) is a UIKit app with the scene life cycle: it owns the
//! window, a Metal-backed view, the native toolbar, the display link and the system pickers, and it
//! calls the C entry points in [`ffi`]. This crate runs FilmCraft's unchanged desktop interface
//! (`FilmcraftApp`) on that view ([`host`]), turns touches, the Pencil, trackpads and hardware
//! keyboards into the input the panels expect ([`touch`], [`keys`]), and sizes the caches and the
//! picture to the iPad it runs on ([`profile`]).
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod bridge;
pub mod ffi;
pub mod gpu;
pub mod host;
pub mod keys;
pub mod profile;
pub mod touch;

// The control channel is deliberately compiled only into debug/test builds. The XCUITest bench
// opts into it with a 64-hex-character launch environment token; Release/TestFlight has no module
// or listener to accidentally expose.
#[cfg(debug_assertions)]
#[path = "../../filmcraft/src/control_server.rs"]
mod control_server;

// The desktop app's audio output (the playback clock), voice-over input and logging, shared as
// source rather than copied.
#[path = "../../filmcraft/src/audio.rs"]
mod audio;
#[path = "../../filmcraft/src/audio_in.rs"]
mod audio_in;
#[path = "../../filmcraft/src/logging.rs"]
#[allow(dead_code)]
mod logging;
