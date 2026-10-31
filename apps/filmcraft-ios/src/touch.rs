//! Touch, Apple Pencil and trackpad input turned into the pointer FilmCraft's panels expect.
//!
//! The panels are written for a mouse: a press, moves, a release, a right click for the context
//! menus, a wheel and a pinch. UIKit gives raw touches, so this layer decides what each one means
//! before egui sees anything:
//!
//! - **One finger** is held back until it means something: lifted quickly, it is a click (pressed
//!   and released where it went down, so egui's click distance holds); moved past the slop, it is a
//!   press where it went down followed by the moves (a drag: trims, clip moves, sliders); held still
//!   for [`LONG_PRESS`], it is a right click (the context menus).
//! - **Two fingers** that land before the first one moved are a gesture, never a press: their
//!   centre moving is a scroll (the wheel), their spread changing is a pinch (the zoom the Timeline
//!   and the monitors read). The first clear movement locks the gesture to one of the two, so a
//!   scroll does not zoom and a pinch does not scroll.
//! - **Apple Pencil** behaves like one finger with a much smaller slop: its tip is precise.
//! - **A trackpad or mouse** pointer (UIKit's indirect touches) is a mouse: pressed and released at
//!   once, with hover between clicks (reported separately by the host) and its own right button.
//!
//! The layer is pure: it takes timed touches and returns egui events, so it is tested here on
//! every system.

use egui::{Event, Modifiers, PointerButton, Pos2, TouchPhase, Vec2, pos2};

/// Hold this long without moving and a finger is a right click (iPadOS context menus use ~0.5 s).
pub const LONG_PRESS: f64 = 0.5;
/// How far (points) a finger moves before it is a drag rather than a tap.
pub const FINGER_SLOP: f32 = 10.0;
/// The same for the Pencil tip.
pub const PENCIL_SLOP: f32 = 3.0;
/// How far the centre of two fingers moves, or their spread changes (as a ratio), before a gesture
/// locks to scrolling or zooming.
const PAN_LOCK: f32 = 8.0;
const ZOOM_LOCK: f32 = 0.06;

/// What touched the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Finger,
    Pencil,
    /// A trackpad or mouse pointer (UIKit `indirectPointer`).
    Pointer,
}

impl Kind {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Kind::Pencil,
            2 => Kind::Pointer,
            _ => Kind::Finger,
        }
    }
}

/// A touch phase as the host reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

impl Phase {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Phase::Began,
            2 => Phase::Ended,
            3 => Phase::Cancelled,
            _ => Phase::Moved,
        }
    }
}

/// One touch sample from the host (positions in points, time in seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    pub id: u64,
    pub phase: Phase,
    pub pos: Pos2,
    pub kind: Kind,
    /// The pointer button for [`Kind::Pointer`] (secondary for a right click on a trackpad).
    pub secondary: bool,
    pub time: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum GestureLock {
    Undecided,
    Pan,
    Zoom,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    Idle,
    /// One finger (or the Pencil) down, its meaning not known yet.
    Pending {
        id: u64,
        kind: Kind,
        start: Pos2,
        at: f64,
        last: Pos2,
    },
    /// A press has been sent; moves follow until the touch ends.
    Pressed {
        id: u64,
        button: PointerButton,
        last: Pos2,
        hover_after: bool,
    },
    /// Two fingers: scrolling or zooming.
    Gesture {
        a: u64,
        b: u64,
        pa: Pos2,
        pb: Pos2,
        start_centre: Pos2,
        start_spread: f32,
        lock: GestureLock,
        started: bool,
    },
    /// Nothing more from these touches (a long press already right-clicked, or a gesture lost a
    /// finger): wait until every finger is up.
    Ignoring,
}

/// The touch layer. Feed it touches with [`TouchLayer::touch`], call [`TouchLayer::tick`] every
/// frame, and send what [`TouchLayer::take_events`] returns to egui.
#[derive(Debug)]
pub struct TouchLayer {
    state: State,
    /// Touches down now (id, position).
    down: Vec<(u64, Pos2)>,
    /// Events ready for egui, in order.
    out: Vec<Event>,
    /// Events that must wait for the next frame: egui reads a press and the first move of a drag
    /// best on separate frames (as `FilmcraftApp` does for scripted input).
    next_frame: Vec<Event>,
    /// A finger has left, but egui must see the release and run drop targets before it loses hover.
    gone_next_frame: bool,
    modifiers: Modifiers,
}

