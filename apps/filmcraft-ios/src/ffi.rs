//! The C entry points the Swift side calls (declared in `packaging/ios/FilmCraft/FilmCraft.h`).
//!
//! Every function takes the host made by [`fc_host_create`] and is called on the main thread, where
//! UIKit delivers input and the display link fires. Each one runs under `catch_unwind`: a panic in
//! FilmCraft is logged (the crash log keeps it) and the app carries on rather than aborting through
//! the C boundary and losing the user's work. Strings are borrowed UTF-8 C strings, copied here.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::host::{Host, Paths};

/// A borrowed C string as a Rust string (empty when null or not UTF-8).
///
/// # Safety
/// `s` is null or a valid NUL-terminated string for the duration of the call.
unsafe fn string(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: non-null and NUL-terminated per the contract above.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

fn json_strings(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

/// Run `f` on the host, catching panics.
///
/// # Safety
/// `host` is null or a pointer returned by [`fc_host_create`] and not yet destroyed, used from the
/// main thread only.
unsafe fn with<R: Default>(host: *mut Host, f: impl FnOnce(&mut Host) -> R) -> R {
    if host.is_null() {
        return R::default();
    }
    // SAFETY: per the contract, `host` is a live, uniquely used (main thread) Host.
    let h = unsafe { &mut *host };
    match catch_unwind(AssertUnwindSafe(|| f(h))) {
        Ok(r) => r,
        Err(_) => {
            log::error!("FilmCraft caught an internal error and kept running");
            R::default()
        }
    }
}

/// Start FilmCraft on the `CAMetalLayer` at `layer` (`width` × `height` pixels, `scale` pixels per
/// point). `documents` is the app's Documents folder; `languages_json` the preferred languages
/// (`["fr-FR", …]`). Returns null when the GPU or the app could not start.
///
/// # Safety
/// `layer` is a live `CAMetalLayer` that outlives the host; string arguments are null or valid C
/// strings. Main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_create(
    layer: *mut c_void,
    width: u32,
    height: u32,
    scale: f32,
    documents: *const c_char,
    languages_json: *const c_char,
    ram_bytes: u64,
) -> *mut Host {
    // SAFETY: valid or null C strings per the contract.
    let (documents, languages) = unsafe { (string(documents), string(languages_json)) };
    let made = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(target_vendor = "apple")]
        // SAFETY: `layer` is a live CAMetalLayer outliving the host (the contract above).
        let painter = match unsafe { crate::gpu::Painter::for_layer(layer, [width, height]) } {
            Ok(p) => Some(p),
            Err(e) => {
                log::error!("no GPU drawing: {e}");
                return None;
            }
        };
        #[cfg(not(target_vendor = "apple"))]
        let painter = {
            let _ = layer;
            None
        };
        let paths = Paths { documents: documents.into() };
        Some(Box::new(Host::new(painter, [width, height], scale, paths, ram_bytes, json_strings(&languages))))
    }));
    match made {
        Ok(Some(h)) => Box::into_raw(h),
        _ => std::ptr::null_mut(),
    }
}

/// End FilmCraft (flush the recovery journal) and free the host.
///
/// # Safety
/// `host` is null or from [`fc_host_create`], not used afterwards. Main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_destroy(host: *mut Host) {
    if host.is_null() {
        return;
    }
    // SAFETY: from Box::into_raw in fc_host_create, destroyed once.
    let mut h = unsafe { Box::from_raw(host) };
    let _ = catch_unwind(AssertUnwindSafe(|| h.shutdown()));
}

/// Draw a frame. Returns the seconds until the next one is needed (infinity: none until input or
/// a wake-up).
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_frame(host: *mut Host) -> f64 {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| Some(h.frame())) }.unwrap_or(0.1)
}

/// The view's drawable size changed.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_resize(host: *mut Host, width: u32, height: u32, scale: f32) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.resize([width, height], scale)) }
}

