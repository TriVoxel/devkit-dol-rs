//! # gc-hal — GameCube/Wii Hardware Abstraction Layer
//!
//! Idiomatic Rust interfaces to GC/Wii hardware subsystems.
//!
//! | Module | Hardware                            | Status              |
//! |--------|-------------------------------------|---------------------|
//! | `vi`   | Video Interface                     | ✅ NTSC/PAL         |
//! | `pi`   | Processor Interface (interrupts)    | ✅ Complete         |
//! | `si`   | Serial Interface (controllers)      | ✅ Sync poll        |
//! | `gx`   | Graphics (GX FIFO)                  | ✅ 3D pipeline      |
//! | `ai`   | Audio Interface (DMA streaming)     | ✅ DMA + callback   |
//! | `dsp`  | Audio DSP (mailbox, control)        | ✅ Reset + mailbox  |
//! | `exi`  | External Interface                  | ✅ Imm + DMA        |
//! | `sd`   | SD card via EXI (SD Gecko)          | ✅ Read + write     |
//! | `dvd`  | DVD drive                           | ✅ Read + seek      |

#![no_std]
#![feature(asm_experimental_arch)]

pub mod ai;
pub mod dsp;
pub mod dvd;
pub mod exi;
pub mod gx;
pub mod pi;
pub mod sd;
pub mod si;
pub mod vi;
