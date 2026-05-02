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
//! | `exi`  | External Interface (memcard, SD)    | ✅ Imm + DMA        |
//! | `dvd`  | DVD Drive                           | 🔴 Stub             |

#![no_std]
#![feature(asm_experimental_arch)]

pub mod ai;
pub mod dsp;
pub mod dvd;
pub mod exi;
pub mod gx;
pub mod pi;
pub mod si;
pub mod vi;
