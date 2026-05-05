//! Serial Interface (SI) — controller, keyboard, mouse, and Wiimote driver.
//!
//! ## Device types and identification
//!
//! | Identify bytes        | `DeviceKind`        | Notes                        |
//! |-----------------------|---------------------|------------------------------|
//! | `[0x09, 0x00, _]`     | `StandardPad`       | Retail GC controller         |
//! | `[0x09, 0x80 | _, _]` | `ExtendedPad`       | BlueRetro extended mode      |
//! | `[0x08, 0x20, 0x00]`  | `PsoKeyboard`       | Official PSO ASCII keyboard  |
//! | `[0x02, 0x00, 0x00]`  | `Mouse`             | GC mouse                     |
//!
//! ## BlueRetro extended protocol
//!
//! Byte 1 of the identify response has bit 7 set for BlueRetro extended mode.
//! Byte 2 carries capability flags:
//!
//! | Bit | Constant       | Meaning                              |
//! |-----|----------------|--------------------------------------|
//! | 0   | `CAP_KBD`      | Port carries keyboard state (0x54)   |
//! | 1   | `CAP_MOUSE`    | Port carries mouse state (0x52)      |
//! | 2   | `CAP_WIIMOTE`  | Port carries WiiMote state (0x45)    |
//!
//! ## WiiMote extended poll (command 0x45)
//!
//! Returns 24 bytes. Extension attachments are mapped to a standard GC pad
//! by BlueRetro firmware; the GC never needs to know what's physically
//! attached to the WiiMote:
//!
//! | Attachment     | `/std` mapping                              |
//! |----------------|---------------------------------------------|
//! | None           | WiiMote buttons (sideways mode)             |
//! | Nunchuck       | Nunchuck stick → main stick; C/Z → L/Z     |
//! | Classic Ctrl   | Full GC layout (L-stick/R-stick/triggers)  |
//!
//! IR pointer coordinates (0–1023 × 0–767) are reported in the raw WiiMote
//! state. The VFS mouse path converts them to absolute coordinates.
//!
//! ## PSO keyboard
//!
//! The PSO keyboard has real gamepad buttons (D-pad, A/B/X/Y, Start) that
//! respond to the standard `0x40` poll command. No remapping is done by this
//! driver — reading `/dev/hid/pN/std` on a PSO keyboard port simply issues a
//! `0x40` command and returns the physical button state.

#![allow(dead_code)]

pub const SI_BASE: usize = crate::mmio::addr(0x006400);

const REG_SIPOLL:   usize = 12;
const REG_SICOMCSR: usize = 13;
const REG_SISR:     usize = 14;
const REG_TXBUF:    usize = 32;

const SICOMCSR_TCINT:  u32 = 1 << 31;
const SICOMCSR_COMERR: u32 = 1 << 29;
const SICOMCSR_TSTART: u32 = 1 << 0;
const SISR_NORESPONSE: u32 = 0x08;

// ─── SI commands ──────────────────────────────────────────────────────────────

/// Standard GC pad poll → 8 bytes.
const CMD_POLL:     u32 = 0x4003_0000;
/// BlueRetro extended poll → 16 bytes (standard data + ext buttons).
const CMD_EXTENDED: u32 = 0x4103_0000;
/// Keyboard poll (PSO keyboard and BlueRetro keyboard) → 7 bytes.
const CMD_KEYBOARD: u32 = 0x5400_0000;
/// Mouse poll → 8 bytes.
const CMD_MOUSE:    u32 = 0x5200_0000;
/// WiiMote full poll → 24 bytes.
const CMD_WIIMOTE:  u32 = 0x4500_0000;
/// Device identify → 3 bytes.
const CMD_IDENTIFY: u32 = 0x0000_0000;

// ─── Identify flags ───────────────────────────────────────────────────────────

/// Bit in identify byte 1 indicating BlueRetro extended mode.
const EXT_FLAG:    u8 = 0x80;
/// Capability: port also carries keyboard state via 0x54.
const CAP_KBD:     u8 = 0x01;
/// Capability: port also carries mouse/IR state via 0x52.
const CAP_MOUSE:   u8 = 0x02;
/// Capability: port carries WiiMote state via 0x45.
const CAP_WIIMOTE: u8 = 0x04;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Physical controller port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Port { P1 = 0, P2 = 1, P3 = 2, P4 = 3 }

