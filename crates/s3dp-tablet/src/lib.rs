//! Pen tablet input that winit 0.30 drops: pressure, tilt, eraser end and pen buttons.
//!
//! **Wayland** ([`wayland::start`]): compositors give a window no pen data through `wl_pointer`;
//! the `zwp_tablet_v2` protocol has to be bound on winit's own `wl_display` (tablet events name
//! winit's `wl_surface`). A second `wayland-client` connection is wrapped around that foreign
//! display (the only `unsafe` in the crate) and read on a background thread with its own event
//! queue. Once a client binds the tablet seat, compositors stop emulating the pointer for the pen,
//! so the app re-injects pen motion / tip / buttons into the UI from the [`PenEvent`]s.
//!
//! The protocol glue only forwards raw values to [`ToolState`], a pure per-frame accumulator
//! that is unit tested without a compositor.
//!
//! Approach after PhotoCraft's `photocraft-tablet` crate (MIT OR Apache-2.0), which documents the
//! Wayland route but uses XInput2 under Xwayland instead.

#[cfg(target_os = "linux")]
pub mod wayland;

/// Linux input button codes of the pen barrel buttons.
pub const BTN_STYLUS: u32 = 0x14b;
pub const BTN_STYLUS2: u32 = 0x14c;
pub const BTN_STYLUS3: u32 = 0x149;

/// A pen barrel button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenButton {
    /// Lower barrel button.
    First,
    /// Upper barrel button.
    Second,
    Other(u32),
}

impl PenButton {
    pub fn from_code(code: u32) -> Self {
        match code {
            BTN_STYLUS => PenButton::First,
            BTN_STYLUS2 | BTN_STYLUS3 => PenButton::Second,
            c => PenButton::Other(c),
        }
    }
}

/// What happened to the pen in one protocol frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenEventKind {
    /// The pen came into proximity of the window.
    Enter,
    /// Moved (or pressure / tilt changed).
    Motion,
    /// Tip (or eraser) touched the surface.
    Down,
    /// Tip lifted.
    Up,
    Button { button: PenButton, pressed: bool },
    /// The pen left the window or proximity.
    Leave,
}

/// One pen event with the tool state after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenEvent {
    pub kind: PenEventKind,
    /// Surface-local position in logical pixels.
    pub pos: [f32; 2],
    /// 0..1 (0 while hovering).
    pub pressure: f32,
    /// Degrees, -90..90.
    pub tilt: [f32; 2],
    /// The eraser end is in use.
    pub eraser: bool,
}

/// Per-tool accumulator: protocol events update the pending state, `frame` emits the events
/// in a sensible order (enter, motion, down, buttons, up, leave).
#[derive(Clone, Debug, Default)]
pub struct ToolState {
    pub eraser: bool,
    pos: [f32; 2],
    pressure: f32,
    tilt: [f32; 2],
    in_window: bool,
    down: bool,
    // Pending for the current frame.
    enter: bool,
    leave: bool,
    moved: bool,
    went_down: bool,
    went_up: bool,
    buttons: Vec<(PenButton, bool)>,
}

impl ToolState {
    pub fn proximity_in(&mut self) {
        self.enter = true;
        self.leave = false;
    }

    pub fn proximity_out(&mut self) {
        self.leave = true;
    }

    pub fn motion(&mut self, x: f64, y: f64) {
        let (x, y) = (x as f32, y as f32);
        if x.is_finite() && y.is_finite() {
            self.pos = [x, y];
            self.moved = true;
        }
    }

    /// Raw protocol pressure 0..65535.
    pub fn pressure(&mut self, raw: u32) {
        self.pressure = (raw.min(65535) as f32 / 65535.0).clamp(0.0, 1.0);
        self.moved = true;
    }

    pub fn tilt(&mut self, x: f64, y: f64) {
        let f = |v: f64| if v.is_finite() { v.clamp(-90.0, 90.0) as f32 } else { 0.0 };
        self.tilt = [f(x), f(y)];
        self.moved = true;
    }

    pub fn down(&mut self) {
        self.went_down = true;
    }

    pub fn up(&mut self) {
        self.went_up = true;
    }

    pub fn button(&mut self, code: u32, pressed: bool) {
        self.buttons.push((PenButton::from_code(code), pressed));
    }

