//! # dkdol-vfs — GameCube/Wii Virtual Filesystem
//!
//! `dkdol-vfs` is a **mandatory** part of the devkit. It always provides:
//!
//! * A `/dev/` device tree populated at boot and refreshed by [`poll`].
//! * Human-interface devices as readable files (`/dev/hid/p1/std`, etc.).
//! * `/dev/null` and `/dev/zero`.
//! * Enumeration of all connected storage hardware.
//!
//! When any of the `fat`, `ext2`, `iso9660`, `dvd`, or `memcard` Cargo
//! features are enabled (or `all-fs` for all of them), `dkdol-fs` is pulled in
//! and filesystem access becomes available. Storage devices listed in `/dev/`
//! are **lazily mounted** — the filesystem is not initialised until code first
//! opens a path beneath that device prefix.
//!
//! ## Typical usage
//!
//! ```toml
//! # Cargo.toml
//! [dependencies]
//! dkdol-vfs = { path = "…", features = ["all-fs"] }
//! ```
//!
//! ```rust,no_run
//! use dkdol_vfs::{self as vfs, O_RDONLY, O_RDWR, O_CREAT, MountOptions};
//!
//! unsafe {
//!     vfs::init();          // probe hardware, populate /dev/, no mounts yet
//!
//!     // Filesystem access — lazy-mounts /dev/sd/sp on first use
//!     let fd = vfs::open("/dev/sd/sp/boot.dol", O_RDONLY).unwrap();
//!     let mut buf = [0u8; 4096];
//!     let n = vfs::read(fd, &mut buf).unwrap();
//!     vfs::close(fd);
//!
//!     // Controller — always works, no filesystem needed
//!     let pad = vfs::open("/dev/hid/p1/std", O_RDONLY).unwrap();
//!     let mut state = vfs::ControllerState::default();
//!     vfs::read(pad, bytemuck::bytes_of_mut(&mut state)).unwrap();
//!     vfs::close(pad);
//!
//!     // Call once per frame to detect inserted/removed cards
//!     vfs::poll();
//! }
//! ```
//!
//! ## Safety
//!
//! Every public function is `unsafe` — they mutate `static mut` tables.
//! Call from one thread only (bare-metal, no RTOS).

#![no_std]

#[cfg(feature = "wii")]
mod wii;

pub mod hid;

// ─── Re-exports used by callers ───────────────────────────────────────────────

pub use dkdol_hal::si::{
    Port, PadState, PadResult, Buttons, ExtButtons, Key,
    KbdState, MouseState, ExtendedPadState, WiimoteState,
    WiiButtons, WiiExtension, DeviceKind,
};

#[cfg(feature = "_fs")]
pub use dkdol_fs::{FsKind, Metadata};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum simultaneously open file descriptors.
pub const MAX_FD: usize = 32;

/// Maximum mounted filesystem slots.
/// Unused when no filesystem feature is enabled.
pub const MAX_VOLUMES: usize = 8;

/// Maximum nodes in the device tree.
pub const MAX_DEVICES: usize = 24;

// open() flags
pub const O_RDONLY: u32 = 0x0001;
pub const O_WRONLY: u32 = 0x0002;
pub const O_RDWR:   u32 = 0x0003;
pub const O_CREAT:  u32 = 0x0100;
pub const O_TRUNC:  u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;

/// Integer file descriptor, 0 – `MAX_FD - 1`.
pub type Fd = u8;

/// Which kind of HID device to open on a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidSlot {
    /// Standard controller (or synthesised pad from keyboard).
    Std,
    /// Keyboard (PSO keyboard or BlueRetro keyboard).
    Kbd,
    /// Mouse (GC mouse or BlueRetro mouse); IR for WiiMote (absolute coords).
    Mouse,
    /// Full WiiMote state (BlueRetro extended mode with WiiMote).
    Wii,
}

// ─── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Path not found in the device tree.
    NotFound,
    /// Hardware or sector-level I/O failure.
    Io,
    /// On-disk structure is corrupt or unrecognised.
    BadFormat,
    /// Write attempted on a read-only device or file.
    ReadOnly,
    /// No space left on the device.
    NoSpace,
    /// Path refers to a directory, not a file (or vice-versa).
    WrongType,
    /// Directory is not empty.
    NotEmpty,
    /// All `MAX_FD` file descriptor slots are occupied.
    TooManyOpen,
    /// `fd` argument is out of range or not open.
    InvalidFd,
    /// Argument value is out of range.
    InvalidArg,
    /// Operation is valid but not implemented for this device / filesystem.
    Unsupported,
    /// Device exists in `/dev/` but no hardware was detected.
    NoDevice,
    /// Volume has open files; unmount refused.
    Busy,
    /// A filesystem feature is required but was not compiled in
    /// (add `dkdol-vfs = { features = ["all-fs"] }` to `Cargo.toml`).
    FsNotAvailable,
}

pub type Result<T> = core::result::Result<T, Error>;

#[cfg(feature = "_fs")]
impl From<dkdol_fs::FsError> for Error {
    fn from(e: dkdol_fs::FsError) -> Self {
        use dkdol_fs::FsError::*;
        match e {
            Io            => Error::Io,
            BadFormat     => Error::BadFormat,
            NotFound      => Error::NotFound,
            ReadOnly      => Error::ReadOnly,
            Eof           => Error::Io,
            BufferTooSmall=> Error::InvalidArg,
            InvalidArg    => Error::InvalidArg,
            NoSpace       => Error::NoSpace,
            NotEmpty      => Error::NotEmpty,
            WrongType     => Error::WrongType,
            Unsupported   => Error::Unsupported,
            TooManyMounts => Error::TooManyOpen,
            FilesOpen     => Error::Busy,
        }
    }
}

// ─── Mount options ────────────────────────────────────────────────────────────

/// Controls how a storage device is mounted when first accessed.
#[derive(Clone, Copy, Debug)]
pub struct MountOptions {
    /// Journaling policy for EXT3/4 volumes.
    pub journal:  JournalMode,
    /// Mount read-only even on writable hardware.
    pub readonly: bool,
}

impl MountOptions {
    pub const DEFAULT: MountOptions = MountOptions {
        journal:  JournalMode::Auto,
        readonly: false,
    };
    pub const READONLY: MountOptions = MountOptions {
        journal:  JournalMode::Auto,
        readonly: true,
    };
}

impl Default for MountOptions { fn default() -> Self { MountOptions::DEFAULT } }

/// Journaling policy (EXT3/4 only; ignored for FAT/ISO/MemCard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalMode {
    /// Enable journaling when supported.  Refuse dirty EXT3/4. (default)
    Auto,
    /// Enable journaling and replay the journal on a dirty unmount.
    Replay,
    /// Disable journaling — EXT2 semantics on every filesystem.
    Disable,
}

// ─── Controller / HID types ───────────────────────────────────────────────────

/// Snapshot of one GC controller port, as read from `/dev/hid/pN/std`.
///
/// 16 bytes, `repr(C)`. The first 9 bytes match the original layout so
/// existing code that reads only the standard fields is unaffected.
/// `ext_buttons` carries the 6 extra digital buttons from BlueRetro
/// modern controllers; it is `0` on a standard GC controller.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerState {
    /// Pressed buttons — AND with [`Buttons`] constants.
    pub buttons:     u16,
    pub stick_x:     u8,   // center ≈ 128
    pub stick_y:     u8,
    pub cstick_x:    u8,
    pub cstick_y:    u8,
    pub trigger_l:   u8,
    pub trigger_r:   u8,
    /// 1 = controller/keyboard present, 0 = nothing plugged in.
    pub connected:   u8,
    /// Extended digital buttons — AND with [`ExtButtons`] constants.
    /// Zero on a standard GC controller; non-zero on BlueRetro modern pads.
    pub ext_buttons: u8,
    _pad: [u8; 6],
}