/// What kind of device is connected to a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Standard retail GameCube controller.
    StandardPad,
    /// BlueRetro receiver in extended mode.
    ExtendedPad {
        /// Port carries keyboard state (poll with 0x54).
        has_keyboard: bool,
        /// Port carries mouse/IR state.
        has_mouse:    bool,
        /// Port carries WiiMote state (poll with 0x45).
        has_wiimote:  bool,
    },
    /// Official PSO ASCII keyboard — responds to both 0x40 (pad) and 0x54 (keys).
    PsoKeyboard,
    /// Nintendo GC mouse.
    Mouse,
    /// Port is empty.
    None,
    /// Unknown device.
    Unknown(u8, u8, u8),
}

impl DeviceKind {
    /// True if this device produces standard pad button data.
    pub fn has_pad(self) -> bool {
        matches!(self, Self::StandardPad | Self::ExtendedPad { .. } | Self::PsoKeyboard)
    }
    /// True if this device produces keyboard scan codes.
    pub fn has_keyboard(self) -> bool {
        matches!(self, Self::PsoKeyboard | Self::ExtendedPad { has_keyboard: true, .. })
    }
    /// True if this device produces pointer/mouse data.
    pub fn has_mouse(self) -> bool {
        matches!(self,
            Self::Mouse
            | Self::ExtendedPad { has_mouse: true, .. }
            | Self::ExtendedPad { has_wiimote: true, .. })
    }
    /// True if this device carries WiiMote raw state.
    pub fn has_wiimote(self) -> bool {
        matches!(self, Self::ExtendedPad { has_wiimote: true, .. })
    }
}

// ─── Button constants ─────────────────────────────────────────────────────────

/// Standard GC button bitmask constants.
#[allow(non_upper_case_globals)]
#[allow(non_snake_case)]
pub mod Buttons {
    pub const DLeft:  u16 = 0x0001;
    pub const DRight: u16 = 0x0002;
    pub const DDown:  u16 = 0x0004;
    pub const DUp:    u16 = 0x0008;
    pub const Z:      u16 = 0x0010;
    pub const R:      u16 = 0x0020;
    pub const L:      u16 = 0x0040;
    pub const A:      u16 = 0x0100;
    pub const B:      u16 = 0x0200;
    pub const X:      u16 = 0x0400;
    pub const Y:      u16 = 0x0800;
    pub const Start:  u16 = 0x1000;
}

/// Six extra digital buttons on modern controllers (BlueRetro extended mode).
#[allow(non_upper_case_globals)]
#[allow(non_snake_case)]
pub mod ExtButtons {
    /// Left stick click (LS / L3).
    pub const StickL:  u8 = 0x01;
    /// Right stick click (RS / R3).
    pub const StickR:  u8 = 0x02;
    /// Home / Guide button.
    pub const Home:    u8 = 0x04;
    /// Minus / View / Select button.
    pub const Minus:   u8 = 0x08;
    /// Left Z (ZL / LB / L1).
    pub const ZL:      u8 = 0x10;
    /// Capture / Share / Screenshot button.
    pub const Capture: u8 = 0x20;
}

/// Raw WiiMote button bitmask constants.
#[allow(non_upper_case_globals)]
#[allow(non_snake_case)]
pub mod WiiButtons {
    pub const DLeft:  u16 = 0x0001;
    pub const DRight: u16 = 0x0002;
    pub const DDown:  u16 = 0x0004;
    pub const DUp:    u16 = 0x0008;
    /// Plus (+) button.
    pub const Plus:   u16 = 0x0010;
    /// Two (2) button.
    pub const Two:    u16 = 0x0100;
    /// One (1) button.
    pub const One:    u16 = 0x0200;
    /// B trigger.
    pub const B:      u16 = 0x0400;
    /// A button.
    pub const A:      u16 = 0x0800;
    /// Minus (−) button.
    pub const Minus:  u16 = 0x1000;
    /// Home button.
    pub const Home:   u16 = 0x8000;
}

