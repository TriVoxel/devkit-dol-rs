//! # gc-hal — GameCube/Wii Hardware Abstraction Layer
//!
//! | Module    | Hardware                            | Status              |
//! |-----------|-------------------------------------|---------------------|
//! | `vi`      | Video Interface                     | ✅ NTSC/PAL         |
//! | `pi`      | Processor Interface (interrupts)    | ✅ Complete         |
//! | `si`      | Serial Interface (controllers)      | ✅ Sync poll        |
//! | `gx`      | Graphics (GX FIFO)                  | ✅ 3D pipeline      |
//! | `ai`      | Audio Interface (DMA streaming)     | ✅ DMA + callback   |
//! | `dsp`     | Audio DSP (mailbox, control)        | ✅ Reset + mailbox  |
//! | `exi`     | External Interface + device ID      | ✅ Imm + DMA        |
//! | `sd`      | SD card (Slot A, B, SP2)            | ✅ Read + write     |
//! | `memcard` | GC Memory Card (Slot A, B)          | ✅ Read + write     |
//! | `dvd`     | DVD drive                           | ✅ Read + seek      |
//! | `storage` | Unified `BlockDevice` + scanner     | ✅ All devices      |
//! | `mmio`    | MMIO base address (GC/Wii)          | ✅ Feature flag     |
//! | `mem2`    | Wii MEM2 extended RAM constants     | ✅ `wii` feature    |
//!
//! ## Features
//!
//! - `wii`: Switch MMIO prefix from `0xCC` to `0xCD` and expose MEM2 constants.
//!   Activate with `cargo dkdol build --wii` or add to your `Cargo.toml`:
//!   ```toml
//!   [dependencies.gc-hal]
//!   features = ["wii"]
//!   ```

#![no_std]
#![feature(asm_experimental_arch)]

pub mod ai;
pub mod dsp;
pub mod dvd;
pub mod exi;
pub mod gx;
pub mod mem2;
pub mod memcard;
pub mod mmio;
pub mod pi;
pub mod sd;
pub mod si;
pub mod storage;
pub mod vi;