/// A touch: `phase` 0 began, 1 moved, 2 ended, 3 cancelled; `kind` 0 finger, 1 Pencil, 2 trackpad
/// or mouse pointer; position in points.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_touch(host: *mut Host, id: u64, phase: i32, x: f32, y: f32, kind: i32, secondary: bool) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.touch(id, phase, x, y, kind, secondary)) }
}

/// A trackpad or mouse pointer hovering at (x, y), or leaving the view (`inside` false).
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_hover(host: *mut Host, x: f32, y: f32, inside: bool) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.hover(x, y, inside)) }
}

/// A trackpad scroll or mouse wheel, in points (positive moves the content right / down).
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_scroll(host: *mut Host, dx: f32, dy: f32, phase: i32) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.scroll(dx, dy, phase)) }
}

/// A trackpad pinch about (x, y): `factor` > 1 zooms in.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_pinch(host: *mut Host, factor: f32, x: f32, y: f32) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.pinch(factor, x, y)) }
}

/// A hardware key: HID usage, pressed or released, UIKit modifier flags, the characters typed and
/// the characters ignoring modifiers.
///
/// # Safety
/// See [`with`]; strings are null or valid C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_key(host: *mut Host, usage: u32, pressed: bool, repeat: bool, flags: u64, chars: *const c_char, unmodified: *const c_char) {
    // SAFETY: valid or null C strings per the contract.
    let (chars, unmodified) = unsafe { (string(chars), string(unmodified)) };
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.key(usage, pressed, repeat, flags, &chars, &unmodified)) }
}

/// Text from the on-screen keyboard.
///
/// # Safety
/// See [`with`]; `text` is null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_text(host: *mut Host, text: *const c_char) {
    // SAFETY: valid or null C string per the contract.
    let text = unsafe { string(text) };
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.text(&text)) }
}

/// The on-screen keyboard's delete key.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_backspace(host: *mut Host) {
    // SAFETY: forwarded contract.
    unsafe { with(host, Host::backspace) }
}

/// Command-V: the pasteboard's text.
///
/// # Safety
/// See [`with`]; `text` is null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_paste(host: *mut Host, text: *const c_char) {
    // SAFETY: valid or null C string per the contract.
    let text = unsafe { string(text) };
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.paste(&text)) }
}

/// A toolbar button: a command id (`edit.undo`, `playback.toggle`, `file.import`…).
///
/// # Safety
/// See [`with`]; `id` is null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_command(host: *mut Host, id: *const c_char) {
    // SAFETY: valid or null C string per the contract.
    let id = unsafe { string(id) };
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.command(&id)) }
}

/// Files given to FilmCraft from outside (`["/path", …]`): projects open, media is imported.
///
/// # Safety
/// See [`with`]; `paths_json` is null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_open_paths(host: *mut Host, paths_json: *const c_char) {
    // SAFETY: valid or null C string per the contract.
    let paths = json_strings(&unsafe { string(paths_json) });
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.open_paths(paths)) }
}

/// The answer to a file dialog the host asked for (`fcb_present_dialog`): the chosen paths as a
/// JSON array, empty when cancelled.
///
/// # Safety
/// `paths_json` is null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_dialog_answer(request: u64, paths_json: *const c_char) {
    // SAFETY: valid or null C string per the contract.
    let paths = json_strings(&unsafe { string(paths_json) });
    let _ = catch_unwind(|| crate::bridge::answer(request, paths));
}

/// iPadOS reported memory pressure.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_memory_warning(host: *mut Host) {
    // SAFETY: forwarded contract.
    unsafe { with(host, Host::memory_warning) }
}

/// `ProcessInfo.thermalState` changed: 0 nominal, 1 fair, 2 serious, 3 critical.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_thermal(host: *mut Host, state: i32) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.thermal(state)) }
}

/// Low Power Mode switched on or off.
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_low_power(host: *mut Host, on: bool) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| h.low_power(on)) }
}

/// The scene went to the background (or came back).
///
/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fc_host_set_active(host: *mut Host, active: bool) {
    // SAFETY: forwarded contract.
    unsafe { with(host, |h| if active { h.foreground() } else { h.background() }) }
}
