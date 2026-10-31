//! Calls from Rust into the Swift side of the app (UIKit work: pickers, clipboard, keyboard, share
//! sheet, waking the display link), and the file dialogs waiting for an answer.
//!
//! On iPadOS these are C functions the Swift side exports (`@_cdecl`, see
//! `packaging/ios/FilmCraft/Bridge.swift`). Everywhere else they are recorded, so the host logic is
//! tested without UIKit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, PoisonError};

use filmcraft_ui_egui::native_dialogs::FileDialog;
use serde_json::json;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod sys {
    use std::ffi::c_char;
    // Implemented in Swift (Bridge.swift). Strings are borrowed for the call; Swift copies them.
    unsafe extern "C" {
        pub fn fcb_request_redraw(delay_seconds: f64);
        pub fn fcb_set_keyboard_visible(visible: bool);
        pub fn fcb_set_clipboard(text: *const c_char);
        pub fn fcb_present_dialog(request: u64, json: *const c_char);
        pub fn fcb_open_url(url: *const c_char);
        pub fn fcb_share_file(path: *const c_char);
        pub fn fcb_status(json: *const c_char);
    }
}

/// What the host asked the Swift side for (recorded off iPadOS, for the tests).
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Redraw(f64),
    Keyboard(bool),
    Clipboard(String),
    Dialog(u64, String),
    OpenUrl(String),
    Share(String),
    Status(String),
}

#[cfg(not(target_os = "ios"))]
static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());

/// The calls made so far (off iPadOS), oldest first, and forget them.
#[cfg(not(target_os = "ios"))]
pub fn take_calls() -> Vec<Call> {
    std::mem::take(&mut *CALLS.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The calls made so far (off iPadOS), without forgetting them (tests running side by side).
#[cfg(not(target_os = "ios"))]
pub fn peek_calls() -> Vec<Call> {
    CALLS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A C string for Swift: interior NULs (hostile file names) are dropped rather than failing.
#[cfg(target_os = "ios")]
fn c(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default()
}

#[allow(unsafe_code)]
fn call(c_: Call) {
    #[cfg(target_os = "ios")]
    // SAFETY: each function is the Swift export of the same signature (Bridge.swift); string
    // arguments are valid NUL-terminated C strings for the duration of the call and Swift copies
    // them before returning. `fcb_request_redraw` may be called from any thread (Swift hops to the
    // main queue); the others are called from the main thread, where the host runs.
    unsafe {
        match &c_ {
            Call::Redraw(d) => sys::fcb_request_redraw(*d),
            Call::Keyboard(v) => sys::fcb_set_keyboard_visible(*v),
            Call::Clipboard(t) => sys::fcb_set_clipboard(c(t).as_ptr()),
            Call::Dialog(r, j) => sys::fcb_present_dialog(*r, c(j).as_ptr()),
            Call::OpenUrl(u) => sys::fcb_open_url(c(u).as_ptr()),
            Call::Share(p) => sys::fcb_share_file(c(p).as_ptr()),
            Call::Status(j) => sys::fcb_status(c(j).as_ptr()),
        }
    }
    #[cfg(not(target_os = "ios"))]
    CALLS.lock().unwrap_or_else(PoisonError::into_inner).push(c_);
}

/// Draw a frame after `delay` seconds (0: at the next display refresh). Any thread.
pub fn request_redraw(delay: f64) {
    call(Call::Redraw(if delay.is_finite() { delay.max(0.0) } else { f64::MAX }));
}

pub fn set_keyboard_visible(visible: bool) {
    call(Call::Keyboard(visible));
}

pub fn set_clipboard(text: &str) {
    call(Call::Clipboard(text.to_string()));
}

pub fn open_url(url: &str) {
    call(Call::OpenUrl(url.to_string()));
}

pub fn share_file(path: &str) {
    call(Call::Share(path.to_string()));
}

/// State the native toolbar shows (playing, busy with an export, unsaved changes…), as JSON.
pub fn status(json: &serde_json::Value) {
    call(Call::Status(json.to_string()));
}

// ------------------------------------------------------------------------------------ dialogs

static NEXT_DIALOG: AtomicU64 = AtomicU64::new(1);
static DIALOGS: Mutex<Option<HashMap<u64, Sender<Vec<String>>>>> = Mutex::new(None);

/// The dialog as the Swift side reads it.
pub fn dialog_json(d: &FileDialog) -> serde_json::Value {
    match d {
        FileDialog::OpenFiles { exts } => json!({"kind": "importMedia", "exts": exts}),
        FileDialog::OpenFile { filter, exts } => json!({"kind": "openFile", "filter": filter, "exts": exts}),
        FileDialog::OpenProject => json!({"kind": "openProject", "exts": ["fcproj"]}),
        FileDialog::SaveProject { name } => json!({"kind": "saveProject", "name": name, "exts": ["fcproj"]}),
        FileDialog::SaveAs { filter, exts, name } => json!({"kind": "saveAs", "filter": filter, "exts": exts, "name": name}),
        FileDialog::Folder { at } => json!({"kind": "folder", "at": at}),
        FileDialog::Relink { exts, .. } => json!({"kind": "relink", "exts": exts}),
    }
}

/// Show `dialog` (HostHooks::run_dialog): the answer comes on the receiver when the user is done
/// (no paths: cancelled).
pub fn present(dialog: FileDialog) -> Receiver<Vec<String>> {
    let (tx, rx) = channel();
    let id = NEXT_DIALOG.fetch_add(1, Ordering::Relaxed);
    DIALOGS.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(id, tx);
    call(Call::Dialog(id, dialog_json(&dialog).to_string()));
    rx
}

/// The Swift side's answer to dialog `id`: the chosen paths (empty: cancelled). An unknown id (a
/// stale answer) is ignored.
pub fn answer(id: u64, paths: Vec<String>) {
    let tx = DIALOGS.lock().unwrap_or_else(PoisonError::into_inner).as_mut().and_then(|m| m.remove(&id));
    if let Some(tx) = tx {
        let _ = tx.send(paths);
    }
    request_redraw(0.0);
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_is_answered_once_and_stale_answers_are_ignored() {
        let rx = present(FileDialog::SaveProject { name: "Mon film.fcproj".into() });
        let id = peek_calls()
            .into_iter()
            .find_map(|c| match c {
                Call::Dialog(id, j) if j.contains("saveProject") && j.contains("Mon film") => Some(id),
                _ => None,
            })
            .unwrap_or(0);
        assert!(id > 0);
        answer(id, vec!["/x/Mon film.fcproj".into()]);
        assert_eq!(rx.try_recv().ok(), Some(vec!["/x/Mon film.fcproj".to_string()]));
        answer(id, vec!["again".into()]);
        answer(u64::MAX, vec![]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn every_dialog_kind_names_itself() {
        let kinds = [
            FileDialog::OpenFiles { exts: vec!["mp4".into()] },
            FileDialog::OpenFile { filter: "LUT".into(), exts: vec!["cube".into()] },
            FileDialog::OpenProject,
            FileDialog::SaveAs { filter: "Media".into(), exts: vec!["mp4".into()], name: "a.mp4".into() },
            FileDialog::Folder { at: None },
        ];
        for k in kinds {
            assert!(dialog_json(&k)["kind"].as_str().is_some_and(|s| !s.is_empty()));
        }
    }
}
