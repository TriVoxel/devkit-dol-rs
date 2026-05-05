//! Human Interface Device helpers.
//!
//! High-level wrappers over the `/dev/hid/pN/` character devices with
//! per-frame edge detection. All types allocate on the stack and are
//! safe to use from a single-threaded bare-metal game loop.
//!
//! # Device types
//!
//! | Device path         | Type        | Notes                              |
//! |---------------------|-------------|------------------------------------|
//! | `/dev/hid/pN/std`   | [`Pad`]     | Standard GC buttons + sticks       |
//! | `/dev/hid/pN/kbd`   | [`Keyboard`]| HID keyboard (PSO or BlueRetro)    |
//! | `/dev/hid/pN/mouse` | [`Mouse`]   | Mouse (GC mouse or BlueRetro)      |
//!
//! # PSO keyboard — automatic dual registration
//!
//! When the driver detects a PSO keyboard on a port it populates **both**
//! `/dev/hid/pN/std` and `/dev/hid/pN/kbd`. Reading `std` returns a
//! synthesised [`Pad`] (WASD → stick, arrows → D-pad, Enter → Start, etc.).
//! Reading `kbd` returns the raw key scan codes. Software designed for a
//! standard controller just works; software that wants keyboard input can
//! open the `kbd` node directly.
//!
//! # Example
//!
//! ```rust,no_run
//! use dkdol_vfs::hid::{Pad, Keyboard, Mouse, Port};
//! use dkdol_vfs::{Buttons, ExtButtons, Key};
//!
//! unsafe {
//!     dkdol_vfs::init();
//!     let mut pad  = Pad::open(Port::P1);
//!     let mut kbd  = Keyboard::open(Port::P1);
//!     let mut mouse= Mouse::open(Port::P1);
//!
//!     loop {
//!         pad.poll();
//!         kbd.poll();
//!         mouse.poll();
//!         dkdol_vfs::poll();
//!
//!         if pad.pressed(Buttons::A) { /* fire */ }
//!         if pad.held_ext(ExtButtons::Home) { /* pause menu */ }
//!         if kbd.pressed(Key::SPACE) { /* jump */ }
//!         let (dx, dy) = mouse.delta();
//!         // aim camera with (dx, dy)
//!     }
//! }
//! ```

use crate::{vfs_open_hid, HidSlot, ControllerState, KbdState, MouseState, WiimoteState, Fd};
pub use crate::{Buttons, ExtButtons, Key, WiiButtons, WiiExtension};

// ─── Port ─────────────────────────────────────────────────────────────────────

/// Physical controller port index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port { P1 = 0, P2 = 1, P3 = 2, P4 = 3 }

impl Port {
    #[inline] pub fn index(self) -> u8 { self as u8 }
}

// ─── Pad ──────────────────────────────────────────────────────────────────────

/// One controller port, polled once per frame with edge detection.
///
/// On a PSO keyboard port this returns a synthesised pad state.
/// On a BlueRetro extended port the `ext_buttons` field carries the
/// 6 extra digital buttons.
pub struct Pad {
    fd:           Fd,
    pub state:    ControllerState,
    prev_btns:    u16,
    prev_ext:     u8,
}

impl Pad {
    /// Open `/dev/hid/pN/std`. Always succeeds; returns zeroed state when
    /// nothing is connected.
    pub unsafe fn open(port: Port) -> Self {
        let fd = vfs_open_hid(port.index(), HidSlot::Std).unwrap_or(Fd::MAX);
        Pad { fd, state: ControllerState::default(), prev_btns: 0, prev_ext: 0 }
    }

    /// Sample hardware state. Call exactly once per frame.
    pub unsafe fn poll(&mut self) {
        self.prev_btns = self.state.buttons;
        self.prev_ext  = self.state.ext_buttons;
        let mut s = ControllerState::default();
        let bytes = core::slice::from_raw_parts_mut(
            &mut s as *mut _ as *mut u8,
            core::mem::size_of::<ControllerState>(),
        );
        if self.fd != Fd::MAX { crate::do_read_fd(self.fd, bytes).ok(); }
        self.state = s;
        if self.state.connected == 0 { self.state.buttons = 0; self.state.ext_buttons = 0; }
    }

    // ── Standard buttons ──────────────────────────────────────────────────

    /// True every frame the button is held.
    #[inline] pub fn held(&self, b: u16) -> bool { self.state.buttons & b != 0 }
    /// True on the first frame the button is pressed.
    #[inline] pub fn pressed(&self, b: u16) -> bool { self.state.buttons & !self.prev_btns & b != 0 }
    /// True on the frame the button is released.
    #[inline] pub fn released(&self, b: u16) -> bool { !self.state.buttons & self.prev_btns & b != 0 }