/// Keyboard state, as read from `/dev/hid/pN/kbd`. 16 bytes, `repr(C)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KbdState {
    /// Modifier bitmask — AND with `Key::MOD_*` constants.
    pub modifiers: u8,
    _reserved:     u8,
    /// Up to 6 simultaneously held HID key codes. Unused slots are `0`.
    pub keys:      [u8; 6],
    /// 1 = keyboard present, 0 = not present.
    pub connected: u8,
    _pad:          [u8; 7],
}

/// Mouse state, as read from `/dev/hid/pN/mouse`. 16 bytes, `repr(C)`.
///
/// When `absolute == 1` (WiiMote IR mode), `dx` and `dy` carry absolute
/// screen coordinates (0–1023 × 0–767) instead of movement deltas.
/// `dx == -1` (0xFFFF as u16) means the pointer is not visible.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MouseState {
    /// Button bitmask: bit 0=left, 1=right, 2=middle, 3=back, 4=forward.
    pub buttons:   u8,
    /// 0 = relative delta, 1 = absolute coordinates (WiiMote IR).
    pub absolute:  u8,
    /// Horizontal: delta (relative) or X coordinate 0–1023 (absolute).
    pub dx:        i16,
    /// Vertical: delta (relative) or Y coordinate 0–767 (absolute).
    pub dy:        i16,
    /// Vertical scroll delta (relative mode only).
    pub scroll_y:  i8,
    /// Horizontal scroll delta (relative mode only).
    pub scroll_x:  i8,
    /// 1 = mouse/WiiMote present, 0 = not present.
    pub connected: u8,
    _pad:          [u8; 7],
}

impl ControllerState {
    pub fn from_pad(pad: &PadState) -> Self {
        ControllerState {
            buttons:     pad.buttons,
            stick_x:     pad.stick_x,
            stick_y:     pad.stick_y,
            cstick_x:    pad.cstick_x,
            cstick_y:    pad.cstick_y,
            trigger_l:   pad.trigger_l,
            trigger_r:   pad.trigger_r,
            connected:   1,
            ext_buttons: 0,
            _pad: [0u8; 6],
        }
    }

    pub fn from_extended(ext: &dkdol_hal::si::ExtendedPadState) -> Self {
        let mut s = Self::from_pad(&ext.base);
        s.ext_buttons = ext.ext_buttons;
        s
    }


}

// ─── Internal: device tree ────────────────────────────────────────────────────

/// What hardware backs a device node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DevKind {
    /// Slot is unused.
    Empty,
    /// SD card via SD2SP2 (EXI channel 2).
    SdSp2,
    /// SD Gecko in GC memory-card slot A (EXI channel 0).
    SdSlotA,
    /// SD Gecko in GC memory-card slot B (EXI channel 1).
    SdSlotB,
    /// Real GameCube DVD drive.
    DvdDrive,
    /// Genuine GC memory card in slot A.
    MemCardA,
    /// Genuine GC memory card in slot B.
    MemCardB,
    /// Controller port 0–3.
    HidPort(u8),
    /// /dev/null
    Null,
    /// /dev/zero
    Zero,
}

/// One node in the `/dev/` tree.
#[derive(Clone, Copy)]
struct DevNode {
    /// Null-terminated path, e.g. `"/dev/sd/sp"` or `"/dev/hid/p1/std"`.
    path:    [u8; 20],
    kind:    DevKind,
    /// True when hardware is actually present.
    present: bool,
}

impl DevNode {
    const fn empty() -> Self {
        DevNode { path: [0u8; 20], kind: DevKind::Empty, present: false }
    }
    fn path_matches(&self, query: &str) -> bool {
        let q = query.as_bytes();
        let n = q.len().min(19);
        &self.path[..n] == &q[..n] && self.path[n] == 0
    }
    fn path_str(&self) -> &str {
        let end = self.path.iter().position(|&b| b == 0).unwrap_or(20);
        core::str::from_utf8(&self.path[..end]).unwrap_or("")
    }
    fn set_path(mut self, s: &str) -> Self {
        let b = s.as_bytes();
        let n = b.len().min(19);
        self.path[..n].copy_from_slice(&b[..n]);
        self.path[n] = 0;
        self
    }
}

// ─── Internal: filesystem volume ──────────────────────────────────────────────
//
// This entire block is omitted from the binary when no filesystem feature
// is enabled, saving significant code size.

#[cfg(feature = "_fs")]
enum VolumeInner {
    /// Slot is unoccupied.
    Empty,
    #[cfg(feature = "fat")]
    Fat(dkdol_fs::fat::FatVolume<dkdol_hal::sd::SdCard>),
    #[cfg(feature = "ext2")]
    Ext2(dkdol_fs::ext2::Ext2<dkdol_hal::sd::SdCard>),
    #[cfg(feature = "iso9660")]
    Iso(dkdol_fs::iso9660::Iso9660<dkdol_hal::dvd::DvdDisk>),
    #[cfg(feature = "dvd")]
    GcDvd(dkdol_fs::dvd::GcDvd<dkdol_hal::dvd::DvdDisk>),
    #[cfg(feature = "memcard")]
    MemCard(dkdol_fs::memcard::MemCardFs),
}

#[cfg(feature = "_fs")]
struct VolumeSlot {
    /// Device prefix this volume is mounted on, e.g. `"/dev/sd/sp"`.
    prefix:     [u8; 20],
    open_count: u8,
    opts:       MountOptions,
    inner:      VolumeInner,
}

#[cfg(feature = "_fs")]
impl VolumeSlot {
    const fn empty() -> Self {
        VolumeSlot {
            prefix:     [0u8; 20],
            open_count: 0,
            opts:       MountOptions::DEFAULT,
            inner:      VolumeInner::Empty,
        }
    }
    fn is_empty(&self) -> bool { matches!(self.inner, VolumeInner::Empty) }
    fn prefix_str(&self) -> &str {
        let end = self.prefix.iter().position(|&b| b == 0).unwrap_or(20);
        core::str::from_utf8(&self.prefix[..end]).unwrap_or("")
    }
    fn set_prefix(&mut self, s: &str) {
        let b = s.as_bytes(); let n = b.len().min(19);
        self.prefix[..n].copy_from_slice(&b[..n]); self.prefix[n] = 0;
    }
}

// ─── Internal: open file ──────────────────────────────────────────────────────

enum VfsFile {
    Empty,

    // ── Always available ──────────────────────────────────────────────────

    /// Standard GC controller port (or synthesised pad from keyboard).
    Controller { port: u8 },
    /// Keyboard device (PSO keyboard or BlueRetro keyboard).
    Keyboard { port: u8 },
    /// Mouse device (GC mouse or BlueRetro mouse).
    Mouse { port: u8 },
    /// Full WiiMote state (BlueRetro extended).
    Wiimote { port: u8 },

    /// `/dev/null` — reads return 0 bytes; writes succeed silently.
    Null,

    /// `/dev/zero` — reads return 0x00 bytes.
    Zero { pos: u64 },

    // ── Filesystem files (require a filesystem feature) ───────────────────

    /// FAT32 / ExFAT file.
    #[cfg(feature = "fat")]
    FatFile {
        vol_idx:  usize,
        start:    u32,
        cur:      u32,
        size:     u64,
        pos:      u64,
        clus_pos: u64,
    },

    /// EXT2/3/4 file.
    #[cfg(feature = "ext2")]
    Ext2File {
        vol_idx:   usize,
        ino:       u32,
        size:      u64,
        flags:     u32,
        block_raw: [u8; 60],
        blocks:    [u32; 15],
        pos:       u64,
    },

