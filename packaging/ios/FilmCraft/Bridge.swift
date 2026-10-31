// The functions the Rust host calls (apps/filmcraft-ios/src/bridge.rs). All but
// `fcb_request_redraw` are called on the main thread, during a frame; strings are borrowed for the
// call and copied here.

import UIKit

private let wakeLock = NSLock()
private var wakeQueued = false
private var wakeDelay = Double.infinity

/// A frame is wanted after `delay` seconds. Called from any thread (FilmCraft's decoders, exports
/// and audio clock ask for frames): coalesced into one hop to the main queue.
@_cdecl("fcb_request_redraw")
public func fcb_request_redraw(_ delay: Double) {
    wakeLock.lock()
    wakeDelay = min(wakeDelay, delay)
    let queue = !wakeQueued
    wakeQueued = true
    wakeLock.unlock()
    guard queue else { return }
    DispatchQueue.main.async {
        wakeLock.lock()
        let d = wakeDelay
        wakeDelay = .infinity
        wakeQueued = false
        wakeLock.unlock()
        FilmCraftViewController.current?.requestFrame(after: d)
    }
}

private func string(_ p: UnsafePointer<CChar>?) -> String {
    guard let p else { return "" }
    return String(cString: p)
}

@_cdecl("fcb_set_keyboard_visible")
public func fcb_set_keyboard_visible(_ visible: Bool) {
    // after the frame: UIKit's keyboard animation must not run inside the host's call
    DispatchQueue.main.async { FilmCraftViewController.current?.setKeyboardVisible(visible) }
}

@_cdecl("fcb_set_clipboard")
public func fcb_set_clipboard(_ text: UnsafePointer<CChar>?) {
    UIPasteboard.general.string = string(text)
}

@_cdecl("fcb_present_dialog")
public func fcb_present_dialog(_ request: UInt64, _ json: UnsafePointer<CChar>?) {
    let spec = string(json)
    DispatchQueue.main.async {
        guard let vc = FilmCraftViewController.current else {
            "[]".withCString { fc_dialog_answer(request, $0) }
            return
        }
        vc.dialogs.present(request: request, json: spec)
    }
}

@_cdecl("fcb_open_url")
public func fcb_open_url(_ url: UnsafePointer<CChar>?) {
    guard let u = URL(string: string(url)) else { return }
    DispatchQueue.main.async { UIApplication.shared.open(u) }
}

@_cdecl("fcb_share_file")
public func fcb_share_file(_ path: UnsafePointer<CChar>?) {
    let p = string(path)
    DispatchQueue.main.async {
        guard let vc = FilmCraftViewController.current, FileManager.default.fileExists(atPath: p) else { return }
        // Save to Photos (videos and images), AirDrop, Save to Files, Messages, Mail…
        let share = UIActivityViewController(activityItems: [URL(fileURLWithPath: p)], applicationActivities: nil)
        vc.dialogs.anchor(share, in: vc)
        vc.present(share, animated: true)
    }
}

@_cdecl("fcb_status")
public func fcb_status(_ json: UnsafePointer<CChar>?) {
    let s = string(json)
    guard let data = s.data(using: .utf8), let status = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return }
    DispatchQueue.main.async { FilmCraftViewController.current?.apply(status: status) }
}