impl Default for TouchLayer {
    fn default() -> Self {
        Self::new()
    }
}

fn centre(a: Pos2, b: Pos2) -> Pos2 {
    pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn spread(a: Pos2, b: Pos2) -> f32 {
    (a - b).length().max(1.0)
}

impl TouchLayer {
    pub fn new() -> Self {
        Self { state: State::Idle, down: Vec::new(), out: Vec::new(), next_frame: Vec::new(), gone_next_frame: false, modifiers: Modifiers::NONE }
    }

    /// The keyboard modifiers held (a hardware keyboard): they go with presses and the wheel.
    pub fn set_modifiers(&mut self, m: Modifiers) {
        self.modifiers = m;
    }

    /// Whether anything is down or in progress (the host keeps drawing frames meanwhile).
    pub fn active(&self) -> bool {
        self.state != State::Idle || !self.next_frame.is_empty() || self.gone_next_frame
    }

    /// When a long press would fire, so the host draws a frame then even if nothing moves.
    pub fn deadline(&self) -> Option<f64> {
        match self.state {
            State::Pending { at, .. } => Some(at + LONG_PRESS),
            _ => None,
        }
    }

    fn press(&mut self, pos: Pos2, button: PointerButton) {
        self.out.push(Event::PointerMoved(pos));
        self.out.push(Event::PointerButton { pos, button, pressed: true, modifiers: self.modifiers });
    }

    fn release(&mut self, pos: Pos2, button: PointerButton, gone: bool) {
        self.out.push(Event::PointerButton { pos, button, pressed: false, modifiers: self.modifiers });
        if gone {
            // A tap is a complete click in one frame; there is no drop target that needs the
            // pointer position after it, so it may leave immediately.
            self.out.push(Event::PointerGone);
        }
    }

    fn defer_gone(&mut self) {
        // Keep the release frame's hover position alive: timeline/project drop targets inspect it
        // after processing PointerButton(false). PointerGone is intentionally a separate frame.
        self.gone_next_frame = true;
    }

    fn click(&mut self, pos: Pos2, button: PointerButton) {
        self.press(pos, button);
        self.release(pos, button, true);
    }

    /// A trackpad or mouse hovering (no button down), or leaving the view (`None`).
    pub fn hover(&mut self, pos: Option<Pos2>) {
        if matches!(self.state, State::Idle) {
            match pos {
                Some(p) => self.out.push(Event::PointerMoved(p)),
                None => self.out.push(Event::PointerGone),
            }
        }
    }

    /// A trackpad scroll or mouse wheel (points; positive moves the content right / down).
    pub fn scroll(&mut self, delta: Vec2, phase: TouchPhase) {
        if delta.x.is_finite() && delta.y.is_finite() {
            self.out.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, phase, modifiers: self.modifiers });
        }
    }

    /// A trackpad pinch: `factor` > 1 zooms in. `at` puts the pointer there first (the Timeline
    /// zooms about the pointer).
    pub fn pinch(&mut self, factor: f32, at: Option<Pos2>) {
        if factor.is_finite() && factor > 0.0 {
            if let Some(p) = at {
                self.out.push(Event::PointerMoved(p));
            }
            self.out.push(Event::Zoom(factor));
        }
    }

    /// One touch sample.
    pub fn touch(&mut self, t: Touch) {
        if !(t.pos.x.is_finite() && t.pos.y.is_finite()) {
            return;
        }
        match t.phase {
            Phase::Began => {
                self.down.retain(|(id, _)| *id != t.id);
                self.down.push((t.id, t.pos));
                self.began(t);
            }
            Phase::Moved => {
                if let Some(d) = self.down.iter_mut().find(|(id, _)| *id == t.id) {
                    d.1 = t.pos;
                }
                self.moved(t);
            }
            Phase::Ended | Phase::Cancelled => {
                self.down.retain(|(id, _)| *id != t.id);
                self.ended(t);
            }
        }
    }

    fn began(&mut self, t: Touch) {
        match self.state.clone() {
            State::Idle => match t.kind {
                // a mouse button: pressed now, as on a desktop
                Kind::Pointer => {
                    let button = if t.secondary { PointerButton::Secondary } else { PointerButton::Primary };
                    self.press(t.pos, button);
                    self.state = State::Pressed { id: t.id, button, last: t.pos, hover_after: true };
                }
                kind => self.state = State::Pending { id: t.id, kind, start: t.pos, at: t.time, last: t.pos },
            },
            // a second finger before the first meant anything: a two-finger gesture
            State::Pending { id, kind: Kind::Finger, last, .. } if t.kind == Kind::Finger && id != t.id => {
                let (pa, pb) = (last, t.pos);
                self.state = State::Gesture {
                    a: id,
                    b: t.id,
                    pa,
                    pb,
                    start_centre: centre(pa, pb),
                    start_spread: spread(pa, pb),
                    lock: GestureLock::Undecided,
                    started: false,
                };
            }
            // more fingers during a gesture, a drag or a long press: ignored (the first ones lead)
            _ => {}
        }
    }

    fn moved(&mut self, t: Touch) {
        match self.state.clone() {
            State::Pending { id, kind, start, at, .. } if id == t.id => {
                let slop = if kind == Kind::Pencil { PENCIL_SLOP } else { FINGER_SLOP };
                if (t.pos - start).length() > slop {
                    // a drag: pressed where the finger went down, the move on the next frame
                    self.press(start, PointerButton::Primary);
                    self.next_frame.push(Event::PointerMoved(t.pos));
                    self.state = State::Pressed { id, button: PointerButton::Primary, last: t.pos, hover_after: false };
                } else {
                    self.state = State::Pending { id, kind, start, at, last: t.pos };
                }
            }
            State::Pressed { id, button, hover_after, .. } if id == t.id => {
                if self.next_frame.is_empty() {
                    self.out.push(Event::PointerMoved(t.pos));
                } else {
                    // keep the order: this move after the one already waiting
                    self.next_frame.push(Event::PointerMoved(t.pos));
                }
                self.state = State::Pressed { id, button, last: t.pos, hover_after };
            }
            State::Gesture { a, b, mut pa, mut pb, start_centre, start_spread, mut lock, mut started } if t.id == a || t.id == b => {
                let (old_c, old_s) = (centre(pa, pb), spread(pa, pb));
                if t.id == a {
                    pa = t.pos;
                } else {
                    pb = t.pos;
                }
                let (c, s) = (centre(pa, pb), spread(pa, pb));
                if lock == GestureLock::Undecided {
                    let zoom = (s / start_spread).ln().abs();
                    let pan = (c - start_centre).length();
                    if zoom > ZOOM_LOCK && zoom * 100.0 > pan {
                        lock = GestureLock::Zoom;
                    } else if pan > PAN_LOCK {
                        lock = GestureLock::Pan;
                    }
                }
                // the pointer sits between the fingers: the Timeline scrolls and zooms what is under it
                self.out.push(Event::PointerMoved(c));
                match lock {
                    GestureLock::Pan => {
                        let phase = if started { TouchPhase::Move } else { TouchPhase::Start };
                        started = true;
                        self.out.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: c - old_c, phase, modifiers: self.modifiers });
                    }
                    GestureLock::Zoom => {
                        let f = s / old_s;
                        if f.is_finite() && f > 0.0 && (f - 1.0).abs() > 1e-4 {
                            self.out.push(Event::Zoom(f));
                        }
                    }
                    GestureLock::Undecided => {}
                }
                self.state = State::Gesture { a, b, pa, pb, start_centre, start_spread, lock, started };
            }
            _ => {}
        }
    }

    fn ended(&mut self, t: Touch) {
        match self.state.clone() {
            State::Pending { id, start, .. } if id == t.id => {
                // UIKit cancellation is not a tap. In particular, cancelling a press over a
                // destructive button must never activate it.
                if t.phase == Phase::Ended {
                    self.click(start, PointerButton::Primary);
                }
                self.state = self.after_lift();
            }
            State::Pressed { id, button, last, hover_after } if id == t.id => {
                let pos = if t.phase == Phase::Cancelled { last } else { t.pos };
                if !self.next_frame.is_empty() {
                    // the drag's moves are still waiting: release after them
                    self.next_frame.push(Event::PointerButton { pos, button, pressed: false, modifiers: self.modifiers });
                    if !hover_after {
                        self.defer_gone();
                    }
                } else {
                    self.release(pos, button, false);
                    if !hover_after {
                        self.defer_gone();
                    }
                }
                self.state = self.after_lift();
            }
            State::Gesture { a, b, lock, started, pa, pb, .. } if t.id == a || t.id == b => {
                if lock == GestureLock::Pan && started {
                    self.out.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: Vec2::ZERO,
                        phase: TouchPhase::End,
                        modifiers: self.modifiers,
                    });
                }
                let _ = (pa, pb);
                self.defer_gone();
                self.state = self.after_lift();
            }
            State::Ignoring => self.state = self.after_lift(),
            _ => {}
        }
    }

    /// The state once a leading touch has ended: idle when nothing else is down.
    fn after_lift(&self) -> State {
        if self.down.is_empty() { State::Idle } else { State::Ignoring }
    }

    /// Advance time: fires the long press of a finger held still.
    pub fn tick(&mut self, now: f64) {
        if let State::Pending { kind, start, at, .. } = self.state
            && now - at >= LONG_PRESS
        {
            if kind == Kind::Finger || kind == Kind::Pencil {
                self.press(start, PointerButton::Secondary);
                self.release(start, PointerButton::Secondary, false);
                self.defer_gone();
            }
            self.state = if self.down.is_empty() { State::Idle } else { State::Ignoring };
        }
    }

    /// The events for this frame. Events held for the next frame move up once this batch is out.
    pub fn take_events(&mut self) -> Vec<Event> {
        let now = std::mem::take(&mut self.out);
        if now.is_empty() {
            // nothing new: release the events that waited a frame
            let next = std::mem::take(&mut self.next_frame);
            if !next.is_empty() {
                return next;
            }
            if self.gone_next_frame {
                self.gone_next_frame = false;
                return vec![Event::PointerGone];
            }
        }
        now
    }

    /// Forget everything (the app went to the background: UIKit cancels the touches anyway).
    pub fn reset(&mut self) {
        let was_pressed = matches!(self.state, State::Pressed { .. });
        self.out.clear();
        self.next_frame.clear();
        self.gone_next_frame = false;
        self.down.clear();
        if let State::Pressed { last, button, .. } = self.state {
            self.out.push(Event::PointerButton { pos: last, button, pressed: false, modifiers: self.modifiers });
        }
        if was_pressed || self.state != State::Idle {
            self.out.push(Event::PointerGone);
        }
        self.state = State::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, phase: Phase, x: f32, y: f32, time: f64) -> Touch {
        Touch { id, phase, pos: pos2(x, y), kind: Kind::Finger, secondary: false, time }
    }

    fn buttons(ev: &[Event]) -> Vec<(PointerButton, bool)> {
        ev.iter()
            .filter_map(|e| match e {
                Event::PointerButton { button, pressed, .. } => Some((*button, *pressed)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_tap_is_a_click_where_the_finger_went_down() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 100.0, 100.0, 0.0));
        assert!(l.take_events().is_empty(), "nothing until the finger means something");
        l.touch(t(1, Phase::Moved, 104.0, 102.0, 0.05));
        l.touch(t(1, Phase::Ended, 104.0, 102.0, 0.1));
        let ev = l.take_events();
        assert_eq!(buttons(&ev), vec![(PointerButton::Primary, true), (PointerButton::Primary, false)]);
        assert!(ev.iter().any(|e| matches!(e, Event::PointerButton { pos, .. } if *pos == pos2(100.0, 100.0))));
        assert!(matches!(ev.last(), Some(Event::PointerGone)));
        assert!(!l.active());
    }

    #[test]
    fn a_moved_finger_is_a_drag_from_where_it_went_down() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 100.0, 100.0, 0.0));
        l.touch(t(1, Phase::Moved, 130.0, 100.0, 0.05));
        let first = l.take_events();
        assert_eq!(buttons(&first), vec![(PointerButton::Primary, true)]);
        assert!(matches!(first.first(), Some(Event::PointerMoved(p)) if *p == pos2(100.0, 100.0)));
        // the move comes on the next frame, on its own
        let second = l.take_events();
        assert_eq!(second, vec![Event::PointerMoved(pos2(130.0, 100.0))]);
        l.touch(t(1, Phase::Moved, 160.0, 100.0, 0.1));
        l.touch(t(1, Phase::Ended, 160.0, 100.0, 0.15));
        let end = l.take_events();
        assert_eq!(buttons(&end), vec![(PointerButton::Primary, false)]);
        assert_eq!(l.take_events(), vec![Event::PointerGone]);
        assert!(!l.active());
    }

    #[test]
    fn a_quick_drag_still_releases_after_its_moves() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 0.0, 0.0, 0.0));
        l.touch(t(1, Phase::Moved, 50.0, 0.0, 0.01));
        l.touch(t(1, Phase::Ended, 50.0, 0.0, 0.02));
        let a = l.take_events();
        assert_eq!(buttons(&a), vec![(PointerButton::Primary, true)]);
        let b = l.take_events();
        assert_eq!(buttons(&b), vec![(PointerButton::Primary, false)]);
        assert!(matches!(b.first(), Some(Event::PointerMoved(_))));
        assert_eq!(l.take_events(), vec![Event::PointerGone]);
        assert!(!l.active());
    }

    #[test]
    fn holding_still_is_a_right_click() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 10.0, 20.0, 1.0));
        assert_eq!(l.deadline(), Some(1.0 + LONG_PRESS));
        l.tick(1.2);
        assert!(l.take_events().is_empty());
        l.tick(1.0 + LONG_PRESS);
        let ev = l.take_events();
        assert_eq!(buttons(&ev), vec![(PointerButton::Secondary, true), (PointerButton::Secondary, false)]);
        // lifting afterwards does not click again
        l.touch(t(1, Phase::Ended, 10.0, 20.0, 1.8));
        assert!(buttons(&l.take_events()).is_empty());
        assert!(!l.active());
    }

    #[test]
    fn cancelling_pending_is_silent() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 100.0, 100.0, 0.0));
        l.touch(t(1, Phase::Cancelled, 100.0, 100.0, 0.1));
        assert!(l.take_events().is_empty());
        assert!(!l.active());
    }

    #[test]
    fn cancelling_pressed_releases_last_position_then_goes() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 0.0, 0.0, 0.0));
        l.touch(t(1, Phase::Moved, 30.0, 0.0, 0.1));
        let _ = l.take_events();
        let _ = l.take_events();
        l.touch(t(1, Phase::Cancelled, 40.0, 5.0, 0.2));
        let release = l.take_events();
        assert_eq!(buttons(&release), vec![(PointerButton::Primary, false)]);
        assert!(matches!(release.last(), Some(Event::PointerButton { pos, .. }) if *pos == pos2(30.0, 0.0)));
        assert_eq!(l.take_events(), vec![Event::PointerGone]);
        assert!(!l.active());
    }

    #[test]
    fn two_fingers_scroll_without_pressing() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 100.0, 100.0, 0.0));
        l.touch(t(2, Phase::Began, 200.0, 100.0, 0.02));
        for i in 1..=5 {
            let dy = i as f32 * 6.0;
            l.touch(t(1, Phase::Moved, 100.0, 100.0 + dy, 0.02 + i as f64 * 0.016));
            l.touch(t(2, Phase::Moved, 200.0, 100.0 + dy, 0.02 + i as f64 * 0.016));
        }
        l.touch(t(1, Phase::Ended, 100.0, 130.0, 0.2));
        l.touch(t(2, Phase::Ended, 200.0, 130.0, 0.2));
        let ev = l.take_events();
        assert!(buttons(&ev).is_empty(), "a gesture never presses: {ev:?}");
        let scrolled: f32 = ev
            .iter()
            .filter_map(|e| match e {
                Event::MouseWheel { delta, .. } => Some(delta.y),
                _ => None,
            })
            .sum();
        assert!(scrolled > 20.0, "{scrolled}");
        assert!(!ev.iter().any(|e| matches!(e, Event::Zoom(_))), "a scroll does not zoom");
        assert_eq!(l.take_events(), vec![Event::PointerGone]);
        assert!(!l.active());
    }

    #[test]
    fn spreading_two_fingers_zooms_about_their_centre() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 100.0, 100.0, 0.0));
        l.touch(t(2, Phase::Began, 200.0, 100.0, 0.01));
        l.touch(t(1, Phase::Moved, 80.0, 100.0, 0.05));
        l.touch(t(2, Phase::Moved, 220.0, 100.0, 0.05));
        let ev = l.take_events();
        let zoom: f32 = ev
            .iter()
            .filter_map(|e| match e {
                Event::Zoom(z) => Some(*z),
                _ => None,
            })
            .product();
        assert!(zoom > 1.2, "{zoom}");
        assert!(ev.iter().any(|e| matches!(e, Event::PointerMoved(p) if (p.x - 150.0).abs() < 1.0)));
        assert!(!ev.iter().any(|e| matches!(e, Event::MouseWheel { .. })), "a pinch does not scroll");
    }

    #[test]
    fn a_second_finger_during_a_drag_is_ignored() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 0.0, 0.0, 0.0));
        l.touch(t(1, Phase::Moved, 40.0, 0.0, 0.05));
        let _ = l.take_events();
        let _ = l.take_events();
        l.touch(t(2, Phase::Began, 100.0, 100.0, 0.1));
        l.touch(t(2, Phase::Moved, 150.0, 100.0, 0.12));
        l.touch(t(2, Phase::Ended, 150.0, 100.0, 0.14));
        assert!(buttons(&l.take_events()).is_empty());
        l.touch(t(1, Phase::Ended, 40.0, 0.0, 0.2));
        assert_eq!(buttons(&l.take_events()), vec![(PointerButton::Primary, false)]);
        assert_eq!(l.take_events(), vec![Event::PointerGone]);
        assert!(!l.active());
    }

    #[test]
    fn the_pencil_drags_after_a_tiny_move() {
        let mut l = TouchLayer::new();
        l.touch(Touch { id: 9, phase: Phase::Began, pos: pos2(0.0, 0.0), kind: Kind::Pencil, secondary: false, time: 0.0 });
        l.touch(Touch { id: 9, phase: Phase::Moved, pos: pos2(5.0, 0.0), kind: Kind::Pencil, secondary: false, time: 0.01 });
        assert_eq!(buttons(&l.take_events()), vec![(PointerButton::Primary, true)]);
    }

    #[test]
    fn a_trackpad_click_presses_at_once_and_keeps_hover() {
        let mut l = TouchLayer::new();
        let p = |phase, secondary| Touch { id: 3, phase, pos: pos2(5.0, 5.0), kind: Kind::Pointer, secondary, time: 0.0 };
        l.touch(p(Phase::Began, true));
        assert_eq!(buttons(&l.take_events()), vec![(PointerButton::Secondary, true)]);
        l.touch(p(Phase::Ended, true));
        let ev = l.take_events();
        assert_eq!(buttons(&ev), vec![(PointerButton::Secondary, false)]);
        assert!(!ev.iter().any(|e| matches!(e, Event::PointerGone)), "a mouse pointer stays");
    }

    #[test]
    fn a_cancelled_drag_releases_and_hostile_positions_are_dropped() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 0.0, 0.0, 0.0));
        l.touch(t(1, Phase::Moved, f32::NAN, 0.0, 0.01));
        l.touch(t(1, Phase::Moved, 30.0, 0.0, 0.02));
        let _ = l.take_events();
        l.reset();
        let ev = l.take_events();
        assert_eq!(buttons(&ev), vec![(PointerButton::Primary, false)]);
        assert!(!l.active());
    }

    #[test]
    fn a_long_press_with_a_stray_second_finger_waits_for_both() {
        let mut l = TouchLayer::new();
        l.touch(t(1, Phase::Began, 0.0, 0.0, 0.0));
        l.tick(1.0);
        let _ = l.take_events();
        l.touch(t(2, Phase::Began, 50.0, 50.0, 1.1));
        l.touch(t(1, Phase::Ended, 0.0, 0.0, 1.2));
        assert!(l.active(), "still a finger down");
        l.touch(t(2, Phase::Ended, 50.0, 50.0, 1.3));
        assert!(buttons(&l.take_events()).is_empty());
        assert!(!l.active());
    }
}