    /// ISO 9660 file (read-only).
    ///
    /// All data needed for reading is stored by value (lba + size) so no
    /// lifetime reference to the volume is needed.
    #[cfg(feature = "iso9660")]
    IsoFile {
        vol_idx:   usize,
        lba_start: u64,
        size:      u64,
        pos:       u64,
    },

    /// GameCube FST file (read-only).
    #[cfg(feature = "dvd")]
    DvdFile {
        vol_idx:   usize,
        offset:    u64,
        size:      u64,
        pos:       u64,
    },

    /// GC memory-card file.
    #[cfg(feature = "memcard")]
    McFile {
        vol_idx: usize,
        entry:   u16,
        pos:     u32,
    },
}

impl VfsFile {
    const fn empty() -> Self { VfsFile::Empty }
    fn is_empty(&self) -> bool { matches!(self, VfsFile::Empty) }

    fn size(&self) -> u64 {
        match self {
            VfsFile::Zero { .. }  => u64::MAX,
            #[cfg(feature = "fat")]
            VfsFile::FatFile { size, .. } => *size,
            #[cfg(feature = "ext2")]
            VfsFile::Ext2File { size, .. } => *size,
            #[cfg(feature = "iso9660")]
            VfsFile::IsoFile { size, .. } => *size,
            #[cfg(feature = "dvd")]
            VfsFile::DvdFile { size, .. } => *size,
            _ => 0,
        }
    }

    fn pos(&self) -> u64 {
        match self {
            VfsFile::Zero { pos }  => *pos,
            #[cfg(feature = "fat")]
            VfsFile::FatFile { pos, .. } => *pos,
            #[cfg(feature = "ext2")]
            VfsFile::Ext2File { pos, .. } => *pos,
            #[cfg(feature = "iso9660")]
            VfsFile::IsoFile { pos, .. } => *pos,
            #[cfg(feature = "dvd")]
            VfsFile::DvdFile { pos, .. } => *pos,
            _ => 0,
        }
    }

    /// Decrement the owning volume's open_count when this handle is released.
    #[cfg(feature = "_fs")]
    fn vol_idx(&self) -> Option<usize> {
        match self {
            #[cfg(feature = "fat")]      VfsFile::FatFile  { vol_idx, .. } => Some(*vol_idx),
            #[cfg(feature = "ext2")]     VfsFile::Ext2File { vol_idx, .. } => Some(*vol_idx),
            #[cfg(feature = "iso9660")]  VfsFile::IsoFile  { vol_idx, .. } => Some(*vol_idx),
            #[cfg(feature = "dvd")]      VfsFile::DvdFile  { vol_idx, .. } => Some(*vol_idx),
            #[cfg(feature = "memcard")]  VfsFile::McFile   { vol_idx, .. } => Some(*vol_idx),
            _ => None,
        }
    }
}

// ─── Static state ─────────────────────────────────────────────────────────────

/// Device nodes, initialised by [`init`] and refreshed by [`poll`].
static mut DEVICES: [DevNode; MAX_DEVICES] = {
    const E: DevNode = DevNode::empty();
    [E; MAX_DEVICES]
};

/// Mounted filesystem volumes.
#[cfg(feature = "_fs")]
static mut VOLUMES: [VolumeSlot; MAX_VOLUMES] = {
    const E: VolumeSlot = VolumeSlot::empty();
    [E; MAX_VOLUMES]
};

/// Open file descriptor table.
static mut FD_TABLE: [VfsFile; MAX_FD] = {
    const E: VfsFile = VfsFile::empty();
    [E; MAX_FD]
};

/// `dkdol_rt` tick at last [`poll`] call per device kind.
/// On Wii, tracks whether each port has an IOS-backed device
/// (Wiimote or USB HID) independent of the SI bus.
#[cfg(feature = "wii")]
static mut WII_PRESENT: [bool; 4] = [false; 4];

/// Cached device kind per controller port — refreshed by `poll()`.
/// Avoids calling `identify()` on every `read()`.
static mut PORT_KINDS: [DeviceKind; 4] = [
    DeviceKind::None, DeviceKind::None, DeviceKind::None, DeviceKind::None,
];

static mut LAST_POLL_SD:  u64 = 0;
static mut LAST_POLL_DVD: u64 = 0;
static mut LAST_POLL_MC:  u64 = 0;

/// Mount options used when lazy-mounting.  Set by [`set_default_mount_opts`].
static mut DEFAULT_OPTS: MountOptions = MountOptions::DEFAULT;

// ─── Path helpers ─────────────────────────────────────────────────────────────

/// Split `/dev/sd/sp/a/b/c` → `("/dev/sd/sp", "a/b/c")`.
///
/// Returns `None` if the path does not start with `/dev/`.
fn split_dev_path(path: &str) -> Option<(&str, &str)> {
    if !path.starts_with("/dev/") { return None; }

    // Try to match the longest known device prefix
    const PREFIXES: &[&str] = &[
        "/dev/sd/sp",
        "/dev/sd/m1",
        "/dev/sd/m2",
        "/dev/dvd",
        "/dev/mem1",
        "/dev/mem2",
        "/dev/hid/p1/std",
        "/dev/hid/p1/kbd",
        "/dev/hid/p1/mouse",
        "/dev/hid/p1/wii",
        "/dev/hid/p2/std",
        "/dev/hid/p2/kbd",
        "/dev/hid/p2/mouse",
        "/dev/hid/p2/wii",
        "/dev/hid/p3/std",
        "/dev/hid/p3/kbd",
        "/dev/hid/p3/mouse",
        "/dev/hid/p3/wii",
        "/dev/hid/p4/std",
        "/dev/hid/p4/kbd",
        "/dev/hid/p4/mouse",
        "/dev/hid/p4/wii",
        "/dev/null",
        "/dev/zero",
    ];

    for pfx in PREFIXES {
        if path == *pfx {
            return Some((pfx, ""));
        }
        let with_sep = &mut [0u8; 20];
        let pl = pfx.len().min(19);
        with_sep[..pl].copy_from_slice(pfx.as_bytes());
        with_sep[pl] = b'/';
        let sep = core::str::from_utf8(&with_sep[..pl+1]).unwrap_or("");
        if path.starts_with(sep) {
            let rest = &path[pl+1..];
            return Some((pfx, rest));
        }
    }

    // No prefix matched — path might be a device prefix itself
    Some((path, ""))
}

fn dev_kind_for(dev_prefix: &str) -> DevKind {
    match dev_prefix {
        "/dev/sd/sp"     => DevKind::SdSp2,
        "/dev/sd/m1"     => DevKind::SdSlotA,
        "/dev/sd/m2"     => DevKind::SdSlotB,
        "/dev/dvd"       => DevKind::DvdDrive,
        "/dev/mem1"      => DevKind::MemCardA,
        "/dev/mem2"      => DevKind::MemCardB,
        "/dev/hid/p1/std" | "/dev/hid/p1/kbd" | "/dev/hid/p1/mouse" | "/dev/hid/p1/wii" => DevKind::HidPort(0),
        "/dev/hid/p2/std" | "/dev/hid/p2/kbd" | "/dev/hid/p2/mouse" | "/dev/hid/p2/wii" => DevKind::HidPort(1),
        "/dev/hid/p3/std" | "/dev/hid/p3/kbd" | "/dev/hid/p3/mouse" | "/dev/hid/p3/wii" => DevKind::HidPort(2),
        "/dev/hid/p4/std" | "/dev/hid/p4/kbd" | "/dev/hid/p4/mouse" | "/dev/hid/p4/wii" => DevKind::HidPort(3),
        "/dev/null"      => DevKind::Null,
        "/dev/zero"      => DevKind::Zero,
        _                => DevKind::Empty,
    }
}

