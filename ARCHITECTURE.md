# DevKit DOL — I/O Architecture

## Overview

The I/O stack follows a strict three-layer design:

```
┌─────────────────────────────────────────────────────────────────────┐
│                       Application / Game Code                        │
│   vfs::open("/dev/sd/sp/save.bin", READ | WRITE)                    │
│   vfs::read(fd, &mut buf)                                           │
│   vfs::open("/dev/hid/p1/std", READ)   // controller as a file      │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│  dkdol-vfs  — virtual filesystem, device tree, file descriptor table   │
│                                                                      │
│  devtree::DevTree         /dev/ in-memory tree                      │
│  mount::MountTable        device path → FsInstance                  │
│  fdtable::FdTable         integer file descriptor → VfsFile         │
│  hid::HidFile             controller/keyboard state as char devices │
│  poll::DevPoll            periodic hotplug detection                │
└──────┬─────────────────────────────────┬────────────────────────────┘
       │                                 │
┌──────▼──────────────┐     ┌────────────▼───────────────────────────┐
│  dkdol-fs              │     │  dkdol-hal                                 │
│                     │     │                                         │
│  FAT32 / ExFAT      │     │  sd::SdCard   (EXI SPI driver)         │
│  EXT2 / 3 / 4       │     │  memcard::MemCard                       │
│  ISO 9660           │     │  dvd::DvdDisk                           │
│  GC DVD FST         │     │  si::PadState  (Serial Interface)       │
│  MemCard FS         │     │  exi / pi / vi / gx / ai / dsp         │
└─────────────────────┘     └────────────────────────────────────────┘
```

`dkdol-hal` drives the hardware. `dkdol-fs` parses filesystems on top of block
devices. `dkdol-vfs` builds the unified device tree, mounts filesystems, manages
file descriptors, and exposes HID peripherals as character device files.

---

## Device Tree — `/dev/` Namespace

The device tree lives entirely in static memory (no heap). Entries are
populated at `vfs::init()`, then refreshed by `vfs::poll()`.

```
/dev/
│
├── sd/                     # SD card media
│   ├── sp/                 # SD2SP2 — SD card via Serial Port 2 (EXI Ch2)
│   ├── m1/                 # SD Gecko in memory card slot A (EXI Ch0)
│   └── m2/                 # SD Gecko in memory card slot B (EXI Ch1)
│
├── dvd/                    # Real GameCube DVD drive (read-only)
│                           # Automatically detects FST (GC disc) or ISO9660
│
├── mem1                    # Genuine GC memory card, slot A — block device
├── mem2                    # Genuine GC memory card, slot B — block device
│                           # If a filesystem is mounted on top, files appear
│                           # at /dev/mem1/<filename>
│
├── net/                    # Future: network devices
│   └── <port>/<shortname>/ # e.g. /dev/net/eth0/ when a BBA is connected
│
├── hid/                    # Human Interface Devices (always present)
│   ├── p1/                 # Player 1 port
│   │   ├── std             # Standard GC/Wii controller
│   │   ├── kbd             # ASCII Keyboard
│   │   ├── mouse           # Mouse adapter
│   │   ├── bongo           # DK Bongos
│   │   └── ddr             # Dance pad
│   ├── p2/                 # Player 2–4 ports (same structure)
│   ├── p3/
│   └── p4/
│
├── null                    # Reads return 0 bytes; writes are discarded
└── zero                    # Reads return 0x00 bytes indefinitely
```

### Path routing rules

`vfs::open(path, flags)` splits the path at the device boundary:

| Path prefix       | Block device    | Default FS     |
|-------------------|-----------------|----------------|
| `/dev/sd/sp/…`    | `SdCard(Sp2)`   | FAT32 / EXT4   |
| `/dev/sd/m1/…`    | `SdCard(SlotA)` | FAT32 / EXT4   |
| `/dev/sd/m2/…`    | `SdCard(SlotB)` | FAT32 / EXT4   |
| `/dev/dvd/…`      | `DvdDisk`       | GC FST / ISO   |
| `/dev/mem1/…`     | `MemCard(SlotA)`| GC MemCard FS  |
| `/dev/mem2/…`     | `MemCard(SlotB)`| GC MemCard FS  |
| `/dev/hid/p1/std` | —               | char device    |
| `/dev/null`       | —               | char device    |

---

## Crate Responsibilities

### `dkdol-hal` — hardware drivers