/// WiiMote extension attachment type constants (used in `WiimoteState::extension`).
#[allow(non_snake_case, non_upper_case_globals)]
pub mod WiiExtension {
    pub const None:     u8 = 0x00;
    pub const Nunchuck: u8 = 0x01;
    pub const Classic:  u8 = 0x02;
    pub const Guitar:   u8 = 0x03;
    pub const Drums:    u8 = 0x04;
    pub const Unknown:  u8 = 0xFF;
}

/// HID USB keyboard usage IDs (usage page 0x07).
#[allow(non_snake_case)]
pub mod Key {
    pub const A: u8 = 0x04; pub const B: u8 = 0x05; pub const C: u8 = 0x06;
    pub const D: u8 = 0x07; pub const E: u8 = 0x08; pub const F: u8 = 0x09;
    pub const G: u8 = 0x0A; pub const H: u8 = 0x0B; pub const I: u8 = 0x0C;
    pub const J: u8 = 0x0D; pub const K: u8 = 0x0E; pub const L: u8 = 0x0F;
    pub const M: u8 = 0x10; pub const N: u8 = 0x11; pub const O: u8 = 0x12;
    pub const P: u8 = 0x13; pub const Q: u8 = 0x14; pub const R: u8 = 0x15;
    pub const S: u8 = 0x16; pub const T: u8 = 0x17; pub const U: u8 = 0x18;
    pub const V: u8 = 0x19; pub const W: u8 = 0x1A; pub const X: u8 = 0x1B;
    pub const Y: u8 = 0x1C; pub const Z: u8 = 0x1D;
    pub const N1: u8 = 0x1E; pub const N2: u8 = 0x1F; pub const N3: u8 = 0x20;
    pub const N4: u8 = 0x21; pub const N5: u8 = 0x22; pub const N6: u8 = 0x23;
    pub const N7: u8 = 0x24; pub const N8: u8 = 0x25; pub const N9: u8 = 0x26;
    pub const N0: u8 = 0x27;
    pub const ENTER:     u8 = 0x28; pub const ESCAPE:    u8 = 0x29;
    pub const BACKSPACE: u8 = 0x2A; pub const TAB:       u8 = 0x2B;
    pub const SPACE:     u8 = 0x2C; pub const DELETE:     u8 = 0x4C;
    pub const RIGHT: u8 = 0x4F; pub const LEFT:  u8 = 0x50;
    pub const DOWN:  u8 = 0x51; pub const UP:    u8 = 0x52;
    pub const F1: u8 = 0x3A; pub const F2: u8 = 0x3B; pub const F3: u8 = 0x3C;
    pub const F4: u8 = 0x3D; pub const F5: u8 = 0x3E; pub const F6: u8 = 0x3F;
    pub const F7: u8 = 0x40; pub const F8: u8 = 0x41; pub const F9: u8 = 0x42;
    pub const F10: u8 = 0x43; pub const F11: u8 = 0x44; pub const F12: u8 = 0x45;
    pub const MINUS:   u8 = 0x2D; pub const EQUALS:    u8 = 0x2E;
    pub const LBRACE:  u8 = 0x2F; pub const RBRACE:    u8 = 0x30;
    pub const BSLASH:  u8 = 0x31; pub const SEMICOLON: u8 = 0x33;
    pub const QUOTE:   u8 = 0x34; pub const GRAVE:     u8 = 0x35;
    pub const COMMA:   u8 = 0x36; pub const DOT:       u8 = 0x37;
    pub const SLASH:   u8 = 0x38;
    pub const HOME:    u8 = 0x4A; pub const END:     u8 = 0x4D;
    pub const PGUP:    u8 = 0x4B; pub const PGDOWN:  u8 = 0x4E;
    pub const INSERT:  u8 = 0x49;
    pub const MOD_LCTRL:  u8 = 0x01; pub const MOD_LSHIFT: u8 = 0x02;
    pub const MOD_LALT:   u8 = 0x04; pub const MOD_LMETA:  u8 = 0x08;
    pub const MOD_RCTRL:  u8 = 0x10; pub const MOD_RSHIFT: u8 = 0x20;
    pub const MOD_RALT:   u8 = 0x40; pub const MOD_RMETA:  u8 = 0x80;
}

