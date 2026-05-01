//! # gc-hal — GameCube/Wii Hardware Abstraction Layer
//!
//! Idiomatic Rust interfaces to GC/Wii hardware subsystems.
//!
//! | Module | Hardware                            | Status        |
//! |--------|-------------------------------------|---------------|
//! | `vi`   | Video Interface                     | ✅ NTSC/PAL   |
//! | `pi`   | Processor Interface (interrupts)    | ✅ Complete   |
//! | `si`   | Serial Interface (controllers)      | ✅ Sync poll  |
//! | `gx`   | Graphics (GX FIFO)                  | 🔴 Stub       |
//! | `exi`  | External Interface (memcard, SD)    | 🔴 Stub       |
//! | `dsp`  | Audio DSP                           | 🔴 Stub       |
//! | `ai`   | Audio Interface (streaming)         | 🔴 Stub       |
//! | `dvd`  | DVD Drive                           | 🔴 Stub       |

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