fn port_for(dev_prefix: &str) -> Option<u8> {
    match dev_prefix {
        "/dev/hid/p1/std" | "/dev/hid/p1/kbd" | "/dev/hid/p1/mouse" | "/dev/hid/p1/wii" => Some(0),
        "/dev/hid/p2/std" | "/dev/hid/p2/kbd" | "/dev/hid/p2/mouse" | "/dev/hid/p2/wii" => Some(1),
        "/dev/hid/p3/std" | "/dev/hid/p3/kbd" | "/dev/hid/p3/mouse" | "/dev/hid/p3/wii" => Some(2),
        "/dev/hid/p4/std" | "/dev/hid/p4/kbd" | "/dev/hid/p4/mouse" | "/dev/hid/p4/wii" => Some(3),
        _ => None,
    }
}

unsafe fn dev_is_present(dev_prefix: &str) -> bool {
    for node in DEVICES.iter() {
        if node.kind == DevKind::Empty { continue; }
        if node.path_matches(dev_prefix) { return node.present; }
    }
    false
}

// ─── Volume table helpers ──────────────────────────────────────────────────────

#[cfg(feature = "_fs")]
unsafe fn find_mounted(dev_prefix: &str) -> Option<usize> {
    for (i, v) in VOLUMES.iter().enumerate() {
        if !v.is_empty() && v.prefix_str() == dev_prefix { return Some(i); }
    }
    None
}

#[cfg(feature = "_fs")]
unsafe fn alloc_volume_slot() -> Option<usize> {
    VOLUMES.iter().position(|v| v.is_empty())
}

// ─── Lazy mount ───────────────────────────────────────────────────────────────
//
// Called the first time a path beneath a device prefix is opened.
// Probes the hardware and mounts the appropriate filesystem.

#[cfg(feature = "_fs")]
unsafe fn lazy_mount(dev_prefix: &str, opts: MountOptions) -> Result<usize> {
    use dkdol_hal::sd::{SdCard, Slot as SdSlot};
    use dkdol_hal::dvd::DvdDisk;

    let slot_idx = alloc_volume_slot().ok_or(Error::TooManyOpen)?;
    let vol = &mut VOLUMES[slot_idx];
    vol.set_prefix(dev_prefix);
    vol.opts = opts;

    match dev_prefix {
        "/dev/sd/sp" | "/dev/sd/m1" | "/dev/sd/m2" => {
            let sd_slot = match dev_prefix {
                "/dev/sd/sp" => SdSlot::Sp2,
                "/dev/sd/m1" => SdSlot::A,
                _            => SdSlot::B,
            };

            // Try FAT32 first (most common on GC/Wii SD cards)
            #[cfg(feature = "fat")]
            {
                let mut card = SdCard::new(sd_slot);
                if card.init().is_ok() {
                    match dkdol_fs::fat::FatVolume::mount(card) {
                        Ok(fv) => {
                            vol.inner = VolumeInner::Fat(fv);
                            return Ok(slot_idx);
                        }
                        Err(_) => {}
                    }
                }
            }

            // FAT failed or not compiled; try EXT2/3/4
            #[cfg(feature = "ext2")]
            {
                let mut card = SdCard::new(sd_slot);
                if card.init().is_ok() {
                    let jmode = match (opts.journal, opts.readonly) {
                        (JournalMode::Disable, _) => dkdol_fs::ext2::JournalMode::Ignore,
                        (JournalMode::Replay, _)  => dkdol_fs::ext2::JournalMode::Ignore,
                        _                          => dkdol_fs::ext2::JournalMode::RequireClean,
                    };
                    match dkdol_fs::ext2::Ext2::mount_opts(card, jmode) {
                        Ok(ev) => {
                            vol.inner = VolumeInner::Ext2(ev);
                            return Ok(slot_idx);
                        }
                        Err(_) => {}
                    }
                }
            }

            vol.inner = VolumeInner::Empty;
            Err(Error::BadFormat)
        }

        "/dev/dvd" => {
            dkdol_hal::dvd::init();

            // Try GC FST disc first
            #[cfg(feature = "dvd")]
            {
                if let Ok(dv) = dkdol_fs::dvd::GcDvd::mount(DvdDisk) {
                    vol.inner = VolumeInner::GcDvd(dv);
                    return Ok(slot_idx);
                }
            }

            // Fall back to ISO 9660
            #[cfg(feature = "iso9660")]
            {
                if let Ok(iv) = dkdol_fs::iso9660::Iso9660::mount(DvdDisk) {
                    vol.inner = VolumeInner::Iso(iv);
                    return Ok(slot_idx);
                }
            }

            vol.inner = VolumeInner::Empty;
            Err(Error::BadFormat)
        }

        "/dev/mem1" | "/dev/mem2" => {
            #[cfg(feature = "memcard")]
            {
                let mc_slot = if dev_prefix == "/dev/mem1" {
                    dkdol_hal::memcard::CardSlot::A
                } else {
                    dkdol_hal::memcard::CardSlot::B
                };
                match dkdol_fs::memcard::MemCardFs::mount(mc_slot) {
                    Ok(mc) => {
                        vol.inner = VolumeInner::MemCard(mc);
                        return Ok(slot_idx);
                    }
                    Err(e) => { vol.inner = VolumeInner::Empty; return Err(e.into()); }
                }
            }
            #[cfg(not(feature = "memcard"))]
            { vol.inner = VolumeInner::Empty; Err(Error::FsNotAvailable) }
        }

        _ => { vol.inner = VolumeInner::Empty; Err(Error::NotFound) }
    }
}

// ─── File I/O dispatch ────────────────────────────────────────────────────────

fn si_port(idx: u8) -> dkdol_hal::si::Port {
    match idx {
        0 => dkdol_hal::si::Port::P1,
        1 => dkdol_hal::si::Port::P2,
        2 => dkdol_hal::si::Port::P3,
        _ => dkdol_hal::si::Port::P4,
    }
}

