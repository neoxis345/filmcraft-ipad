//! The iPad host: FilmCraft's egui app driven by the UIKit view instead of eframe and winit.
//!
//! eframe runs on winit, whose iOS backend predates UIKit's scene life cycle, which apps built with
//! the iPadOS 27 SDK must adopt or they do not launch. So the Swift side owns the scene, the view,
//! the display link and every input, and calls this host: touches, keys and text go in, a frame
//! comes out on the view's Metal layer. The app itself is the desktop's `FilmcraftApp`, unchanged:
//! the same `App::logic` / `App::ui` calls eframe makes, with an `eframe::Frame` it never reads.

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{Event, Modifiers, Pos2, Rect, ViewportId, pos2, vec2};
use filmcraft_engine::Session;
use filmcraft_engine::autosave::{AutosaveConfig, default_data_dir};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::state::PlaybackRes;
use serde_json::json;

use crate::bridge;
use crate::gpu::Painter;
use crate::keys;
use crate::profile::{Governor, Plan, Thermal};
use crate::touch::{Kind, Phase, Touch, TouchLayer};

/// Where the app keeps its files (inside its container).
#[derive(Clone, Debug, Default)]
pub struct Paths {
    /// `Documents`: projects, imported media and exports, visible in the Files app.
    pub documents: PathBuf,
}

/// The largest view side (pixels) the host accepts: bigger than any iPad screen or external display.
const MAX_SIDE: u32 = 8192;

/// Commands the native toolbar sends that the host runs itself (the rest are FilmCraft's).
pub const SHARE_LAST_EXPORT: &str = "ios.shareLastExport";

/// The GPU export frame renderer (as the desktop app registers it).
struct GpuFrameRenderer(filmcraft_gpu::ExportRenderer);

impl filmcraft_export::FrameRenderer for GpuFrameRenderer {
    fn render(
        &mut self,
        project: &filmcraft_project::Project,
        seq: filmcraft_project::ItemId,
        t: filmcraft_time::Tick,
        opts: filmcraft_render::RenderOptions,
        sources: &dyn filmcraft_render::SourceProvider,
    ) -> Option<filmcraft_render::Image> {
        self.0.render(project, seq, t, opts, sources)
    }
}

fn local_offset(unix: i64) -> i32 {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(unix, 0).single().map(|d| d.offset().local_minus_utc()).unwrap_or(0)
}

/// Settings the host has applied, so it changes them only when the plan changes and leaves alone
/// what the user picked since.
#[derive(Default)]
struct Applied {
    plan: Option<Plan>,
    /// (cap, the user's setting) the frame cache was last set from.
    cache: Option<(u32, u32)>,
    /// The monitor resolutions the host last set: (playing, paused).
    res: Option<(PlaybackRes, PlaybackRes)>,
}

pub struct Host {
    ctx: egui::Context,
    app: FilmcraftApp,
    frame: eframe::Frame,
    painter: Option<Painter>,
    touch: TouchLayer,
    /// Keyboard and text events for the next frame.
    keys: Vec<Event>,
    modifiers: Modifiers,
    start: Instant,
    size_px: [u32; 2],
    scale: f32,
    focused: bool,
    commands: Sender<String>,
    governor: Governor,
    applied: Applied,
    keyboard_visible: bool,
    last_status: String,
    /// The pointer shape last sent to the Swift side (trackpad and mouse).
    last_cursor: &'static str,
    paths: Paths,
}