Owns all register-level I/O. Exports:
- `sd::SdCard` — EXI SPI SD driver
- `memcard::MemCard` — EXI memory card driver
- `dvd::DvdDisk` — DVD drive
- `si::read_pad()` — controller polling
- `storage::scan()` — hotplug detection
- `storage::BlockDevice` trait — the canonical block I/O interface

No filesystem logic. No path strings.

### `dkdol-fs` — filesystem parsers

Owns all on-disk format knowledge. Each module is self-contained:

| Module     | Reads | Writes | Journal |
|------------|-------|--------|---------|
| `fat`      |  ✅   |   ✅   |   —     |
| `ext2`     |  ✅   |   ✅   |  JBD2   |
| `iso9660`  |  ✅   |   —    |   —     |
| `dvd`      |  ✅   |   —    |   —     |
| `memcard`  |  ✅   |   ✅   |   —     |

`dkdol-fs` does **not** maintain global state or mount tables. It provides
pure filesystem instances (`FatVolume<D>`, `Ext2<D>`, etc.) that operate
over any `BlockDev` implementation. The `vfs` module inside `dkdol-fs` is
kept for legacy compatibility and should be migrated to `dkdol-vfs`.

**The `BlockDev` trait** (defined in `dkdol-fs::lib`) accepts any type that
can `read_sector`/`write_sector`. `dkdol-vfs` bridges `dkdol-hal::BlockDevice`
to this trait via a newtype wrapper.

### `dkdol-vfs` — virtual filesystem and device tree (new crate)

The unified API. Owns:
- The static device tree
- The static mount table (`MAX_VOLUMES = 8` slots)
- The static file descriptor table (`MAX_FD = 32` slots)
- The `VfsFile` enum (dispatches to FAT/EXT2/ISO/MC/HID/Null)
- POSIX-like free functions: `open`, `read`, `write`, `seek`, `close`,
  `stat`, `readdir`, `mkdir`, `create`, `unlink`, `rmdir`, `ioctl`
- `init()` — boot-time device probe and auto-mount
- `poll()` — periodic hotplug refresh (call once per frame or less)
- `mount()` / `unmount()` — manual mount control

---

## Journaling Flag

`MountOptions` controls journaling per-mount:

```rust
pub struct MountOptions {
    /// Journaling mode for filesystems that support it (EXT3/4).
    pub journal:  JournalMode,
    /// Force read-only even on writable hardware.
    pub readonly: bool,
    /// Run fsck-style consistency check before mounting.
    /// Currently: refuse mount if journal needs replay (EXT3/4 dirty flag).
    pub check:    bool,
}

pub enum JournalMode {
    /// Enable journaling if the filesystem supports it; disable for EXT2/FAT.
    /// On EXT3/4: refuse to mount if journal needs replay. (Default)
    Auto,
    /// Like Auto but allow mounting a dirty EXT3/4 after replaying the journal.
    Replay,
    /// Treat all filesystems as if they have no journal (EXT2 semantics).
    /// Unsafe for EXT3/4 if the device loses power mid-write.
    Disable,
}
```

At mount time, `dkdol-vfs` calls `Ext2::mount_opts(dev, journal_mode)`.
FAT32 and ISO9660 ignore the flag entirely.

---

## File Descriptor Table

`MAX_FD = 32` slots in a static array. File descriptor 0, 1, 2 are
pre-assigned to `/dev/null`, `/dev/null`, and `/dev/null` at init; games
that want stdin/stdout/stderr can redirect them.

```rust
pub type Fd = u8;   // 0..31

// Returned by open() / create(). Passed to read(), write(), seek(), close().
```

Error on `open` when all slots are full: `FsError::TooManyOpen`.

---

## HID Devices as Files

Every port/accessory combination appears as a read-only character device.
The device node exists even when nothing is plugged in; reads then return a
zeroed state struct and `HidError::NoDevice` is set in the extended status.

### Read semantics

Reading `/dev/hid/p1/std` with a buffer of at least
`size_of::<ControllerState>()` bytes returns the current pad state
**at the time of the read**. The call is synchronous and polls the SI bus.

```rust
let mut state = ControllerState::default();
vfs::read(fd, bytemuck::bytes_of_mut(&mut state))?;
```

### Write semantics (rumble)

Writing 1 byte to `/dev/hid/p1/std`:
- `0x00` — stop rumble
- `0x01` — start rumble