// ─── State structs ────────────────────────────────────────────────────────────

/// Standard GC controller state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadState {
    pub buttons:   u16,
    pub stick_x:   u8,
    pub stick_y:   u8,
    pub cstick_x:  u8,
    pub cstick_y:  u8,
    pub trigger_l: u8,
    pub trigger_r: u8,
}

impl PadState {
    #[inline] pub fn pressed(&self, button: u16) -> bool { self.buttons & button != 0 }
    #[inline] pub fn stick_x_centered(&self)  -> i8 { self.stick_x.wrapping_sub(128) as i8 }
    #[inline] pub fn stick_y_centered(&self)  -> i8 { self.stick_y.wrapping_sub(128) as i8 }
    #[inline] pub fn cstick_x_centered(&self) -> i8 { self.cstick_x.wrapping_sub(128) as i8 }
    #[inline] pub fn cstick_y_centered(&self) -> i8 { self.cstick_y.wrapping_sub(128) as i8 }
}

/// Extended pad state — standard buttons plus 6 modern extras.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtendedPadState {
    pub base:        PadState,
    /// Extra buttons; AND with [`ExtButtons`] constants.
    pub ext_buttons: u8,
}

impl ExtendedPadState {
    #[inline] pub fn held_ext(&self, button: u8) -> bool { self.ext_buttons & button != 0 }
}

/// Full WiiMote state including raw buttons, IR pointer, and accelerometer.
///
/// The `pad` field contains a synthesised GC controller state produced by
/// BlueRetro from the attached extension (Nunchuck, Classic Controller, etc.),
/// so existing code that only cares about standard inputs just works.
#[derive(Debug, Clone, Copy, Default)]
pub struct WiimoteState {
    /// GC-compatible pad state synthesised from the attached extension.
    pub pad:         PadState,
    /// Extra buttons from extension (see [`ExtButtons`]).
    pub ext_buttons: u8,
    /// Attached extension type (see [`WiiExtension`] constants).
    pub extension:   u8,
    /// Raw WiiMote digital buttons (see [`WiiButtons`]).
    pub wii_buttons: u16,
    /// IR pointer X (0–1023). `0xFFFF` when not pointing at screen.
    pub ir_x:        u16,
    /// IR pointer Y (0–767). `0xFFFF` when not pointing at screen.
    pub ir_y:        u16,
    /// Accelerometer X, center = 128.
    pub accel_x:     u8,
    /// Accelerometer Y, center = 128.
    pub accel_y:     u8,
    /// Accelerometer Z, center = 128.
    pub accel_z:     u8,
    pub connected:   u8,
}

impl WiimoteState {
    /// True if the IR pointer is currently locked onto the screen.
    #[inline] pub fn ir_visible(&self) -> bool { self.ir_x != 0xFFFF }
}

/// Keyboard state.
#[derive(Debug, Clone, Copy, Default)]
pub struct KbdState {
    /// Modifier bitmask; AND with `Key::MOD_*` constants.
    pub modifiers: u8,
    /// Up to 6 simultaneous HID key codes. Unused slots are `0x00`.
    pub keys:      [u8; 6],
    pub connected: u8,
}

impl KbdState {
    #[inline] pub fn held(&self, key: u8) -> bool { self.keys.iter().any(|&k| k == key) }
    #[inline] pub fn modifier(&self, m: u8) -> bool { self.modifiers & m != 0 }
}

/// Mouse state (relative deltas).
#[derive(Debug, Clone, Copy, Default)]
pub struct MouseState {
    pub buttons:  u8,
    pub dx:       i16,
    pub dy:       i16,
    pub scroll_y: i8,
    pub scroll_x: i8,
    pub connected: u8,
}

impl MouseState {
    #[inline] pub fn left(&self)   -> bool { self.buttons & 0x01 != 0 }
    #[inline] pub fn right(&self)  -> bool { self.buttons & 0x02 != 0 }
    #[inline] pub fn middle(&self) -> bool { self.buttons & 0x04 != 0 }
}