impl Host {
    /// Start FilmCraft. `painter` is None without a GPU (tests): the monitors then composite on the
    /// CPU and nothing is drawn.
    pub fn new(painter: Option<Painter>, size_px: [u32; 2], scale: f32, paths: Paths, ram_bytes: u64, languages: Vec<String>) -> Self {
        // first of all: the decoders' and exports' budgets follow this iPad's memory (3 GB iPad 9 to
        // 16 GB iPad Pro), and they are read when the first source opens
        filmcraft_frame::device::set_memory(ram_bytes);
        let data_dir = default_data_dir();
        let log_dir = data_dir.as_ref().map(|d| d.join(crate::logging::LOG_DIR));
        if let (Some(logger), Some(dir)) = (crate::logging::install(), log_dir.as_deref()) {
            match logger.attach_dir(dir) {
                Ok(p) => log::info!("log file {}", p.display()),
                Err(e) => log::warn!("no log file: {e}"),
            }
        }
        log::info!("FilmCraft {} for iPadOS, {:.1} GB of memory", env!("CARGO_PKG_VERSION"), ram_bytes as f64 / f64::from(1u32 << 30));
        filmcraft_ui_egui::crash::install(log_dir);
        let hardware = filmcraft_platform::register();
        log::info!("hardware decoding: {hardware:?}");
        if painter.is_some() {
            filmcraft_export::register_frame_renderer(|| {
                filmcraft_gpu::ExportRenderer::new().map(|r| Box::new(GpuFrameRenderer(r)) as Box<dyn filmcraft_export::FrameRenderer>)
            });
        }
        for sub in ["Projects", "Media", "Exports"] {
            if let Err(e) = std::fs::create_dir_all(paths.documents.join(sub)) {
                log::warn!("can't create Documents/{sub}: {e}");
            }
        }

        let mut session = Session::default();
        let first_launch = data_dir.as_ref().is_some_and(|d| !d.join(IPAD_PROFILE_MARKER).exists());
        if let Some(dir) = data_dir.clone() {
            let mut cfg = AutosaveConfig::new(dir);
            cfg.local_offset = local_offset;
            if let Err(e) = session.start_autosave(cfg) {
                log::warn!("auto-save and crash recovery unavailable: {e}");
            }
        }
        if first_launch {
            first_launch_settings(&mut session);
            if let Some(d) = &data_dir
                && let Err(e) = std::fs::write(d.join(IPAD_PROFILE_MARKER), b"1")
            {
                log::warn!("can't record the iPad settings: {e}");
            }
        }
        session.voiceover.input = Some(Box::new(crate::audio_in::CpalIn::new(&session.prefs.audio_hardware.device_class)));
        if session.prefs.general.at_startup == "openMostRecent" {
            let recent = session.prefs.general.recent_projects.iter().find(|p| std::path::Path::new(p).exists()).cloned();
            let opened = recent.map(|p| session.execute("file.open", json!({"path": p})));
            if !matches!(opened, Some(Ok(_))) {
                let _ = session.execute("file.openDemoProject", json!({}));
            }
        } else if session.prefs.general.at_startup != "emptyProject" {
            let _ = session.execute("file.openDemoProject", json!({}));
        }

        let ctx = egui::Context::default();
        // no OS windows on an iPad: any extra viewport is drawn inside the main one
        ctx.set_embed_viewports(true);
        ctx.options_mut(|o| {
            // a fingertip lands a few points off and lingers: clicks tolerate both
            o.input_options.max_click_dist = 14.0;
            o.input_options.max_click_duration = 0.6;
        });
        // FilmCraft's worker threads (decoding, exports, renders) ask for frames: wake the display link
        ctx.set_request_repaint_callback(|info| bridge::request_redraw(info.delay.as_secs_f64()));

        let mut app = FilmcraftApp::new(session);
        // the in-window menus (File, Edit, Clip…) stay: an iPad has no system menu bar to hold
        // them. The status row goes: the native toolbar sits at the bottom instead.
        app.ui.show_menu_bar = true;
        app.ui.show_status_bar = false;
        if let Some(p) = &painter {
            app.set_wgpu(p.render_state.clone());
        }
        app.audio = Some(Box::new(crate::audio::CpalOut::new()));
        app.hooks.run_dialog = Some(Box::new(bridge::present));
        app.hooks.open_url = Some(Box::new(|url: &str| {
            bridge::open_url(url);
            Ok(())
        }));
        // Edit Original, Reveal in Finder, Reveal Log Files: the share sheet (Save to Files, AirDrop…)
        app.hooks.open_path = Some(Box::new(|path: &str, _reveal: bool| {
            bridge::share_file(path);
            Ok(())
        }));
        app.hooks.system_languages = Some(Box::new(move || languages.clone()));
        let (tx, rx) = channel();
        app.command_inbox = Some(rx);
        #[cfg(debug_assertions)]
        if std::env::var("FILMCRAFT_CONTROL_TOKEN").is_ok() {
            let port = std::env::var("FILMCRAFT_CONTROL_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(9876);
            let rx = crate::control_server::start(port, ctx.clone());
            app = app.with_control(rx);
        }

        Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            painter,
            touch: TouchLayer::new(),
            keys: Vec::new(),
            modifiers: Modifiers::NONE,
            start: Instant::now(),
            size_px: [size_px[0].clamp(1, MAX_SIDE), size_px[1].clamp(1, MAX_SIDE)],
            scale: if scale.is_finite() && scale > 0.0 { scale } else { 2.0 },
            focused: true,
            commands: tx,
            governor: Governor::new(ram_bytes),
            applied: Applied::default(),
            keyboard_visible: false,
            last_status: String::new(),
            last_cursor: "default",
            paths,
        }
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn to_points(&self, x: f32, y: f32) -> Pos2 {
        // the Swift side reports points already; kept as one place to convert if that changes
        pos2(x, y)
    }

    // ------------------------------------------------------------------------------ input

    pub fn resize(&mut self, size_px: [u32; 2], scale: f32) {
        self.size_px = [size_px[0].clamp(1, MAX_SIDE), size_px[1].clamp(1, MAX_SIDE)];
        if scale.is_finite() && scale > 0.0 {
            self.scale = scale;
        }
        if let Some(p) = self.painter.as_mut() {
            p.resize(self.size_px);
        }
    }

    pub fn touch(&mut self, id: u64, phase: i32, x: f32, y: f32, kind: i32, secondary: bool) {
        let t = Touch { id, phase: Phase::from_raw(phase), pos: self.to_points(x, y), kind: Kind::from_raw(kind), secondary, time: self.now() };
        self.touch.touch(t);
    }

    pub fn hover(&mut self, x: f32, y: f32, inside: bool) {
        let p = self.to_points(x, y);
        self.touch.hover(inside.then_some(p));
    }

    pub fn scroll(&mut self, dx: f32, dy: f32, phase: i32) {
        let phase = match phase {
            0 => egui::TouchPhase::Start,
            2 => egui::TouchPhase::End,
            3 => egui::TouchPhase::Cancel,
            _ => egui::TouchPhase::Move,
        };
        self.touch.scroll(vec2(dx, dy), phase);
    }

    pub fn pinch(&mut self, factor: f32, x: f32, y: f32) {
        let p = self.to_points(x, y);
        self.touch.pinch(factor, Some(p));
    }

    fn set_modifiers(&mut self, m: Modifiers) {
        if m != self.modifiers {
            self.modifiers = m;
            self.touch.set_modifiers(m);
            self.keys.push(Event::ModifiersChanged(m));
        }
    }

    /// A hardware key (HID usage), with the characters it typed and those it types unmodified.
    pub fn key(&mut self, usage: u32, pressed: bool, repeat: bool, flags: u64, chars: &str, unmodified: &str) {
        let m = keys::modifiers(flags);
        self.set_modifiers(m);
        if keys::is_modifier(usage) {
            return;
        }
        let Some(key) = keys::logical(usage, unmodified) else { return };
        // the clipboard shortcuts arrive as egui's clipboard events, as on the desktop (FilmCraft
        // turns them back into its Copy / Cut / Paste shortcuts outside text fields); Paste comes with
        // the pasteboard's text through `paste`
        if m.command && !m.shift && !m.alt {
            match key {
                egui::Key::C if pressed => return self.keys.push(Event::Copy),
                egui::Key::X if pressed => return self.keys.push(Event::Cut),
                egui::Key::C | egui::Key::X | egui::Key::V => return,
                _ => {}
            }
        }
        self.keys.push(Event::Key { key, physical_key: keys::physical(usage), pressed, repeat, modifiers: m });
        if pressed && let Some(text) = keys::typed_text(chars, m) {
            self.keys.push(Event::Text(text));
        }
    }

    /// Text from the on-screen keyboard (or dictation).
    pub fn text(&mut self, text: &str) {
        let m = self.modifiers;
        let mut run = String::new();
        for c in text.chars() {
            if c == '\n' || c == '\r' {
                if !run.is_empty() {
                    self.keys.push(Event::Text(std::mem::take(&mut run)));
                }
                for pressed in [true, false] {
                    self.keys.push(Event::Key { key: egui::Key::Enter, physical_key: Some(egui::Key::Enter), pressed, repeat: false, modifiers: m });
                }
            } else if !c.is_control() {
                run.push(c);
            }
        }
        if !run.is_empty() {
            self.keys.push(Event::Text(run));
        }
    }

    /// The on-screen keyboard's delete key.
    pub fn backspace(&mut self) {
        for pressed in [true, false] {
            self.keys.push(Event::Key {
                key: egui::Key::Backspace,
                physical_key: Some(egui::Key::Backspace),
                pressed,
                repeat: false,
                modifiers: self.modifiers,
            });
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.keys.push(Event::Paste(text.to_string()));
    }

    /// A toolbar button: a FilmCraft command id, or one of the host's own.
    pub fn command(&mut self, id: &str) {
        match id {
            SHARE_LAST_EXPORT => match newest_file(&self.paths.documents.join("Exports")) {
                Some(p) => bridge::share_file(&p.to_string_lossy()),
                None => self.app.ui.status = "Aucun export pour l'instant : Fichier ▸ Exporter ▸ Média…".into(),
            },
            _ => {
                let _ = self.commands.send(id.to_string());
            }
        }
    }

    /// Files handed to FilmCraft from outside (Open in…, the share sheet, drag and drop): a project
    /// opens, media is imported.
    pub fn open_paths(&mut self, paths: Vec<String>) {
        let (projects, media): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| p.to_ascii_lowercase().ends_with(".fcproj"));
        if let Some(p) = projects.first()
            && let Err(e) = self.app.session.execute("file.open", json!({"path": p}))
        {
            self.app.ui.status = e.to_string();
        }
        if !media.is_empty()
            && let Err(e) = self.app.session.execute("file.import", json!({"paths": media}))
        {
            self.app.ui.status = e.to_string();
        }
    }

    // ---------------------------------------------------------------------------- lifecycle

    pub fn memory_warning(&mut self) {
        let now = self.now();
        log::warn!("memory warning from iPadOS: shrinking the caches");
        self.governor.memory_warning(now);
        filmcraft_frame::pool::trim_f32();
        let plan = self.governor.plan(self.sequence_size(), now);
        self.app.frames.set_cache_budget(plan.frame_cache_mb as usize * (1 << 20));
        self.applied.cache = None;
    }

    pub fn thermal(&mut self, raw: i32) {
        let t = Thermal::from_raw(raw);
        if t != self.governor.thermal() {
            log::info!("thermal state {t:?}");
            if t >= Thermal::Serious {
                self.app.ui.status = "iPad chaud : qualité de lecture réduite pour le laisser refroidir".into();
            }
            self.governor.set_thermal(t);
        }
    }

    pub fn low_power(&mut self, on: bool) {
        self.governor.set_low_power(on);
    }

    /// The app is leaving the screen: stop playing, and write the recovery journal now (iPadOS may
    /// end the app at any time from here on).
    pub fn background(&mut self) {
        self.focused = false;
        if self.app.playback.playing {
            self.app.stop();
        }
        self.touch.reset();
        self.app.session.sync_persistence();
        if let Some(p) = self.app.session.persistence.as_ref() {
            p.flush();
        }
    }

    pub fn foreground(&mut self) {
        self.focused = true;
        bridge::request_redraw(0.0);
    }

    /// The app is quitting (rare on iPadOS): what eframe does on exit.
    pub fn shutdown(&mut self) {
        self.app.on_exit();
    }

    // -------------------------------------------------------------------------------- frame

    fn sequence_size(&self) -> Option<(u32, u32)> {
        self.app.session.active_sequence().map(|s| (s.settings.width, s.settings.height))
    }

    /// Apply the device plan where it changed (and the user has not changed it since).
    fn govern(&mut self, now: f64) {
        let plan = self.governor.plan(self.sequence_size(), now);
        let user_cache = self.app.session.prefs.memory.frame_cache_mb;
        let cache_key = (plan.frame_cache_mb, user_cache);
        if self.applied.cache != Some(cache_key) {
            self.app.frames.set_cache_budget(user_cache.min(plan.frame_cache_mb).max(16) as usize * (1 << 20));
            self.applied.cache = Some(cache_key);
        }
        let want = (plan.playback, plan.paused);
        if self.applied.res != Some(want) {
            for view in [&mut self.app.ui.program, &mut self.app.ui.source] {
                // only what the host set before (or the defaults at launch): a choice made in the
                // monitor's menu stays
                let untouched = self.applied.res.is_none_or(|(p, q)| view.res == p && view.paused_res == q);
                if untouched {
                    view.res = want.0;
                    view.paused_res = want.1;
                }
            }
            self.applied.res = Some(want);
        }
        if self.applied.plan.map(|p| p.max_fps) != Some(plan.max_fps) {
            bridge::status(&json!({"maxFps": plan.max_fps}));
        }
        self.applied.plan = Some(plan);
    }

    fn raw_input(&mut self, now: f64) -> egui::RawInput {
        let size = vec2(self.size_px[0] as f32 / self.scale, self.size_px[1] as f32 / self.scale);
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let mut raw = egui::RawInput { screen_rect: Some(rect), time: Some(now), focused: self.focused, ..Default::default() };
        raw.max_texture_side = Some(self.painter.as_ref().map_or(8192, Painter::max_texture_side));
        let vp = raw.viewports.entry(ViewportId::ROOT).or_default();
        vp.native_pixels_per_point = Some(self.scale);
        vp.inner_rect = Some(rect);
        vp.outer_rect = Some(rect);
        vp.monitor_size = Some(size);
        vp.focused = Some(self.focused);
        vp.fullscreen = Some(true);
        raw.events = self.touch.take_events();
        raw.events.append(&mut self.keys);
        raw
    }

    /// Run one frame and draw it. Returns the seconds until the next frame is needed (infinite:
    /// only when something happens).
    pub fn frame(&mut self) -> f64 {
        let now = self.now();
        self.touch.tick(now);
        self.govern(now);
        let mut raw = self.raw_input(now);
        self.app.raw_input_hook(&self.ctx, &mut raw);
        let (app, frame) = (&mut self.app, &mut self.frame);
        let full = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx(), frame);
            app.ui(ui, frame);
        });
        self.platform_output(&full.platform_output);
        let ppp = full.pixels_per_point;
        let clear = self.ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        let primitives = self.ctx.tessellate(full.shapes, ppp);
        let mut textures = full.textures_delta;
        if let Some(p) = self.painter.as_mut() {
            p.paint(ppp, clear, &primitives, &mut textures);
        }
        // without a GPU (tests) nothing uploads them; egui asserts that no delta is dropped unseen
        textures.clear();
        self.publish_status();
        let delay = full.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        let mut next = if delay == Duration::MAX { f64::INFINITY } else { delay.as_secs_f64() };
        if self.touch.active() || !self.keys.is_empty() {
            next = 0.0;
        }
        if let Some(d) = self.touch.deadline() {
            next = next.min((d - now).max(0.0));
        }
        next
    }

    fn platform_output(&mut self, out: &egui::PlatformOutput) {
        for c in &out.commands {
            match c {
                egui::OutputCommand::CopyText(t) => bridge::set_clipboard(t),
                egui::OutputCommand::OpenUrl(u) => bridge::open_url(&u.url),
                egui::OutputCommand::CopyImage(_) => {}
            }
        }
        // a text field has the keyboard focus: the on-screen keyboard comes up (iPadOS hides it by
        // itself when a hardware keyboard is attached)
        let wants = out.ime.is_some();
        if wants != self.keyboard_visible {
            self.keyboard_visible = wants;
            bridge::set_keyboard_visible(wants);
        }
        // the iPad pointer takes the shape egui asks for (trim edges, text fields, the hand tool)
        let cursor = pointer_shape(out.cursor_icon);
        if cursor != self.last_cursor {
            self.last_cursor = cursor;
            bridge::status(&json!({ "cursor": cursor }));
        }
    }

    /// What the native toolbar shows, sent when it changes.
    fn publish_status(&mut self) {
        let s = &self.app.session;
        let busy = s.jobs.iter().any(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false));
        let status = json!({
            "playing": self.app.playback.playing,
            "busy": busy,
            "dirty": s.is_dirty(),
            "hot": self.governor.thermal() >= Thermal::Serious,
        });
        let text = status.to_string();
        if text != self.last_status {
            bridge::status(&status);
            self.last_status = text;
        }
    }

    #[cfg(test)]
    pub(crate) fn app(&mut self) -> &mut FilmcraftApp {
        &mut self.app
    }
}

