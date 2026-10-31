# FilmCraft for iPadOS

FilmCraft's full desktop interface on an iPad, with touch, Apple Pencil, trackpad and keyboard.

```
packaging/ios/deploy.sh
```

builds the Rust engine, generates the Xcode project, signs it with your Apple ID and installs it on
the connected iPad (Developer Mode on). The first build takes 10 to 25 minutes; later ones a minute
or two. A free Apple ID signs builds that run for 7 days: run `deploy.sh` again to renew.

## How it is built

| Part | Where | What it does |
| --- | --- | --- |
| Rust host | `apps/filmcraft-ios` | `FilmcraftApp` (the desktop UI, unchanged) driven without eframe/winit: egui + wgpu on the view's `CAMetalLayer`, the frame loop, input, device profile |
| Swift app | `packaging/ios/FilmCraft` | UIKit scene life cycle (required by the iPadOS 27 SDK, which winit 0.30 predates), Metal view, display link, native toolbar, Photos / Files pickers, share sheet, keyboard |
| Engine | `crates/*` | VideoToolbox hardware decoding and encoding enabled on iOS; smaller caches on iOS |

### Input

- **One finger**: tap = click, drag = drag (from where the finger went down), hold = right click.
- **Two fingers**: scroll (the Timeline scrolls; vertically over tracks), pinch = zoom about the
  fingers (Timeline, monitors). Locked to one or the other at the first clear movement.
- **Apple Pencil**: precise clicks and drags (smaller slop, coalesced 240 Hz samples).
- **Trackpad / mouse**: hover, right button, two-finger scroll, pinch.
- **Keyboard**: all of FilmCraft's shortcuts with Command as on a Mac (J/K/L, I/O, Space, ⌘Z…),
  AZERTY and other layouts by the label on the key; arrows and delete repeat.
- **Toolbar**: import, save, export, share, undo/redo, tools, mark in/out, add edit, ripple delete,
  marker, frame step, play, timeline zoom.

### Files

Everything lives in the app's Documents folder, visible in Files ▸ On My iPad ▸ FilmCraft:
`Projects/`, `Media/` (imports from Photos or other locations are copied here so projects always
open), `Exports/`. Share an export to Photos with the toolbar's Share button.

### Device profile (`apps/filmcraft-ios/src/profile.rs`)

An iPad has no swap and no fan. Decoder, frame and export budgets follow the iPad's memory
(`crates/frame/src/device.rs`: 60 % of the 4 GB tuning on 3 GB, up to 300 % on 12-16 GB, never past the
desktop's). The host sizes the frame cache from the RAM (192 MB on a 4 GB iPad),
shrinks it on memory warnings, renders playback at 1/2 (HD) or 1/4 (UHD) and the paused frame at
full (HD) or 1/2 (UHD), lowers both when the iPad is hot, and draws up to 120 fps on ProMotion screens,
60 elsewhere or when warm, 30 when hot or in Low Power Mode, and only when something changes. Exports use the GPU compositor and the hardware
encoder by default on iOS. Choices made in the monitors' menus and in Settings are kept.

## Limits

- No background export: iPadOS stops GPU and hardware-encoder work in the background. Keep
  FilmCraft on screen while exporting (the screen stays on meanwhile).
- No plugins, no Whisper speech-to-text in this build (as on the desktop default build).
- Floating panels are drawn inside the window (no separate OS windows on an iPad).

## Testing without an iPad

```
packaging/ios/fixtures.sh                  # test media (ffmpeg): iPhone HEVC 4K, Dolby Vision, H.264
packaging/ios/sim.sh                       # Debug build in the "iPad (A16)" simulator (2360 × 1640)
packaging/ios/scenarios.py --loop 5        # menus, import, playback, editing, saving, 5 rounds
```

`sim.sh` launches the app with its control channel (Debug builds only) on 127.0.0.1:9876;
`scenarios.py` drives it there and saves screenshots in `target/ios-sim/shots`. The simulator has no
hardware video decoder, so playback there uses FilmCraft's own decoders (slower than an iPad).
`matrix.sh [rounds] [scenario…]` runs the scenarios on every screen size and generation iPadOS 27's
simulator offers (iPad 9 to iPad Pro 13-inch M5), creating the simulators it lacks.

## TestFlight

Needs a paid Apple Developer account and an app record in App Store Connect:

```
FILMCRAFT_TEAM=ABCDE12345 FILMCRAFT_BUNDLE_ID=org.example.filmcraft packaging/ios/testflight.sh
```

It archives a Release build, refuses it if `check-private.sh` finds this Mac's user name or folders in
the app's files, and uploads it. An App Store Connect API key is optional and stays outside the
repository (see the script's header).

## Privacy

No network code, telemetry or account (`FilmCraft/PrivacyInfo.xcprivacy` declares no collected data).
The control channel used by the test scripts is compiled into Debug builds only. Source paths are
rewritten at build time (`build-rust.sh`). `Generated/`, which holds your signing team, is not committed.