unsafe fn do_read(file: &mut VfsFile, buf: &mut [u8]) -> Result<usize> {
    match file {
        VfsFile::Empty => Err(Error::InvalidFd),

        VfsFile::Null  => Ok(0),

        VfsFile::Zero { pos } => {
            buf.fill(0);
            *pos += buf.len() as u64;
            Ok(buf.len())
        }

        VfsFile::Controller { port } => {
            let sp   = si_port(*port);
            let kind = PORT_KINDS[*port as usize];
            let want = core::mem::size_of::<ControllerState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            // For WiiMote: extract the synthesised pad from the full state.
            // For extended: use extended poll (falls back to standard).
            // For PSO keyboard: use standard 0x40 poll (keyboard has real
            //   gamepad buttons — no remapping is done here).
            // For everything else: standard 0x40 poll.
            let state = match kind {
                DeviceKind::ExtendedPad { has_wiimote: true, .. } => {
                    let wii = dkdol_hal::si::read_wiimote(sp);
                    ControllerState {
                        buttons: wii.pad.buttons, stick_x: wii.pad.stick_x,
                        stick_y: wii.pad.stick_y, cstick_x: wii.pad.cstick_x,
                        cstick_y: wii.pad.cstick_y,
                        trigger_l: wii.pad.trigger_l, trigger_r: wii.pad.trigger_r,
                        connected: wii.connected, ext_buttons: wii.ext_buttons,
                        _pad: [0u8; 6],
                    }
                }
                DeviceKind::ExtendedPad { .. } => {
                    let ext = dkdol_hal::si::read_extended(sp);
                    ControllerState::from_extended(&ext)
                }
                DeviceKind::None => ControllerState::default(),
                _ => match dkdol_hal::si::read_pad(sp) {
                    PadResult::Ok(pad) => ControllerState::from_pad(&pad),
                    _                  => ControllerState::default(),
                },
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        VfsFile::Keyboard { port } => {
            let si_port = si_port(*port);
            let want = core::mem::size_of::<KbdState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            let raw = dkdol_hal::si::read_kbd(si_port);
            let state = KbdState {
                modifiers: raw.modifiers,
                _reserved: 0, // layout padding, always 0
                keys:      raw.keys,
                connected: raw.connected,
                _pad:      [0u8; 7],
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        VfsFile::Mouse { port } => {
            let sp   = si_port(*port);
            let kind = PORT_KINDS[*port as usize];
            let want = core::mem::size_of::<MouseState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            // WiiMote: report IR pointer as absolute coordinates.
            // Standard mouse: report relative deltas.
            let state = if kind.has_wiimote() {
                let wii = dkdol_hal::si::read_wiimote(sp);
                MouseState {
                    buttons:  wii.wii_buttons as u8 & 0x0F, // A=bit3, B=bit2, etc.
                    absolute: 1,
                    dx:       if wii.ir_x == 0xFFFF { -1i16 } else { wii.ir_x as i16 },
                    dy:       if wii.ir_y == 0xFFFF { -1i16 } else { wii.ir_y as i16 },
                    scroll_y: 0, scroll_x: 0,
                    connected: wii.connected,
                    _pad: [0u8; 7],
                }
            } else {
                let raw = dkdol_hal::si::read_mouse(sp);
                MouseState {
                    buttons:  raw.buttons,
                    absolute: 0,
                    dx:       raw.dx, dy: raw.dy,
                    scroll_y: raw.scroll_y, scroll_x: raw.scroll_x,
                    connected: raw.connected,
                    _pad: [0u8; 7],
                }
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        VfsFile::Wiimote { port } => {
            let sp   = si_port(*port);
            let want = core::mem::size_of::<WiimoteState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            let raw = dkdol_hal::si::read_wiimote(sp);
            let state = WiimoteState {
                buttons:     raw.pad.buttons,
                stick_x:     raw.pad.stick_x,    stick_y:  raw.pad.stick_y,
                cstick_x:    raw.pad.cstick_x,   cstick_y: raw.pad.cstick_y,
                trigger_l:   raw.pad.trigger_l,  trigger_r: raw.pad.trigger_r,
                ext_buttons: raw.ext_buttons,
                extension:   raw.extension,
                wii_buttons: raw.wii_buttons,
                ir_x:        raw.ir_x,            ir_y:     raw.ir_y,
                accel_x:     raw.accel_x,
                accel_y:     raw.accel_y,
                accel_z:     raw.accel_z,
                connected:   raw.connected,
                _pad:        [0u8; 4],
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        #[cfg(feature = "fat")]
        VfsFile::FatFile { vol_idx, cur, size, pos, clus_pos, .. } => {
            match &VOLUMES[*vol_idx].inner {
                VolumeInner::Fat(fv) =>
                    fv.raw_read(cur, pos, clus_pos, *size, buf).map_err(Into::into),
                _ => Err(Error::Io),
            }
        }

        #[cfg(feature = "ext2")]
        VfsFile::Ext2File { vol_idx, flags, block_raw, blocks, size, pos, .. } => {
            match &VOLUMES[*vol_idx].inner {
                VolumeInner::Ext2(ev) =>
                    ev.raw_read(*flags, block_raw, blocks, *size, pos, buf).map_err(Into::into),
                _ => Err(Error::Io),
            }
        }

        #[cfg(feature = "iso9660")]
        VfsFile::IsoFile { vol_idx, lba_start, size, pos } => {
            match &mut VOLUMES[*vol_idx].inner {
                VolumeInner::Iso(iv) => {
                    iv.raw_read(*lba_start, *size, pos, buf).map_err(Into::into)
                }
                _ => Err(Error::Io),
            }
        }

        #[cfg(feature = "dvd")]
        VfsFile::DvdFile { vol_idx, offset, size, pos } => {
            match &mut VOLUMES[*vol_idx].inner {
                VolumeInner::GcDvd(dv) =>
                    dv.raw_read(*offset, *size, pos, buf).map_err(Into::into),
                _ => Err(Error::Io),
            }
        }

        #[cfg(feature = "memcard")]
        VfsFile::McFile { vol_idx, entry, pos } => {
            match &VOLUMES[*vol_idx].inner {
                VolumeInner::MemCard(mc) =>
                    mc.raw_read(*entry, pos, buf).map_err(Into::into),
                _ => Err(Error::Io),
            }
        }
    }
}

unsafe fn do_write(file: &mut VfsFile, buf: &[u8]) -> Result<usize> {
    match file {
        VfsFile::Empty => Err(Error::InvalidFd),
        VfsFile::Null  => Ok(buf.len()),
        VfsFile::Zero { .. } | VfsFile::Controller { .. } => Err(Error::ReadOnly),

        VfsFile::Wiimote { port } => {
            let sp   = si_port(*port);
            let want = core::mem::size_of::<WiimoteState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            let raw = dkdol_hal::si::read_wiimote(sp);
            let state = WiimoteState {
                buttons:     raw.pad.buttons,
                stick_x:     raw.pad.stick_x,    stick_y:  raw.pad.stick_y,
                cstick_x:    raw.pad.cstick_x,   cstick_y: raw.pad.cstick_y,
                trigger_l:   raw.pad.trigger_l,  trigger_r: raw.pad.trigger_r,
                ext_buttons: raw.ext_buttons,
                extension:   raw.extension,
                wii_buttons: raw.wii_buttons,
                ir_x:        raw.ir_x,            ir_y:     raw.ir_y,
                accel_x:     raw.accel_x,
                accel_y:     raw.accel_y,
                accel_z:     raw.accel_z,
                connected:   raw.connected,
                _pad:        [0u8; 4],
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        #[cfg(feature = "fat")]
        VfsFile::FatFile { vol_idx, start, cur, size, pos, clus_pos } => {
            match &mut VOLUMES[*vol_idx].inner {
                VolumeInner::Fat(fv) if !fv.is_readonly() =>
                    fv.raw_write(*start, cur, pos, clus_pos, size, buf).map_err(Into::into),
                _ => Err(Error::ReadOnly),
            }
        }

        #[cfg(feature = "ext2")]
        VfsFile::Ext2File { vol_idx, ino, flags, block_raw, blocks, size, pos } => {
            match &mut VOLUMES[*vol_idx].inner {
                VolumeInner::Ext2(ev) =>
                    ev.raw_write(*ino, flags, block_raw, blocks, size, pos, buf)
                      .map_err(Into::into),
                _ => Err(Error::ReadOnly),
            }
        }

        _ => Err(Error::ReadOnly),
    }
}

unsafe fn do_seek(file: &mut VfsFile, target: u64) -> Result<u64> {
    match file {
        VfsFile::Empty => Err(Error::InvalidFd),
        VfsFile::Null  => Ok(0),
        VfsFile::Zero { pos } => { *pos = target; Ok(target) }
        VfsFile::Controller { .. } => Ok(0),

        VfsFile::Wiimote { port } => {
            let sp   = si_port(*port);
            let want = core::mem::size_of::<WiimoteState>();
            if buf.len() < want { return Err(Error::InvalidArg); }
            let raw = dkdol_hal::si::read_wiimote(sp);
            let state = WiimoteState {
                buttons:     raw.pad.buttons,
                stick_x:     raw.pad.stick_x,    stick_y:  raw.pad.stick_y,
                cstick_x:    raw.pad.cstick_x,   cstick_y: raw.pad.cstick_y,
                trigger_l:   raw.pad.trigger_l,  trigger_r: raw.pad.trigger_r,
                ext_buttons: raw.ext_buttons,
                extension:   raw.extension,
                wii_buttons: raw.wii_buttons,
                ir_x:        raw.ir_x,            ir_y:     raw.ir_y,
                accel_x:     raw.accel_x,
                accel_y:     raw.accel_y,
                accel_z:     raw.accel_z,
                connected:   raw.connected,
                _pad:        [0u8; 4],
            };
            let bytes = core::slice::from_raw_parts(
                &state as *const _ as *const u8, want);
            buf[..want].copy_from_slice(bytes);
            Ok(want)
        }

        #[cfg(feature = "fat")]
        VfsFile::FatFile { vol_idx, start, cur, size, pos, clus_pos } => {
            match &VOLUMES[*vol_idx].inner {
                VolumeInner::Fat(fv) => {
                    fv.raw_seek(*start, cur, pos, clus_pos, *size, target).map_err(Into::into)?;
                    Ok(*pos)
                }
                _ => Err(Error::Io),
            }
        }

        #[cfg(feature = "ext2")]
        VfsFile::Ext2File { size, pos, .. } => {
            if target > *size { return Err(Error::InvalidArg); }
            *pos = target; Ok(target)
        }

        #[cfg(feature = "iso9660")]
        VfsFile::IsoFile { size, pos, .. } => {
            if target > *size { return Err(Error::InvalidArg); }
            *pos = target; Ok(target)
        }

        #[cfg(feature = "dvd")]
        VfsFile::DvdFile { size, pos, .. } => {
            if target > *size { return Err(Error::InvalidArg); }
            *pos = target; Ok(target)
        }

        _ => Err(Error::Unsupported),
    }
}

// ─── Allocate / free file descriptors ────────────────────────────────────────

unsafe fn alloc_fd() -> Option<Fd> {
    for (i, slot) in FD_TABLE.iter().enumerate() {
        if slot.is_empty() { return Some(i as Fd); }
    }
    None
}

unsafe fn release_fd(fd: Fd) {
    let idx = fd as usize;
    if idx >= MAX_FD { return; }

    // Decrement the owning volume's open_count
    #[cfg(feature = "_fs")]
    if let Some(vi) = FD_TABLE[idx].vol_idx() {
        if VOLUMES[vi].open_count > 0 { VOLUMES[vi].open_count -= 1; }
    }

    FD_TABLE[idx] = VfsFile::Empty;
}

unsafe fn check_fd(fd: Fd) -> Result<&'static mut VfsFile> {
    let idx = fd as usize;
    if idx >= MAX_FD || FD_TABLE[idx].is_empty() { return Err(Error::InvalidFd); }
    Ok(&mut FD_TABLE[idx])
}


// ─── Internal helpers used by hid.rs ─────────────────────────────────────────

/// Open a HID port by index (0–3) for the given slot type.
/// Returns the Fd, or None if the table is full.
pub(crate) unsafe fn vfs_open_hid(port: u8, slot: HidSlot) -> Option<Fd> {
    let fd = alloc_fd()?;
    FD_TABLE[fd as usize] = match slot {
        HidSlot::Std   => VfsFile::Controller { port },
        HidSlot::Kbd   => VfsFile::Keyboard   { port },
        HidSlot::Mouse => VfsFile::Mouse       { port },
        HidSlot::Wii   => VfsFile::Wiimote     { port },
    };
    Some(fd)
}

/// Read bytes from an already-open Fd. Used by hid.rs to avoid
/// re-parsing the path on every poll().
pub(crate) unsafe fn do_read_fd(fd: Fd, buf: &mut [u8]) -> Result<usize> {
    do_read(check_fd(fd)?, buf)
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise the VFS.
///
/// Probes all hardware and populates the `/dev/` device tree. No filesystems
/// are mounted — that happens lazily on first access.
///
/// Call once at startup, before any other `vfs::*` function.
pub unsafe fn init() {
    init_with_opts(MountOptions::DEFAULT);
}

/// Like [`init`] but specifies the mount options used for lazy-mounts.
pub unsafe fn init_with_opts(opts: MountOptions) {
    DEFAULT_OPTS = opts;

    // ── Wii-native backends (IOS Bluetooth + USB) ──────────────
    #[cfg(feature = "wii")]
    unsafe { wii::init(); }

    // ── Fixed device nodes (HID + pseudo-devices — always present) ────────
    let fixed: &[(&str, DevKind, bool)] = &[
        ("/dev/hid/p1/std",   DevKind::HidPort(0), true),
        ("/dev/hid/p1/kbd",   DevKind::HidPort(0), true),
        ("/dev/hid/p1/mouse", DevKind::HidPort(0), true),
        ("/dev/hid/p1/wii",   DevKind::HidPort(0), true),
        ("/dev/hid/p2/std",   DevKind::HidPort(1), true),
        ("/dev/hid/p2/kbd",   DevKind::HidPort(1), true),
        ("/dev/hid/p2/mouse", DevKind::HidPort(1), true),
        ("/dev/hid/p2/wii",   DevKind::HidPort(1), true),
        ("/dev/hid/p3/std",   DevKind::HidPort(2), true),
        ("/dev/hid/p3/kbd",   DevKind::HidPort(2), true),
        ("/dev/hid/p3/mouse", DevKind::HidPort(2), true),
        ("/dev/hid/p3/wii",   DevKind::HidPort(2), true),
        ("/dev/hid/p4/std",   DevKind::HidPort(3), true),
        ("/dev/hid/p4/kbd",   DevKind::HidPort(3), true),
        ("/dev/hid/p4/mouse", DevKind::HidPort(3), true),
        ("/dev/hid/p4/wii",   DevKind::HidPort(3), true),
        ("/dev/null",          DevKind::Null,        true),
        ("/dev/zero",          DevKind::Zero,        true),
    ];

    let mut slot = 0usize;
    for &(path, kind, present) in fixed {
        if slot >= MAX_DEVICES { break; }
        DEVICES[slot] = DevNode::empty().set_path(path);
        DEVICES[slot].kind    = kind;
        DEVICES[slot].present = present;
        slot += 1;
    }

    // ── Storage nodes — probe hardware to determine presence ──────────────
    let storage: &[(&str, DevKind)] = &[
        ("/dev/sd/sp",  DevKind::SdSp2),
        ("/dev/sd/m1",  DevKind::SdSlotA),
        ("/dev/sd/m2",  DevKind::SdSlotB),
        ("/dev/dvd",    DevKind::DvdDrive),
        ("/dev/mem1",   DevKind::MemCardA),
        ("/dev/mem2",   DevKind::MemCardB),
    ];

    for &(path, kind) in storage {
        if slot >= MAX_DEVICES { break; }
        DEVICES[slot] = DevNode::empty().set_path(path);
        DEVICES[slot].kind    = kind;
        DEVICES[slot].present = probe_device(kind);
        slot += 1;
    }
}

/// Probe hardware for a single device kind. Returns `true` if present.
unsafe fn probe_device(kind: DevKind) -> bool {
    use dkdol_hal::sd::{SdCard, Slot as SdSlot};
    use dkdol_hal::exi;
    use dkdol_hal::memcard::CardSlot;

    match kind {
        DevKind::SdSp2   => { let mut c = SdCard::new(SdSlot::Sp2);  c.init().is_ok() }
        DevKind::SdSlotA => { let mut c = SdCard::new(SdSlot::A);    c.init().is_ok() }
        DevKind::SdSlotB => { let mut c = SdCard::new(SdSlot::B);    c.init().is_ok() }
        DevKind::DvdDrive => {
            dkdol_hal::dvd::init();
            !dkdol_hal::dvd::cover_open()
        }
        DevKind::MemCardA => dkdol_hal::memcard::MemCard::probe(CardSlot::A).is_ok(),
        DevKind::MemCardB => dkdol_hal::memcard::MemCard::probe(CardSlot::B).is_ok(),
        _ => true, // HID ports, null, zero are always "present"
    }
}

/// Refresh the device tree.
///
/// Re-probes storage hardware at rate-limited intervals and updates
/// `present` flags. Unmounts volumes whose hardware has been removed.
/// Call once per frame (or less) from the main loop.
pub unsafe fn poll() {
    let now = dkdol_rt::timer::get_ticks();

    // SD cards: check every 500 ms (≈ 20_250_000 ticks at 40.5 MHz)
    if now.wrapping_sub(LAST_POLL_SD) > 20_250_000 {
        LAST_POLL_SD = now;
        for node in DEVICES.iter_mut() {
            match node.kind {
                DevKind::SdSp2 | DevKind::SdSlotA | DevKind::SdSlotB => {
                    let was_present = node.present;
                    node.present = probe_device(node.kind);
                    if was_present && !node.present {
                        // Card removed — mark the volume as gone
                        #[cfg(feature = "_fs")]
                        mark_volume_removed(node.path_str());
                    }
                }
                _ => {}
            }
        }
    }

    // DVD: check every 1 s
    if now.wrapping_sub(LAST_POLL_DVD) > 40_500_000 {
        LAST_POLL_DVD = now;
        for node in DEVICES.iter_mut() {
            if matches!(node.kind, DevKind::DvdDrive) {
                let was_present = node.present;
                node.present = probe_device(node.kind);
                if was_present && !node.present {
                    #[cfg(feature = "_fs")]
                    mark_volume_removed(node.path_str());
                }
            }
        }
    }

    // Memory cards: check every 250 ms
    if now.wrapping_sub(LAST_POLL_MC) > 10_125_000 {
        LAST_POLL_MC = now;
        for node in DEVICES.iter_mut() {
            match node.kind {
                DevKind::MemCardA | DevKind::MemCardB => {
                    let was_present = node.present;
                    node.present = probe_device(node.kind);
                    if was_present && !node.present {
                        #[cfg(feature = "_fs")]
                        mark_volume_removed(node.path_str());
                    }
                }
                _ => {}
            }
        }
    }
}

/// Mark a volume as removed (device pulled while mounted).
///
/// If `open_count == 0`, frees the slot immediately.
/// If files are still open, the slot lingers as a "zombie" until all
/// descriptors are closed, after which it is freed on the next `close()`.
#[cfg(feature = "_fs")]
unsafe fn mark_volume_removed(dev_prefix: &str) {
    for v in VOLUMES.iter_mut() {
        if !v.is_empty() && v.prefix_str() == dev_prefix {
            if v.open_count == 0 {
                v.inner = VolumeInner::Empty;
            }
            // If open_count > 0, leave it; release_fd() will clean up later.
        }
    }
}

/// Override the default `MountOptions` used for lazy-mounts.
pub unsafe fn set_default_mount_opts(opts: MountOptions) {
    DEFAULT_OPTS = opts;
}

/// Open a file or device by path.
///
/// The path must start with `/dev/`. On the first call for any storage
/// device prefix (e.g. `/dev/sd/sp`), the filesystem is detected and
/// mounted automatically.
pub unsafe fn open(path: &str, flags: u32) -> Result<Fd> {
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    let kind = dev_kind_for(dev_prefix);

    // ── Pseudo-devices (always available) ─────────────────────────────────
    match kind {
        DevKind::Null => {
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::Null;
            return Ok(fd);
        }
        DevKind::Zero => {
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::Zero { pos: 0 };
            return Ok(fd);
        }
        DevKind::HidPort(port) => {
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::Controller { port };
            return Ok(fd);
        }
        DevKind::Empty => return Err(Error::NotFound),
        _ => {} // fall through to filesystem handling
    }

    // ── Storage devices ───────────────────────────────────────────────────
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }

    // Filesystem features not compiled in → error immediately
    #[cfg(not(feature = "_fs"))]
    return Err(Error::FsNotAvailable);

    #[cfg(feature = "_fs")]
    {
        if subpath.is_empty() {
            // Opening the device node itself — return as a raw block device handle.
            // (Future: return a block-device FD for dd-style access.)
            return Err(Error::WrongType);
        }

        // Lazy-mount if not yet mounted
        let vol_idx = match find_mounted(dev_prefix) {
            Some(i) => i,
            None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
        };

        open_from_volume(vol_idx, subpath, flags)
    }
}

/// Open a path that is known to live on volume `vol_idx`.
#[cfg(feature = "_fs")]
unsafe fn open_from_volume(vol_idx: usize, subpath: &str, flags: u32) -> Result<Fd> {
    let writable = flags & (O_WRONLY | O_RDWR) != 0;
    let create   = flags & O_CREAT  != 0;

    match &mut VOLUMES[vol_idx].inner {
        VolumeInner::Empty => Err(Error::NoDevice),

        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv) => {
            if create {
                let (dir, name) = dkdol_fs::fat::split_dir_name(subpath)
                    .ok_or(Error::InvalidArg)?;
                let f = fv.create(dir, name).map_err(Into::<Error>::into)?;
                let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
                FD_TABLE[fd as usize] = VfsFile::FatFile {
                    vol_idx, start: f.start, cur: f.start,
                    size: 0, pos: 0, clus_pos: 0,
                };
                VOLUMES[vol_idx].open_count += 1;
                Ok(fd)
            } else {
                let f = fv.open(subpath).map_err(Into::<Error>::into)?;
                let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
                FD_TABLE[fd as usize] = VfsFile::FatFile {
                    vol_idx, start: f.start, cur: f.start,
                    size: f.size(), pos: 0, clus_pos: 0,
                };
                VOLUMES[vol_idx].open_count += 1;
                Ok(fd)
            }
        }

        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev) => {
            if create {
                let ino = ev.create_file(subpath).map_err(Into::<Error>::into)?;
                let (flags, block_raw, blocks) =
                    ev.inode_raw_info(ino).map_err(Into::<Error>::into)?;
                let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
                FD_TABLE[fd as usize] = VfsFile::Ext2File {
                    vol_idx, ino, size: 0, flags, block_raw, blocks, pos: 0,
                };
                VOLUMES[vol_idx].open_count += 1;
                Ok(fd)
            } else {
                let f = ev.open(subpath).map_err(Into::<Error>::into)?;
                let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
                FD_TABLE[fd as usize] = VfsFile::Ext2File {
                    vol_idx, ino: f.ino,
                    size: f.size(), flags: f.flags(),
                    block_raw: *f.block_raw(), blocks: *f.blocks(), pos: 0,
                };
                VOLUMES[vol_idx].open_count += 1;
                Ok(fd)
            }
        }

        #[cfg(feature = "iso9660")]
        VolumeInner::Iso(iv) => {
            let (lba_start, size) =
                iv.open_raw(subpath).map_err(Into::<Error>::into)?;
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::IsoFile {
                vol_idx, lba_start, size, pos: 0,
            };
            VOLUMES[vol_idx].open_count += 1;
            Ok(fd)
        }

        #[cfg(feature = "dvd")]
        VolumeInner::GcDvd(dv) => {
            let (offset, size) =
                dv.open_raw(subpath).map_err(Into::<Error>::into)?;
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::DvdFile {
                vol_idx, offset, size, pos: 0,
            };
            VOLUMES[vol_idx].open_count += 1;
            Ok(fd)
        }

        #[cfg(feature = "memcard")]
        VolumeInner::MemCard(mc) => {
            let entry = mc.find_by_name(subpath).ok_or(Error::NotFound)?;
            let fd = alloc_fd().ok_or(Error::TooManyOpen)?;
            FD_TABLE[fd as usize] = VfsFile::McFile {
                vol_idx, entry, pos: 0,
            };
            VOLUMES[vol_idx].open_count += 1;
            Ok(fd)
        }
    }
}

/// Create a new file at `path` (implies `O_CREAT | O_WRONLY | O_TRUNC`).
pub unsafe fn create(path: &str) -> Result<Fd> {
    open(path, O_CREAT | O_WRONLY | O_TRUNC)
}

/// Read up to `buf.len()` bytes from `fd`. Returns bytes read.
pub unsafe fn read(fd: Fd, buf: &mut [u8]) -> Result<usize> {
    do_read(check_fd(fd)?, buf)
}

/// Write `buf` to `fd`. Returns bytes written.
pub unsafe fn write(fd: Fd, buf: &[u8]) -> Result<usize> {
    do_write(check_fd(fd)?, buf)
}

/// Seek `fd` to absolute byte position `pos`. Returns the new position.
pub unsafe fn seek(fd: Fd, pos: u64) -> Result<u64> {
    do_seek(check_fd(fd)?, pos)
}

/// Return the byte size of the file open at `fd`.
pub unsafe fn size(fd: Fd) -> Result<u64> {
    Ok(check_fd(fd)?.size())
}

/// Return the current byte position of `fd`.
pub unsafe fn tell(fd: Fd) -> Result<u64> {
    Ok(check_fd(fd)?.pos())
}

/// Close `fd`. Decrements the volume's open-file counter.
pub unsafe fn close(fd: Fd) {
    release_fd(fd);
}

/// Stat a path without opening it.
#[cfg(feature = "_fs")]
pub unsafe fn stat(path: &str) -> Result<dkdol_fs::Metadata> {
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    if subpath.is_empty() { return Err(Error::WrongType); }
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }
    let vol_idx = match find_mounted(dev_prefix) {
        Some(i) => i,
        None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
    };
    match &VOLUMES[vol_idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv)    => fv.stat(subpath).map_err(Into::into),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev)   => ev.stat(subpath).map_err(Into::into),
        #[cfg(feature = "iso9660")]
        VolumeInner::Iso(iv)    => iv.stat(subpath).map_err(Into::into),
        #[cfg(feature = "dvd")]
        VolumeInner::GcDvd(dv)  => dv.stat(subpath).map_err(Into::into),
        _ => Err(Error::Unsupported),
    }
}

/// Enumerate directory entries at `path`.
///
/// `cb` receives one [`dkdol_fs::Metadata`] per entry. Return `false` from the
/// callback to stop early.
#[cfg(feature = "_fs")]
pub unsafe fn read_dir<F>(path: &str, cb: F) -> Result<()>
where F: FnMut(&dkdol_fs::Metadata) -> bool
{
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }
    let vol_idx = match find_mounted(dev_prefix) {
        Some(i) => i,
        None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
    };
    let dir_path = if subpath.is_empty() { "/" } else { subpath };
    match &mut VOLUMES[vol_idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv)    => fv.read_dir(dir_path, cb).map_err(Into::into),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev)   => ev.read_dir(dir_path, cb).map_err(Into::into),
        #[cfg(feature = "iso9660")]
        VolumeInner::Iso(iv)    => iv.read_dir(dir_path, cb).map_err(Into::into),
        #[cfg(feature = "dvd")]
        VolumeInner::GcDvd(dv)  => dv.read_dir(dir_path, cb).map_err(Into::into),
        _ => Err(Error::Unsupported),
    }
}

/// Create a directory.
#[cfg(feature = "_fs")]
pub unsafe fn mkdir(path: &str) -> Result<()> {
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    if subpath.is_empty() { return Err(Error::InvalidArg); }
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }
    let vol_idx = match find_mounted(dev_prefix) {
        Some(i) => i,
        None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
    };
    match &mut VOLUMES[vol_idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv)  => fv.mkdir(subpath).map_err(Into::into),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev) => ev.mkdir(subpath).map_err(Into::into),
        _ => Err(Error::ReadOnly),
    }
}