/// Marks that the iPad's first-launch settings were applied (in the data folder).
const IPAD_PROFILE_MARKER: &str = "ipad-profile-v1";

/// Settings for a first launch on an iPad (the user can change all of them in Settings).
fn first_launch_settings(session: &mut Session) {
    let interval = session.prefs.auto_save.interval_minutes.min(3);
    let cache = session.prefs.memory.frame_cache_mb.min(256);
    let values = [
        // decode the frames a 1/2-resolution preview needs faster (never for exports or a paused frame)
        ("playback.draftDecode", json!(true)),
        ("media.hardwareDecoding", json!(true)),
        // proxies attached to clips play instead of 4K originals
        ("media.enableProxies", json!(true)),
        // iPadOS may end a backgrounded app at any time: save often
        ("autoSave.enabled", json!(true)),
        ("autoSave.intervalMinutes", json!(interval)),
        ("autoSave.recoveryJournal", json!(true)),
        ("memory.frameCacheMb", json!(cache)),
    ];
    // one at a time, so a setting this version lacks does not cost the others
    for (key, value) in values {
        if let Err(e) = session.execute("prefs.set", json!({"key": key, "value": value})) {
            log::warn!("first-launch setting {key}: {e}");
        }
    }
}

/// The iPad pointer shape for an egui cursor (the Swift side draws it with UIPointerInteraction).
fn pointer_shape(icon: egui::CursorIcon) -> &'static str {
    use egui::CursorIcon as C;
    match icon {
        C::Text | C::VerticalText => "text",
        C::ResizeHorizontal | C::ResizeColumn | C::ResizeEast | C::ResizeWest => "resizeH",
        C::ResizeVertical | C::ResizeRow | C::ResizeNorth | C::ResizeSouth => "resizeV",
        C::ResizeNeSw | C::ResizeNwSe | C::ResizeNorthEast | C::ResizeNorthWest | C::ResizeSouthEast | C::ResizeSouthWest => "resizeD",
        C::Grab | C::Grabbing | C::Move | C::AllScroll => "move",
        C::Crosshair | C::Cell => "crosshair",
        C::None => "hidden",
        C::NotAllowed | C::NoDrop => "notAllowed",
        _ => "default",
    }
}