    // ── Extended buttons (BlueRetro modern controllers) ───────────────────

    /// True every frame the extended button is held.
    #[inline] pub fn held_ext(&self, b: u8) -> bool { self.state.ext_buttons & b != 0 }
    /// True on the first frame the extended button is pressed.
    #[inline] pub fn pressed_ext(&self, b: u8) -> bool { self.state.ext_buttons & !self.prev_ext & b != 0 }
    /// True on the frame the extended button is released.
    #[inline] pub fn released_ext(&self, b: u8) -> bool { !self.state.ext_buttons & self.prev_ext & b != 0 }

    // ── Analog ────────────────────────────────────────────────────────────

    #[inline] pub fn stick_x(&self) -> f32 { axis_f32(self.state.stick_x) }
    #[inline] pub fn stick_y(&self) -> f32 { axis_f32(self.state.stick_y) }
    #[inline] pub fn cstick_x(&self) -> f32 { axis_f32(self.state.cstick_x) }
    #[inline] pub fn cstick_y(&self) -> f32 { axis_f32(self.state.cstick_y) }
    #[inline] pub fn trigger_l(&self) -> f32 { self.state.trigger_l as f32 / 255.0 }
    #[inline] pub fn trigger_r(&self) -> f32 { self.state.trigger_r as f32 / 255.0 }
    #[inline] pub fn connected(&self) -> bool { self.state.connected != 0 }

    pub fn stick_with_deadzone(&self, dz: f32) -> (f32, f32) {
        apply_deadzone(self.stick_x(), self.stick_y(), dz)
    }
    pub fn cstick_with_deadzone(&self, dz: f32) -> (f32, f32) {
        apply_deadzone(self.cstick_x(), self.cstick_y(), dz)
    }
}

// ─── Keyboard ─────────────────────────────────────────────────────────────────

/// Keyboard state with per-frame edge detection.
///
/// Works with the PSO keyboard, a USB keyboard connected via BlueRetro,
/// or any other SI keyboard device. Key codes use HID usage IDs; see [`Key`].
pub struct Keyboard {
    fd:        Fd,
    pub state: KbdState,
    prev_keys: [u8; 6],
    prev_mods: u8,
}

impl Keyboard {
    /// Open `/dev/hid/pN/kbd`. Always succeeds; returns zeroed state when
    /// no keyboard is present.
    pub unsafe fn open(port: Port) -> Self {
        let fd = vfs_open_hid(port.index(), HidSlot::Kbd).unwrap_or(Fd::MAX);
        Keyboard { fd, state: KbdState::default(), prev_keys: [0u8; 6], prev_mods: 0 }
    }

    /// Sample hardware state. Call exactly once per frame.
    pub unsafe fn poll(&mut self) {
        self.prev_keys = self.state.keys;
        self.prev_mods = self.state.modifiers;
        let mut s = KbdState::default();
        let bytes = core::slice::from_raw_parts_mut(
            &mut s as *mut _ as *mut u8,
            core::mem::size_of::<KbdState>(),
        );
        if self.fd != Fd::MAX { crate::do_read_fd(self.fd, bytes).ok(); }
        self.state = s;
    }

    // ── Key queries ───────────────────────────────────────────────────────

    /// True every frame the key is held.
    #[inline]
    pub fn held(&self, key: u8) -> bool {
        self.state.keys.iter().any(|&k| k == key && k != 0)
    }

    /// True on the first frame the key is pressed.
    #[inline]
    pub fn pressed(&self, key: u8) -> bool {
        let now  = self.state.keys.iter().any(|&k| k == key && k != 0);
        let prev = self.prev_keys.iter().any(|&k| k == key && k != 0);
        now && !prev
    }

    /// True on the frame the key is released.
    #[inline]
    pub fn released(&self, key: u8) -> bool {
        let now  = self.state.keys.iter().any(|&k| k == key && k != 0);
        let prev = self.prev_keys.iter().any(|&k| k == key && k != 0);
        !now && prev
    }

    /// True if a modifier key is held.
    #[inline]
    pub fn modifier(&self, mod_bit: u8) -> bool {
        self.state.modifiers & mod_bit != 0
    }

    /// True if Shift (left or right) is held.
    #[inline]
    pub fn shift(&self) -> bool {
        self.modifier(Key::MOD_LSHIFT) || self.modifier(Key::MOD_RSHIFT)
    }

    /// True if Ctrl (left or right) is held.
    #[inline]
    pub fn ctrl(&self) -> bool {
        self.modifier(Key::MOD_LCTRL) || self.modifier(Key::MOD_RCTRL)
    }