/// Delete a file.
#[cfg(feature = "_fs")]
pub unsafe fn unlink(path: &str) -> Result<()> {
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    if subpath.is_empty() { return Err(Error::InvalidArg); }
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }
    let vol_idx = match find_mounted(dev_prefix) {
        Some(i) => i,
        None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
    };
    match &mut VOLUMES[vol_idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv)  => fv.unlink(subpath).map_err(Into::into),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev) => ev.unlink(subpath).map_err(Into::into),
        _ => Err(Error::ReadOnly),
    }
}

/// Delete an empty directory.
#[cfg(feature = "_fs")]
pub unsafe fn rmdir(path: &str) -> Result<()> {
    let (dev_prefix, subpath) = split_dev_path(path).ok_or(Error::NotFound)?;
    if subpath.is_empty() { return Err(Error::InvalidArg); }
    if !dev_is_present(dev_prefix) { return Err(Error::NoDevice); }
    let vol_idx = match find_mounted(dev_prefix) {
        Some(i) => i,
        None    => lazy_mount(dev_prefix, DEFAULT_OPTS)?,
    };
    match &mut VOLUMES[vol_idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat(fv)  => fv.rmdir(subpath).map_err(Into::into),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2(ev) => ev.rmdir(subpath).map_err(Into::into),
        _ => Err(Error::ReadOnly),
    }
}