/// The most recently modified file in `dir`.
fn newest_file(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        let dir = std::env::temp_dir().join(format!("filmcraft-ios-test-{}", std::process::id()));
        Host::new(None, [2360, 1640], 2.0, Paths { documents: dir }, 4 << 30, vec!["fr-FR".into()])
    }

    #[test]
    fn frames_run_without_a_gpu_and_settle() {
        let mut h = host();
        // paced like the display link: the demo project's first decodes and previews finish, then
        // the host stops asking for frames (the display link sleeps: battery and heat)
        let mut idle = 0;
        for _ in 0..250 {
            std::thread::sleep(Duration::from_millis(20));
            if h.frame().is_infinite() {
                idle += 1;
                if idle >= 5 {
                    return;
                }
            } else {
                idle = 0;
            }
        }
        panic!("the app keeps asking for frames with nothing happening");
    }

    #[test]
    fn the_device_plan_sets_the_monitors_but_keeps_a_users_choice() {
        let mut h = host();
        h.frame();
        assert_eq!(h.app().ui.program.res, PlaybackRes::Half);
        h.app().ui.program.res = PlaybackRes::Full;
        h.thermal(2);
        h.frame();
        assert_eq!(h.app().ui.program.res, PlaybackRes::Full, "the user's pick stays");
        assert_eq!(h.app().ui.source.res, PlaybackRes::Quarter, "a hot iPad renders less");
    }

    #[test]
    fn taps_keys_and_text_reach_the_app_without_crashing() {
        let mut h = host();
        h.frame();
        // a tap in the middle, a drag across the timeline area, a long press, a pinch and a scroll
        h.touch(1, 0, 590.0, 410.0, 0, false);
        h.touch(1, 2, 590.0, 410.0, 0, false);
        h.frame();
        h.touch(2, 0, 300.0, 700.0, 0, false);
        for i in 1..10 {
            h.touch(2, 1, 300.0 + i as f32 * 20.0, 700.0, 0, false);
            h.frame();
        }
        h.touch(2, 2, 480.0, 700.0, 0, false);
        h.frame();
        h.touch(3, 0, 200.0, 600.0, 0, false);
        h.touch(4, 0, 400.0, 600.0, 0, false);
        h.touch(3, 1, 150.0, 600.0, 0, false);
        h.touch(4, 1, 450.0, 600.0, 0, false);
        h.frame();
        h.touch(3, 2, 150.0, 600.0, 0, false);
        h.touch(4, 2, 450.0, 600.0, 0, false);
        h.scroll(0.0, -40.0, 1);
        h.pinch(1.2, 600.0, 650.0);
        // Space plays, Command-Z undoes, typing with no field focused does nothing harmful
        h.key(44, true, false, 0, " ", " ");
        h.key(44, false, false, 0, " ", " ");
        h.frame();
        h.key(29, true, false, keys::MOD_COMMAND, "z", "z");
        h.key(29, false, false, keys::MOD_COMMAND, "z", "z");
        h.text("Bonjour\n");
        h.backspace();
        h.paste("collé");
        for _ in 0..5 {
            h.frame();
        }
        h.command("playback.toggle");
        h.command(SHARE_LAST_EXPORT);
        h.frame();
        h.memory_warning();
        h.background();
        h.foreground();
        h.frame();
        h.shutdown();
    }

    #[test]
    fn egui_cursors_become_ipad_pointer_shapes() {
        assert_eq!(pointer_shape(egui::CursorIcon::ResizeHorizontal), "resizeH");
        assert_eq!(pointer_shape(egui::CursorIcon::ResizeWest), "resizeH");
        assert_eq!(pointer_shape(egui::CursorIcon::Text), "text");
        assert_eq!(pointer_shape(egui::CursorIcon::Grabbing), "move");
        assert_eq!(pointer_shape(egui::CursorIcon::None), "hidden");
        assert_eq!(pointer_shape(egui::CursorIcon::PointingHand), "default");
    }

    #[test]
    fn hostile_sizes_and_scales_are_clamped() {
        let mut h = host();
        h.resize([0, 0], f32::NAN);
        h.frame();
        h.resize([u32::MAX, 1], -1.0);
        h.touch(1, 99, f32::INFINITY, 0.0, 99, false);
        h.frame();
    }
}