    /// True if Alt (left or right) is held.
    #[inline]
    pub fn alt(&self) -> bool {
        self.modifier(Key::MOD_LALT) || self.modifier(Key::MOD_RALT)
    }

    /// True if a keyboard is present on this port.
    #[inline]
    pub fn connected(&self) -> bool { self.state.connected != 0 }
}

// ─── Mouse ────────────────────────────────────────────────────────────────────

/// Mouse state with per-frame edge detection.
///
/// `dx` and `dy` are signed deltas accumulated since the last poll.
/// They reset each frame so you can use them directly for camera movement.
pub struct Mouse {
    fd:        Fd,
    pub state: MouseState,
    prev_btns: u8,
}

impl Mouse {
    /// Open `/dev/hid/pN/mouse`. Always succeeds; returns zeroed state when
    /// no mouse is present.
    pub unsafe fn open(port: Port) -> Self {
        let fd = vfs_open_hid(port.index(), HidSlot::Mouse).unwrap_or(Fd::MAX);
        Mouse { fd, state: MouseState::default(), prev_btns: 0 }
    }

    /// Sample hardware state. Call exactly once per frame.
    pub unsafe fn poll(&mut self) {
        self.prev_btns = self.state.buttons;
        let mut s = MouseState::default();
        let bytes = core::slice::from_raw_parts_mut(
            &mut s as *mut _ as *mut u8,
            core::mem::size_of::<MouseState>(),
        );
        if self.fd != Fd::MAX { crate::do_read_fd(self.fd, bytes).ok(); }
        self.state = s;
    }

    // ── Mode ──────────────────────────────────────────────────────────────

    /// True when this mouse is a WiiMote IR pointer (absolute coordinates).
    #[inline] pub fn is_absolute(&self) -> bool { self.state.absolute != 0 }

    // ── Deltas / coordinates ──────────────────────────────────────────────

    /// Raw pixel-space delta since last poll (relative mode).
    /// In absolute mode (WiiMote IR), returns the cursor position instead.
    #[inline] pub fn delta(&self)    -> (i16, i16) { (self.state.dx, self.state.dy) }
    /// Vertical scroll delta. Positive = scroll up. Zero in absolute mode.
    #[inline] pub fn scroll_y(&self) -> i8 { self.state.scroll_y }
    /// Horizontal scroll delta. Zero in absolute mode.
    #[inline] pub fn scroll_x(&self) -> i8 { self.state.scroll_x }
    /// IR pointer visibility (absolute mode only).
    #[inline] pub fn ir_visible(&self) -> bool { self.state.dx != -1 }

    // ── Buttons ───────────────────────────────────────────────────────────

    #[inline] pub fn left_held(&self)    -> bool { self.state.buttons & 0x01 != 0 }
    #[inline] pub fn right_held(&self)   -> bool { self.state.buttons & 0x02 != 0 }
    #[inline] pub fn middle_held(&self)  -> bool { self.state.buttons & 0x04 != 0 }

    #[inline] pub fn left_pressed(&self) -> bool {
        self.state.buttons & !self.prev_btns & 0x01 != 0
    }
    #[inline] pub fn right_pressed(&self) -> bool {
        self.state.buttons & !self.prev_btns & 0x02 != 0
    }
    #[inline] pub fn middle_pressed(&self) -> bool {
        self.state.buttons & !self.prev_btns & 0x04 != 0
    }

    /// True if a mouse device is present on this port.
    #[inline] pub fn connected(&self) -> bool { self.state.connected != 0 }
}

// ─── Wiimote ──────────────────────────────────────────────────────────────────

/// Full WiiMote state with per-frame edge detection on both WiiMote buttons
/// and the synthesised GC pad buttons.
///
/// `ir_x` / `ir_y` are absolute screen coordinates (0–1023 × 0–767) when the
/// IR pointer is visible, or `0xFFFF` otherwise.
pub struct Wiimote {
    fd:           Fd,
    pub state:    WiimoteState,
    prev_btns:    u16,      // synthesised pad buttons
    prev_wii:     u16,      // raw WiiMote buttons
    prev_ext:     u8,
}

impl Wiimote {
    /// Open `/dev/hid/pN/wii`. Always succeeds; returns zeroed state when
    /// no WiiMote is connected.
    pub unsafe fn open(port: Port) -> Self {
        let fd = vfs_open_hid(port.index(), HidSlot::Wii).unwrap_or(Fd::MAX);
        Wiimote { fd, state: WiimoteState::default(), prev_btns: 0, prev_wii: 0, prev_ext: 0 }
    }