/// Flush dirty metadata to disk (no-op for read-only filesystems).
#[cfg(feature = "_fs")]
pub unsafe fn sync(dev_prefix: &str) -> Result<()> {
    if let Some(vi) = find_mounted(dev_prefix) {
        match &mut VOLUMES[vi].inner {
            // FAT and EXT2 write-through — nothing extra needed
            _ => {}
        }
    }
    Ok(())
}

/// Forcibly unmount a device. Returns [`Error::Busy`] if files are open.
#[cfg(feature = "_fs")]
pub unsafe fn unmount(dev_prefix: &str) -> Result<()> {
    if let Some(vi) = find_mounted(dev_prefix) {
        if VOLUMES[vi].open_count > 0 { return Err(Error::Busy); }
        VOLUMES[vi].inner = VolumeInner::Empty;
    }
    Ok(())
}

// ─── Device tree query ────────────────────────────────────────────────────────

/// Return `true` if a device at `dev_prefix` is detected and present.
///
/// Example: `is_present("/dev/sd/sp")`.
pub unsafe fn is_present(dev_prefix: &str) -> bool {
    dev_is_present(dev_prefix)
}

/// Return `true` if the device at `dev_prefix` has a mounted filesystem.
pub unsafe fn is_mounted(dev_prefix: &str) -> bool {
    #[cfg(feature = "_fs")]
    { find_mounted(dev_prefix).is_some() }
    #[cfg(not(feature = "_fs"))]
    { let _ = dev_prefix; false }
}

/// Iterate over every node in the device tree.
///
/// `cb` receives `(path, present, mounted)` for each node.
/// Useful for displaying a device list on screen.
pub unsafe fn list_devices<F>(mut cb: F)
where F: FnMut(&str, bool, bool)
{
    for node in DEVICES.iter() {
        if node.kind == DevKind::Empty { continue; }
        let mounted = is_mounted(node.path_str());
        cb(node.path_str(), node.present, mounted);
    }
}

/// Poll one controller port directly (bypasses the VFS file path).
///
/// Equivalent to `dkdol_hal::si::read_pad` but exported from `dkdol-vfs` so
/// callers only need one dependency. The VFS `/dev/hid/pN/std` path is the
/// preferred approach for new code; this function exists for tight loops.
pub unsafe fn read_pad(port: Port) -> PadResult {
    dkdol_hal::si::read_pad(port)
}
