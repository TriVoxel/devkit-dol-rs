//! Human Interface Device helpers.
//!
//! Higher-level wrappers over the raw `/dev/hid/pN/std` character devices
//! that `gc-vfs` exposes. Import this module for frame-by-frame edge
//! detection without touching `gc-hal` directly.
//!
//! # Example
//!
//! ```rust,no_run
//! use gc_vfs::hid::{Pad, Port};
//! use gc_vfs::Buttons;
//!
//! unsafe {
//!     gc_vfs::init();
//!
//!     let mut pad = Pad::open(Port::P1);
//!
//!     loop {
//!         pad.poll();           // call once per frame
//!         gc_vfs::poll();
//!
//!         if pad.pressed(Buttons::A) {
//!             // fires exactly once per press
//!         }
//!         if pad.held(Buttons::R) && pad.pressed(Buttons::Z) {
//!             // chord: hold R, tap Z
//!         }
//!     }
//! }
//! ```

use crate::{vfs_open_hid, ControllerState, Fd, Buttons};

// ─── Port ─────────────────────────────────────────────────────────────────────

/// Which physical controller port to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port { P1 = 0, P2 = 1, P3 = 2, P4 = 3 }

impl Port {
    fn index(self) -> u8 { self as u8 }
    fn std_path(self) -> &'static str {
        match self {
            Port::P1 => "/dev/hid/p1/std",
            Port::P2 => "/dev/hid/p2/std",
            Port::P3 => "/dev/hid/p3/std",
            Port::P4 => "/dev/hid/p4/std",
        }
    }
}

// ─── Pad ──────────────────────────────────────────────────────────────────────

/// Polled state of one GameCube controller port with frame-edge detection.
///
/// Open one per port at startup with [`Pad::open`], then call [`Pad::poll`]
/// once per frame before checking any buttons.
pub struct Pad {
    fd:        Fd,
    pub state: ControllerState,
    prev_btns: u16,
}

impl Pad {
    /// Open the standard controller device for `port`.
    ///
    /// Always succeeds — the `/dev/hid/pN/std` node exists even when nothing
    /// is plugged in. [`ControllerState::connected`] will be `0` in that case.
    pub unsafe fn open(port: Port) -> Self {
        // vfs_open_hid is a crate-internal shortcut that opens a HID node
        // without going through the full path-split machinery.
        let fd = vfs_open_hid(port.index()).unwrap_or(Fd::MAX);
        Pad { fd, state: ControllerState::default(), prev_btns: 0 }
    }

    /// Read the current hardware state. Call exactly once per frame, before
    /// any calls to [`held`](Self::held), [`pressed`](Self::pressed), or
    /// [`released`](Self::released).
    pub unsafe fn poll(&mut self) {
        self.prev_btns = self.state.buttons;
        let mut s = ControllerState::default();
        let bytes = core::slice::from_raw_parts_mut(
            &mut s as *mut _ as *mut u8,
            core::mem::size_of::<ControllerState>(),
        );
        if self.fd != Fd::MAX {
            crate::do_read_fd(self.fd, bytes).ok();
        }
        self.state = s;
        if self.state.connected == 0 {
            self.state.buttons = 0;
        }
    }

    // ── Button queries ────────────────────────────────────────────────────

    /// `true` every frame the button is physically held down.
    #[inline]
    pub fn held(&self, button: u16) -> bool {
        self.state.buttons & button != 0
    }

    /// `true` only on the **first** frame the button transitions from
    /// released → pressed. Use this to fire one-shot actions.
    #[inline]
    pub fn pressed(&self, button: u16) -> bool {
        self.state.buttons & !self.prev_btns & button != 0
    }

    /// `true` only on the frame the button transitions from pressed → released.
    #[inline]
    pub fn released(&self, button: u16) -> bool {
        !self.state.buttons & self.prev_btns & button != 0
    }

    // ── Analog helpers ────────────────────────────────────────────────────

    /// Main stick X axis, remapped to `[-1.0, 1.0]`.
    #[inline]
    pub fn stick_x(&self) -> f32 { axis_f32(self.state.stick_x) }

    /// Main stick Y axis, remapped to `[-1.0, 1.0]`.
    #[inline]
    pub fn stick_y(&self) -> f32 { axis_f32(self.state.stick_y) }

    /// C-stick X axis, remapped to `[-1.0, 1.0]`.
    #[inline]
    pub fn cstick_x(&self) -> f32 { axis_f32(self.state.cstick_x) }

    /// C-stick Y axis, remapped to `[-1.0, 1.0]`.
    #[inline]
    pub fn cstick_y(&self) -> f32 { axis_f32(self.state.cstick_y) }

    /// Left analog trigger, remapped to `[0.0, 1.0]`.
    #[inline]
    pub fn trigger_l(&self) -> f32 { self.state.trigger_l as f32 / 255.0 }

    /// Right analog trigger, remapped to `[0.0, 1.0]`.
    #[inline]
    pub fn trigger_r(&self) -> f32 { self.state.trigger_r as f32 / 255.0 }

    /// `true` if a controller is plugged into this port.
    #[inline]
    pub fn connected(&self) -> bool { self.state.connected != 0 }

    // ── Deadzone helpers ──────────────────────────────────────────────────

    /// Main stick vector with a circular deadzone applied.
    /// Returns `(0.0, 0.0)` when the stick is within `deadzone` of center.
    pub fn stick_with_deadzone(&self, deadzone: f32) -> (f32, f32) {
        apply_deadzone(self.stick_x(), self.stick_y(), deadzone)
    }

    /// C-stick vector with a circular deadzone applied.
    pub fn cstick_with_deadzone(&self, deadzone: f32) -> (f32, f32) {
        apply_deadzone(self.cstick_x(), self.cstick_y(), deadzone)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Remap a raw 0–255 axis byte to [-1.0, 1.0] with center ≈ 128.
fn axis_f32(raw: u8) -> f32 {
    let v = raw as f32 - 128.0;
    // Scale so that the full ±128 range maps to exactly ±1.0
    (v / 128.0).clamp(-1.0, 1.0)
}

fn apply_deadzone(x: f32, y: f32, dz: f32) -> (f32, f32) {
    let mag = libm_sqrtf(x * x + y * y);
    if mag < dz { return (0.0, 0.0); }
    // Rescale so that the output reaches 1.0 at full deflection
    let scale = (mag - dz) / (1.0 - dz) / mag;
    (x * scale, y * scale)
}

/// Minimal `sqrtf` without pulling in a full libm.
fn libm_sqrtf(x: f32) -> f32 {
    // Use the PowerPC `frsqrte` estimate + one Newton-Raphson step.
    // Accurate to ~22 bits — sufficient for deadzone computation.
    if x <= 0.0 { return 0.0; }
    // SAFETY: PowerPC `frsqrte` is a single instruction, always valid.
    let est: f32;
    unsafe { core::arch::asm!("frsqrte {0}, {1}", out(freg) est, in(freg) x as f64) };
    // Newton-Raphson: y_{n+1} = y_n * (1.5 - 0.5 * x * y_n^2)
    let y = est * (1.5 - 0.5 * x * est * est);
    x * y
}
