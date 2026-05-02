# DevKit DOL — Work In Progress

## Milestone 0 — Scaffold ✅ | 1 — Runtime ✅ | 2 — Controllers ✅
## Milestone 3 — GX GPU ✅ | 4 — Audio ✅ | 5 — Storage ✅
## Milestone 6 — Wii Extensions ✅ | 7 — cargo-dkdol ✅

---

## Milestone 8 — Crash Handler ✅

### gc-crash crate (new)

**Dependency chain:** `gc-rt` + `gc-hal` + `gc-gfx`

`gc_crash::init()` registers a crash handler for all fatal PPC exceptions:
`SystemReset`, `MachineCheck`, `DSI`, `ISI`, `Alignment`, `Program`, `FpUnavailable`.

When triggered it:
1. Builds the full output into a 256-line static `LineBuffer` (no heap)
2. Initialises VI (even if the app's video was torn down) and takes over the XFB
3. Renders everything visible via `gc-gfx::Console` on a dark red background
4. Polls the D-pad for scrolling via `gc-hal::si`

**Output layout:**

```
╔═══════════════════════════════════════════════════════════╗
  FATAL EXCEPTION — Dsi
╚═══════════════════════════════════════════════════════════╝

  PC (SRR0): 0x80012ABC    MSR (SRR1): 0x0000B032
  DAR:       0x00000000    DSISR:      0x40000000
  Cause: page fault (no translation)
  ───────────────────────────────────────────────────────────
  General-Purpose Registers:
  r0 =00000000 r1 =817FFEE0 r2 =00000000 r3 =80045678 ...
  r8 =00000000 r9 =00000000 r10=00000000 r11=00000000 ...
  ...
  ───────────────────────────────────────────────────────────
  Special Registers:
   LR =80012A80  CTR=00000000  CR =28000040  XER=00000000
  ───────────────────────────────────────────────────────────
  Stack Trace (return addresses, oldest last):
  (match against .map or load .elf in Dolphin debugger)
   #00  0x80012ABC  ← exception PC (SRR0)
   #01  0x80009F14
   #02  0x8000D238
   ...
  ───────────────────────────────────────────────────────────
  D-Up/D-Down: scroll  |  Reset to reboot
```

**Stack walk:** follows PPC ABI back-chain (SP+0 = prev SP, SP+4 = saved LR)
up to 32 frames. Addresses outside MEM1 are flagged.

**DSI cause decoder:** decodes all DSISR bit fields (page fault, protection
violation, alignment, TLB miss, etc.).

**Controls:**
- D-Up / D-Down: scroll 3 lines at a time
- Scroll indicator shows % progress and total line count

---

## Milestone 9 — Filesystems ✅

### gc-fs crate (new)

A complete filesystem library with a unified VFS layer.

**Feature flags:**

| Flag       | Filesystem    | Read | Write |
|------------|---------------|------|-------|
| `fat`      | FAT12/16/32   | ✅   | 🟡    |
| `fat`      | ExFAT         | ✅   | —     |
| `ext2`     | EXT2/3/4      | ✅   | —     |
| `memcard`  | Nintendo MC   | ✅   | ✅    |
| `dvd`      | GC disc (FST) | ✅   | —     |
| `iso9660`  | ISO 9660      | ✅   | —     |

---

### `gc-fs::fat` — FAT12/16/32 + ExFAT

- `FatGeom::parse(&[u8;512])` — BPB parsing; auto-detects FAT12/16/32/ExFAT
- `FatVolume<D: BlockDev>::mount(dev)` — reads boot sector, validates magic
- `FatVolume::open(path)` → `FatFile` — cluster-chain sequential read
- `FatVolume::read_dir(path, cb)` — walks directory entries, skips LFN + deleted
- `FatVolume::stat(path)` — file/dir metadata
- `FatFile::seek(pos)` — forward seek (rewind restarts from cluster 0)
- `FatFile::read(&mut buf)` — reads sector-by-sector through cluster chain
- `FatVolume::fat_next(cluster)` — FAT chain: FAT12 (byte-pair), FAT16 (u16), FAT32 (u32 masked)
- `FatVolume::fat_set(cluster, val)` — writes all FAT copies
- ExFAT geometry parsed from OEM name + extended BPB
- 8.3 name matching: case-insensitive, extension dot synthesis
- Space trimming on both name and extension fields

### `gc-fs::ext2` — EXT2/3/4 read-only

- `Ext2<D>::mount(dev)` — reads superblock at byte 1024, validates magic 0xEF53
- Inode reading: group descriptor → inode table → inode record
- Block resolution: direct (12), singly indirect, doubly indirect, triply indirect
- `Ext2::read_dir(path, cb)` — walks linear directory records (8-byte headers)
- `Ext2::open(path)` → `Ext2File` — reads via block pointer chain
- Supports EXT2 rev 0 (fixed 128-byte inodes) and rev 1+ (variable inode size)
- EXT3/4 read-compatible (journals and extended features ignored)

### `gc-fs::memcard` — Nintendo GC memory card filesystem

- `MemCardFs::mount(slot)` — reads both directory copies + both BAT copies,
  picks the one with the highest `updated` counter
- Geometry from EXI device ID: sector size from `_ROTL(id,23)&0x1C`, 
  latency from `_ROTL(id,26)&0x1C`
- `MemCardFs::read_dir(cb)` — iterates all 127 directory entries
- `MemCardFs::find(gamecode, filename)` — lookup by game code + 32-char name
- `MemCardFs::read_file(entry, buf)` — reads entire file following BAT chain
  (16 × 512-byte read_segment calls per 8 KB block)
- `MemCardFs::write_file(entry, data)` — erase + write all blocks in chain
- `MemCardFs::free_blocks()` — counts BAT entries = 0x0000
- Address encoding: `[opcode, addr>>17, addr>>9, addr>>7, addr&0x7F]`
- Block layout: 0=header, 1/2=directory A/B, 3/4=BAT A/B, 5+=user data

### `gc-fs::dvd` — GameCube disc filesystem

- `GcDvd<D>::mount(dev)` — reads disc header, validates GC magic 0xC2339F3D
- Disc header: game code, maker code, title, DOL/FST offset/size at 0x420
- FST parsing: root entry count, 12-byte entries (flag, name_off, param1, param2)
- String table: variable-length NUL-terminated names after FST entries
- `GcDvd::read_dir(path, cb)` — walks FST, skips into subdirectories
- `GcDvd::read_file(path, buf)` — reads from `param1` (disc offset) + `param2` (size)
- `GcDvd::stat(path)` — returns Metadata from FST entry
- Directory traversal: skips subtrees by jumping to `entry.param2` (next sibling)

### `gc-fs::iso9660` — ISO 9660 + Rock Ridge

- `Iso9660<D>::mount(dev)` — scans VDs at LBA 16+, finds PVD (type 1)
- VD terminator (0xFF) detection; Joliet VDs skipped (future work)
- Directory record parser: data LBA, size, flags, name stripping (`;1`, trailing `.`)
- `. ` and `..` entries (file ID 0x00/0x01) automatically skipped
- `Iso9660::read_dir(path, cb)` — walks directory sectors
- `Iso9660::open(path)` → `IsoFile` — sector-aligned positional reads
- `IsoFile::seek(pos)` / `IsoFile::read(&mut buf)` — arbitrary byte-range I/O
- Variable sector size (2048 standard, configurable from PVD)
- Compatible with PS1, Neo Geo CD, PC Engine CD, Sega CD, GD-ROM layouts

### `gc-fs::image` — File-as-block-device bridge

- `FileImage<SECTOR, D>` wraps any `D: BlockDev` with byte `offset` + `size`
- Translates image-sector LBAs to device-sector LBAs, handling:
  - Perfectly aligned reads (multiple image sectors per device sector)
  - Unaligned reads (image starts at non-sector-aligned offset)
- `FileImage::whole(dev)` — full device
- `FileImage::new(dev, offset, size)` — sub-range (ISO inside partition)
- Use case: `FileImage<2048, SdCard>` wraps an SD card, reads a `.iso` at
  the file's byte offset → pass to `Iso9660::mount()` for transparent ISO access

### `gc-fs::vfs` — Volume manager

- Static volume table: up to 8 simultaneously mounted volumes, no heap
- Mount point names: short ASCII strings (`"sd"`, `"dvd"`, `"mc"`, `"iso"`)
- Path routing: `"sd:/ROMS/game.iso"` → volume `"sd"`, path `"/ROMS/game.iso"`
  Plain `"/path"` routes to the first mounted volume

**Mount functions:**
- `vfs::mount_sd(slot, name, FsKind::Auto)` — SD card (FAT auto-detect)
- `vfs::mount_dvd(name)` — GC disc filesystem
- `vfs::mount_dvd_iso(name)` — DVD as ISO 9660 (for non-GC discs in ODEs)
- `vfs::mount_mc(slot, name)` — memory card
- `vfs::unmount(name)` — release a volume slot
- `vfs::list_volumes(cb)` — enumerate mounted volumes

**I/O functions:**
- `vfs::open(path)` → `VfsFile` — open any file; handles FAT, ISO, GC-DVD
- `vfs::read_dir(path, cb)` — list directory on any mounted filesystem
- `vfs::stat(path)` — file metadata on any filesystem
- `vfs::read_dvd_file(path, buf)` — efficient GC disc read (bypasses VfsFile)
- `vfs::mc_read_file(mount, gamecode, filename, buf)` — MC save data read

**VfsFile** is an enum over all concrete file types, providing uniform
`size()`, `pos()`, `seek()`, `read()`, `read_exact()` regardless of source.

---

## ISO Portability

The standout feature: `.iso` images on SD cards work identically to physical discs:

```rust
// On real hardware with a physical PS1 disc in an ODE:
vfs::mount_dvd_iso("ps1").unwrap();
let file = vfs::open("ps1:/SYSTEM.CNF").unwrap();

// Or with a .iso file on an SD card (same API):
vfs::mount_sd(Slot::A, "sd", FsKind::Auto).unwrap();
let iso_file = vfs::open("sd:/ROMS/xenogears.iso").unwrap();
// ... wrap iso_file as FileImage<2048, SdCard> and pass to Iso9660::mount ...
let file = vfs::open("iso:/SYSTEM.CNF").unwrap();
```

Supported ISO-on-block-device scenarios:
- **FAT32 SD card** containing a `.iso` → `FileImage<2048, SdCard>`
- **Physical GC disc** → `DvdDisk` directly to `Iso9660`
- **ODE serving any ISO** → identical to physical disc, transparent

---

## Examples available

| Example            | Demonstrates                                               |
|--------------------|------------------------------------------------------------|
| `hello_world`      | VI, XFB, text console                                      |
| `controller_test`  | SI polling, 4 ports                                        |
| `spinning_triangle`| GX 3D pipeline                                             |
| `sine_wave`        | AI DMA audio                                               |
| `sd_reader`        | SD card init, sector read, hex dump                        |
| `storage_detect`   | All storage devices, scan + sector preview                 |