### `ioctl` commands

```rust
pub enum IoctlCmd {
    /// Get device type present at this port (returns DeviceKind as u64)
    HidGetKind   = 0x1000,
    /// Set rumble motor: arg = 0 (off) or 1 (on)
    HidSetRumble = 0x1001,
    /// Flush any buffered key events (keyboard only)
    HidFlush     = 0x1002,
}
```

### Layout of `ControllerState`

```
Offset  Size  Field
  0       2   buttons (bitfield, same layout as dkdol-hal::si::Buttons)
  2       1   stick_x   (u8, center = 128)
  3       1   stick_y
  4       1   cstick_x
  5       1   cstick_y
  6       1   trigger_l
  7       1   trigger_r
  8       1   connected (0 = no controller, 1 = connected)
  9       7   reserved (zeroed)
 16  total
```

---

## Polling and Hotplug

`vfs::poll()` is designed to be called from the main game loop. It is
low-overhead: it only calls into hardware if enough time has elapsed since
the last check.

Default poll intervals:
- SD cards: 500 ms (EXI ID check, fast)
- Memory cards: 250 ms (cover sensor)
- DVD: 1000 ms (cover state register)
- HID: no polling needed; read on demand

On insert: auto-mounts with the same `MountOptions` used at `init()`.
On removal: marks the volume as unmounted; any open `Fd` pointing at it
will return `FsError::Io` on subsequent operations.

If a device is removed with open file descriptors (`open_count > 0`),
the mount entry is kept as a "zombie" until all FDs are closed, then
freed.

---

## Migration Guide

### Replacing `dkdol-fs::vfs` calls

Old (dkdol-fs VFS):
```rust
use dkdol_fs::vfs;
unsafe {
    vfs::mount_sd(SdSlot::Sp2, "sd", FsKind::Auto);
    let mut f = vfs::open("sd:/game.dol").unwrap();
    vfs::read(&mut f, &mut buf).unwrap();
}
```

New (dkdol-vfs):
```rust
use dkdol_vfs as vfs;
unsafe {
    vfs::init();                   // probes + auto-mounts everything
    let fd = vfs::open("/dev/sd/sp/game.dol", vfs::O_RDONLY).unwrap();
    vfs::read(fd, &mut buf).unwrap();
    vfs::close(fd).unwrap();
}
```

### Replacing `dkdol-hal::si` calls

Old:
```rust
match dkdol_hal::si::read_pad(Port::P1) {
    PadResult::Ok(pad) => { /* use pad */ }
    _ => {}
}
```

New (still works — `dkdol-hal::si` is unchanged):
```rust
// Filesystem-style alternative:
let fd = vfs::open("/dev/hid/p1/std", vfs::O_RDONLY).unwrap();
let mut state = ControllerState::default();
vfs::read(fd, bytemuck::bytes_of_mut(&mut state)).unwrap();
```

Both APIs coexist. `dkdol-hal::si` remains valid for tight loops. The VFS
path is preferred for tools, save managers, and anything that already
has a fd open.

---

## Workspace Changes

```toml
# Cargo.toml (root workspace)
members = [
    # existing ...
    "crates/dkdol-vfs",   # NEW
]
```

```toml
# crates/dkdol-vfs/Cargo.toml
[dependencies]
dkdol-hal = { path = "../dkdol-hal" }
dkdol-fs  = { path = "../dkdol-fs", features = ["fat", "ext2", "memcard", "dvd", "iso9660"] }
dkdol-rt  = { path = "../dkdol-rt" }
```

`dkdol-fs` remains in the workspace as a standalone crate for direct use.
`dkdol-vfs` is the recommended high-level entry point for new code.

---

## Design Constraints

- **`no_std` / `no_alloc`**: All tables are static arrays with fixed capacities.
  Capacity constants can be overridden via Cargo features.
- **`unsafe` at the boundary**: All public `vfs::*` functions are `unsafe`
  because they access `static mut` tables. Games are expected to call them
  from a single thread (no RTOS; bare metal).
- **No RTOS scheduler**: `poll()` is cooperative, not interrupt-driven.
  DVD cover-open detection uses the PI interrupt if available.
- **Stack depth**: `dkdol-fs` filesystem functions allocate up to ~12 KB of
  stack temporaries (extent tree traversal). Game code must ensure the
  stack is large enough when calling VFS functions (default `dkdol-rt` stack
  is 64 KB).
