//! `zwp_tablet_v2` on winit's `wl_display`.
//!
//! A `wayland-client` connection is wrapped around the foreign display (libwayland is
//! thread-safe and routes each object's events to the queue that created it), the tablet seat of
//! the first `wl_seat` is bound on a private event queue, and a thread dispatches that queue.
//! The thread polls with a short timeout so it can stop (via [`Tablet`]'s drop) before winit
//! disconnects the display.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_backend::client::{Backend, ObjectId, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
    zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
    zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::ZwpTabletV2,
};

use crate::{Error, PenEvent, ToolState};

/// Running tablet reader. Dropping it stops the thread (call before the display goes away).
pub struct Tablet {
    rx: mpsc::Receiver<PenEvent>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tablet {
    /// Pen events received since the last call, oldest first.
    pub fn drain(&self) -> impl Iterator<Item = PenEvent> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for Tablet {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct State {
    tools: HashMap<ObjectId, ToolState>,
    tx: mpsc::Sender<PenEvent>,
    wake: Box<dyn Fn() + Send>,
}

/// Bind the tablet protocol on `display` and start reading it. `wake` is called (from the reader
/// thread) after new events were queued, to request a UI repaint.
///
/// # Safety
/// `display` must be a valid `wl_display*` that stays connected until the returned [`Tablet`] is
/// dropped.
pub unsafe fn start(display: NonNull<c_void>, wake: impl Fn() + Send + 'static) -> Result<Tablet, Error> {
    // SAFETY: the caller guarantees a live display; the backend does not take ownership (it never
    // disconnects a foreign display).
    let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| Error::Platform(format!("registry: {e}")))?;
    let qh = queue.handle();
    let manager: ZwpTabletManagerV2 = globals.bind(&qh, 1..=1, ()).map_err(|_| Error::Unsupported("the compositor has no zwp_tablet_manager_v2".into()))?;
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| Error::Unsupported("no wl_seat".into()))?;
    manager.get_tablet_seat(&seat, &qh, ());
    conn.flush().map_err(|e| Error::Platform(format!("flush: {e}")))?;

    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let state = State { tools: HashMap::new(), tx, wake: Box::new(wake) };
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("s3dp-tablet".into())
        .spawn(move || run(conn, queue, state, &stop2))
        .map_err(|e| Error::Platform(format!("thread: {e}")))?;
    log::info!("tablet: zwp_tablet_v2 bound on the Wayland display");
    Ok(Tablet { rx, stop, thread: Some(thread) })
}

fn run(conn: Connection, mut queue: EventQueue<State>, mut state: State, stop: &AtomicBool) {
    let timeout = Timespec { tv_sec: 0, tv_nsec: 50_000_000 };
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = queue.dispatch_pending(&mut state) {
            log::warn!("tablet: dispatch failed: {e}");
            return;
        }
        if conn.flush().is_err() {
            return;
        }
        // Another reader (winit) may already have queued our events: dispatch them first.
        let Some(guard) = queue.prepare_read() else { continue };
        let ready = {
            let fd = guard.connection_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::ERR)];
            matches!(poll(&mut fds, Some(&timeout)), Ok(n) if n > 0)
        };
        if !ready {
            drop(guard);
            continue;
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                log::warn!("tablet: read failed: {e}");
                return;
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpTabletManagerV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletManagerV2, _: <ZwpTabletManagerV2 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpTabletSeatV2, ()> for State {
    fn event(state: &mut Self, _: &ZwpTabletSeatV2, event: zwp_tablet_seat_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_tablet_seat_v2::Event::ToolAdded { id } = event {
            state.tools.insert(id.id(), ToolState::default());
        }
    }

    event_created_child!(State, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<ZwpTabletV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletV2, _: <ZwpTabletV2 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(state: &mut Self, tool: &ZwpTabletToolV2, event: zwp_tablet_tool_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_tablet_tool_v2::Event as E;
        let id = tool.id();
        if let E::Removed = event {
            state.tools.remove(&id);
            tool.destroy();
            return;
        }
        let t = state.tools.entry(id).or_default();
        match event {
            E::Type { tool_type: WEnum::Value(ty) } => t.eraser = ty == zwp_tablet_tool_v2::Type::Eraser,
            E::ProximityIn { .. } => t.proximity_in(),
            E::ProximityOut => t.proximity_out(),
            E::Down { .. } => t.down(),
            E::Up => t.up(),
            E::Motion { x, y } => t.motion(x, y),
            E::Pressure { pressure } => t.pressure(pressure),
            E::Tilt { tilt_x, tilt_y } => t.tilt(tilt_x, tilt_y),
            E::Button { button, state: WEnum::Value(s), .. } => t.button(button, s == zwp_tablet_tool_v2::ButtonState::Pressed),
            E::Frame { .. } => {
                let events = t.frame();
                if !events.is_empty() {
                    for e in events {
                        let _ = state.tx.send(e);
                    }
                    (state.wake)();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpTabletPadV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletPadV2, _: zwp_tablet_pad_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}

    event_created_child!(State, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletPadGroupV2, _: zwp_tablet_pad_group_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}

    event_created_child!(State, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadRingV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletPadRingV2, _: <ZwpTabletPadRingV2 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpTabletPadStripV2, ()> for State {
    fn event(_: &mut Self, _: &ZwpTabletPadStripV2, _: <ZwpTabletPadStripV2 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