/// Result of a standard pad poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadResult {
    Ok(PadState),
    NoController,
    Error,
}

// ─── Register helpers ─────────────────────────────────────────────────────────

#[inline(always)]
fn reg(idx: usize) -> *mut u32 { (SI_BASE + idx * 4) as *mut u32 }

/// Execute one SI transfer. Returns `true` on success.
unsafe fn transfer(channel: u32, cmd: u32, outlen: u32, inlen: u32) -> bool {
    core::ptr::write_volatile(reg(REG_TXBUF), cmd);
    let csr = SICOMCSR_TCINT
        | (outlen << 16)
        | (inlen  <<  8)
        | (channel << 1)
        | SICOMCSR_TSTART;
    core::ptr::write_volatile(reg(REG_SICOMCSR), csr);
    let mut timeout = 150_000u32;
    while core::ptr::read_volatile(reg(REG_SICOMCSR)) & SICOMCSR_TSTART != 0 {
        timeout -= 1;
        if timeout == 0 { return false; }
    }
    core::ptr::read_volatile(reg(REG_SICOMCSR)) & SICOMCSR_COMERR == 0
}

/// Copy `buf.len()` bytes from the TXBUF (big-endian words).
unsafe fn txbuf_read(buf: &mut [u8]) {
    let words = (buf.len() + 3) / 4;
    for w in 0..words {
        let val = core::ptr::read_volatile(reg(REG_TXBUF + w));
        let base = w * 4;
        for b in 0..4usize {
            let i = base + b;
            if i < buf.len() {
                buf[i] = ((val >> ((3 - b) * 8)) & 0xFF) as u8;
            }
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Identify the device on `port`. Returns [`DeviceKind::None`] when empty.
///
/// This issues one SI transfer (~few µs). Cache the result in your game loop
/// rather than calling it every frame.
pub unsafe fn identify(port: Port) -> DeviceKind {
    if !transfer(port as u32, CMD_IDENTIFY, 1, 3) { return DeviceKind::None; }
    let mut id = [0u8; 3];
    txbuf_read(&mut id);
    classify(id[0], id[1], id[2])
}

fn classify(b0: u8, b1: u8, b2: u8) -> DeviceKind {
    match b0 {
        0x09 if b1 & EXT_FLAG != 0 => DeviceKind::ExtendedPad {
            has_keyboard: b2 & CAP_KBD     != 0,
            has_mouse:    b2 & CAP_MOUSE   != 0,
            has_wiimote:  b2 & CAP_WIIMOTE != 0,
        },
        0x09 => DeviceKind::StandardPad,
        0x08 if b1 == 0x20 => DeviceKind::PsoKeyboard,
        0x02 => DeviceKind::Mouse,
        0x00 | 0xFF => DeviceKind::None,
        _ => DeviceKind::Unknown(b0, b1, b2),
    }
}

/// Poll a standard GC controller.
pub unsafe fn read_pad(port: Port) -> PadResult {
    if !transfer(port as u32, CMD_POLL, 3, 8) {
        let sisr = core::ptr::read_volatile(reg(REG_SISR));
        let ch_flags = (sisr >> ((3 - port as u32) * 8)) as u8;
        return if ch_flags & SISR_NORESPONSE as u8 != 0 {
            PadResult::NoController
        } else { PadResult::Error };
    }
    let mut buf = [0u8; 8];
    txbuf_read(&mut buf);
    PadResult::Ok(decode_pad(&buf))
}

fn decode_pad(buf: &[u8]) -> PadState {
    let w0 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let w1 = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    PadState {
        buttons:   ((w0 >> 16) & 0x3FFF) as u16,
        stick_x:   ((w0 >>  8) & 0xFF) as u8,
        stick_y:   ((w0      ) & 0xFF) as u8,
        cstick_x:  ((w1 >> 24) & 0xFF) as u8,
        cstick_y:  ((w1 >> 16) & 0xFF) as u8,
        trigger_l: ((w1 >>  8) & 0xFF) as u8,
        trigger_r: ((w1      ) & 0xFF) as u8,
    }
}

/// Poll all four ports.
pub unsafe fn read_all() -> [PadResult; 4] {
    [read_pad(Port::P1), read_pad(Port::P2), read_pad(Port::P3), read_pad(Port::P4)]
}

/// Poll a BlueRetro extended device (16-byte response).
///
/// Falls back to a standard 8-byte poll silently.
pub unsafe fn read_extended(port: Port) -> ExtendedPadState {
    if transfer(port as u32, CMD_EXTENDED, 3, 16) {
        let mut buf = [0u8; 16];
        txbuf_read(&mut buf);
        ExtendedPadState { base: decode_pad(&buf[..8]), ext_buttons: buf[8] }
    } else {
        match read_pad(port) {
            PadResult::Ok(p) => ExtendedPadState { base: p, ext_buttons: 0 },
            _                => ExtendedPadState::default(),
        }
    }
}

/// Poll keyboard state (PSO keyboard or BlueRetro keyboard).
///
/// Key codes use HID usage IDs; see the [`Key`] module for constants.
pub unsafe fn read_kbd(port: Port) -> KbdState {
    if !transfer(port as u32, CMD_KEYBOARD, 3, 7) { return KbdState::default(); }
    let mut buf = [0u8; 7];
    txbuf_read(&mut buf);
    // buf[0] = modifiers, buf[1] = reserved/overflow, buf[2..7] = up to 5 keys
    // We report 6 slots: treat buf[1] as a sixth key slot (overflow flag = 0x01
    // on most implementations, so we mask it and only use it as a keycode when
    // it looks like one, i.e. >= 0x04).
    let mut keys = [0u8; 6];
    let extra = if buf[1] >= 0x04 { buf[1] } else { 0 };
    keys[0..5].copy_from_slice(&buf[2..7]);
    keys[5] = extra;
    KbdState { modifiers: buf[0], keys, connected: 1 }
}

/// Poll GC mouse state.
pub unsafe fn read_mouse(port: Port) -> MouseState {
    if !transfer(port as u32, CMD_MOUSE, 3, 8) { return MouseState::default(); }
    let mut buf = [0u8; 8];
    txbuf_read(&mut buf);
    MouseState {
        buttons:   buf[0],
        dx:        i16::from_be_bytes([buf[2], buf[3]]),
        dy:        i16::from_be_bytes([buf[4], buf[5]]),
        scroll_y:  buf[6] as i8,
        scroll_x:  buf[7] as i8,
        connected: 1,
    }
}

/// Poll full WiiMote state (BlueRetro extended with `CAP_WIIMOTE`).
///
/// ## 24-byte response layout
///
/// ```text
/// Bytes  0–7:   Standard GC pad data (from extension mapping)
/// Byte   8:     Extended buttons ([`ExtButtons`])
/// Byte   9:     Extension type ([`WiiExtension`] constant)
/// Bytes 10–11:  WiiMote button bitmask ([`WiiButtons`])
/// Bytes 12–13:  IR X (0–1023; 0xFFFF = not visible)
/// Bytes 14–15:  IR Y (0–767;  0xFFFF = not visible)
/// Byte  16:     Accelerometer X (center = 128)
/// Byte  17:     Accelerometer Y
/// Byte  18:     Accelerometer Z
/// Bytes 19–23:  Reserved
/// ```
pub unsafe fn read_wiimote(port: Port) -> WiimoteState {
    if !transfer(port as u32, CMD_WIIMOTE, 3, 24) {
        return WiimoteState::default();
    }
    let mut buf = [0u8; 24];
    txbuf_read(&mut buf);
    WiimoteState {
        pad:         decode_pad(&buf[0..8]),
        ext_buttons: buf[8],
        extension:   buf[9],
        wii_buttons: u16::from_be_bytes([buf[10], buf[11]]),
        ir_x:        u16::from_be_bytes([buf[12], buf[13]]),
        ir_y:        u16::from_be_bytes([buf[14], buf[15]]),
        accel_x:     buf[16],
        accel_y:     buf[17],
        accel_z:     buf[18],
        connected:   1,
    }
}