    /// End of a protocol frame: the events it produced, oldest first.
    pub fn frame(&mut self) -> Vec<PenEvent> {
        let mut kinds = Vec::new();
        if self.enter && !self.in_window {
            self.in_window = true;
            kinds.push(PenEventKind::Enter);
        }
        self.enter = false;
        if self.in_window {
            if self.moved {
                kinds.push(PenEventKind::Motion);
            }
            if self.went_down && !self.down {
                self.down = true;
                kinds.push(PenEventKind::Down);
            }
            kinds.extend(self.buttons.drain(..).map(|(button, pressed)| PenEventKind::Button { button, pressed }));
            // Leaving while touching lifts the tip first.
            if (self.went_up || self.leave) && self.down {
                self.down = false;
                kinds.push(PenEventKind::Up);
            }
            if self.leave {
                self.in_window = false;
                kinds.push(PenEventKind::Leave);
            }
        }
        self.buttons.clear();
        self.moved = false;
        self.went_down = false;
        self.went_up = false;
        self.leave = false;
        if !self.down {
            // Hovering: no pressure.
            self.pressure = 0.0;
        }
        kinds.into_iter().map(|kind| self.event(kind)).collect()
    }

    fn event(&self, kind: PenEventKind) -> PenEvent {
        // The tip reports pressure until `up`; a Down with no pressure event yet reads as full.
        let pressure = match kind {
            PenEventKind::Down if self.pressure <= 0.0 => 1.0,
            _ => self.pressure,
        };
        PenEvent { kind, pos: self.pos, pressure, tilt: self.tilt, eraser: self.eraser }
    }
}

/// Why tablet input could not be started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported(String),
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(why) => write!(f, "tablet input unsupported: {why}"),
            Error::Platform(why) => write!(f, "tablet input failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(ev: &[PenEvent]) -> Vec<PenEventKind> {
        ev.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn stroke_sequence() {
        let mut t = ToolState::default();
        t.proximity_in();
        t.motion(10.0, 20.0);
        assert_eq!(kinds(&t.frame()), [PenEventKind::Enter, PenEventKind::Motion]);
        t.down();
        t.pressure(32768);
        t.motion(11.0, 20.0);
        let ev = t.frame();
        assert_eq!(kinds(&ev), [PenEventKind::Motion, PenEventKind::Down]);
        assert!((ev[1].pressure - 0.5).abs() < 0.01 && ev[1].pos == [11.0, 20.0]);
        // Pressure persists between frames that do not change it.
        t.motion(12.0, 21.0);
        assert!((t.frame()[0].pressure - 0.5).abs() < 0.01);
        t.up();
        t.pressure(0);
        assert_eq!(kinds(&t.frame()), [PenEventKind::Motion, PenEventKind::Up]);
        t.motion(13.0, 21.0);
        assert_eq!(t.frame()[0].pressure, 0.0, "hovering has no pressure");
        t.proximity_out();
        assert_eq!(kinds(&t.frame()), [PenEventKind::Leave]);
        assert!(t.frame().is_empty());
    }

    #[test]
    fn leaving_while_down_lifts_first_and_buttons_map() {
        let mut t = ToolState::default();
        t.proximity_in();
        t.down();
        t.button(BTN_STYLUS, true);
        let ev = t.frame();
        assert_eq!(kinds(&ev), [PenEventKind::Enter, PenEventKind::Down, PenEventKind::Button { button: PenButton::First, pressed: true }]);
        assert_eq!(ev[1].pressure, 1.0, "down with no pressure axis reads full");
        t.proximity_out();
        assert_eq!(kinds(&t.frame()), [PenEventKind::Up, PenEventKind::Leave]);
        // Events outside the window are dropped.
        t.motion(1.0, 1.0);
        t.down();
        assert!(t.frame().is_empty());
        assert_eq!(PenButton::from_code(BTN_STYLUS2), PenButton::Second);
    }

    #[test]
    fn hostile_values() {
        let mut t = ToolState::default();
        t.proximity_in();
        t.motion(f64::NAN, 3.0);
        t.tilt(f64::INFINITY, -1000.0);
        t.pressure(u32::MAX);
        t.down();
        let ev = t.frame();
        let d = ev.iter().find(|e| e.kind == PenEventKind::Down).unwrap();
        assert_eq!(d.pos, [0.0, 0.0]);
        assert_eq!(d.tilt, [0.0, -90.0]);
        assert_eq!(d.pressure, 1.0);
    }
}