    /// Sample hardware state. Call once per frame.
    pub unsafe fn poll(&mut self) {
        self.prev_btns = self.state.buttons;
        self.prev_wii  = self.state.wii_buttons;
        self.prev_ext  = self.state.ext_buttons;
        let mut s = WiimoteState::default();
        let bytes = core::slice::from_raw_parts_mut(
            &mut s as *mut _ as *mut u8,
            core::mem::size_of::<WiimoteState>(),
        );
        if self.fd != Fd::MAX { crate::do_read_fd(self.fd, bytes).ok(); }
        self.state = s;
        if self.state.connected == 0 {
            self.state.buttons = 0;
            self.state.wii_buttons = 0;
            self.state.ext_buttons = 0;
        }
    }

    // ── Synthesised GC pad buttons ────────────────────────────────────────

    #[inline] pub fn held(&self, b: u16)    -> bool { self.state.buttons & b != 0 }
    #[inline] pub fn pressed(&self, b: u16) -> bool { self.state.buttons & !self.prev_btns & b != 0 }
    #[inline] pub fn released(&self,b: u16) -> bool { !self.state.buttons & self.prev_btns & b != 0 }

    // ── Extended buttons ──────────────────────────────────────────────────

    #[inline] pub fn held_ext(&self, b: u8)    -> bool { self.state.ext_buttons & b != 0 }
    #[inline] pub fn pressed_ext(&self, b: u8) -> bool { self.state.ext_buttons & !self.prev_ext & b != 0 }

    // ── Raw WiiMote buttons ───────────────────────────────────────────────

    #[inline] pub fn wii_held(&self, b: u16)    -> bool { self.state.wii_buttons & b != 0 }
    #[inline] pub fn wii_pressed(&self, b: u16) -> bool { self.state.wii_buttons & !self.prev_wii & b != 0 }
    #[inline] pub fn wii_released(&self,b: u16) -> bool { !self.state.wii_buttons & self.prev_wii & b != 0 }

    // ── Analog (synthesised from extension) ───────────────────────────────

    #[inline] pub fn stick_x(&self) -> f32  { axis_f32(self.state.stick_x) }
    #[inline] pub fn stick_y(&self) -> f32  { axis_f32(self.state.stick_y) }
    #[inline] pub fn cstick_x(&self) -> f32 { axis_f32(self.state.cstick_x) }
    #[inline] pub fn cstick_y(&self) -> f32 { axis_f32(self.state.cstick_y) }
    #[inline] pub fn trigger_l(&self) -> f32 { self.state.trigger_l as f32 / 255.0 }
    #[inline] pub fn trigger_r(&self) -> f32 { self.state.trigger_r as f32 / 255.0 }

    // ── IR pointer ────────────────────────────────────────────────────────

    /// True if the WiiMote is pointing at the sensor bar.
    #[inline] pub fn ir_visible(&self) -> bool { self.state.ir_x != 0xFFFF }

    /// IR pointer position as (x, y) in 0.0–1.0 normalised screen space.
    /// Returns `(0.5, 0.5)` when not visible.
    pub fn ir_normalised(&self) -> (f32, f32) {
        if !self.ir_visible() { return (0.5, 0.5); }
        (self.state.ir_x as f32 / 1023.0,
         self.state.ir_y as f32 / 767.0)
    }

    // ── Accelerometer ─────────────────────────────────────────────────────

    #[inline] pub fn accel_x(&self) -> f32 { axis_f32(self.state.accel_x) }
    #[inline] pub fn accel_y(&self) -> f32 { axis_f32(self.state.accel_y) }
    #[inline] pub fn accel_z(&self) -> f32 { axis_f32(self.state.accel_z) }

    #[inline] pub fn extension(&self) -> u8 { self.state.extension }
    #[inline] pub fn connected(&self) -> bool { self.state.connected != 0 }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn axis_f32(raw: u8) -> f32 {
    ((raw as f32 - 128.0) / 128.0).clamp(-1.0, 1.0)
}

fn apply_deadzone(x: f32, y: f32, dz: f32) -> (f32, f32) {
    let mag = libm_sqrtf(x * x + y * y);
    if mag < dz { return (0.0, 0.0); }
    let scale = (mag - dz) / (1.0 - dz) / mag;
    (x * scale, y * scale)
}

fn libm_sqrtf(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    // Quake-style bit-manipulation rsqrt + two Newton-Raphson steps.
    // ~22 bits accuracy, no inline assembly required.
    let bits = x.to_bits();
    let est = f32::from_bits(0x5F37_59DF - (bits >> 1));
    let y = est * (1.5 - 0.5 * x * est * est);
    let y = y   * (1.5 - 0.5 * x * y   * y);
    x * y
}
