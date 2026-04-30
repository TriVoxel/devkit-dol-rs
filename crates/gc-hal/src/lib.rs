//! # gc-hal — GameCube/Wii Hardware Abstraction Layer
//!
//! Safe, idiomatic Rust interfaces to the GameCube and Wii hardware subsystems.
//!
//! ## Subsystems
//!
//! | Module | Hardware       | Status       |
//! |--------|----------------|--------------|
//! | `vi`   | Video Interface| 🟡 WIP       |
//! | `gx`   | Graphics (GX)  | 🔴 Stub      |
//! | `si`   | Serial Interface (controllers) | 🔴 Stub |
//! | `exi`  | External Interface (memory card, SD) | 🔴 Stub |
//! | `dsp`  | Audio DSP      | 🔴 Stub      |
//! | `ai`   | Audio Interface | 🔴 Stub     |
//! | `dvd`  | DVD Drive      | 🔴 Stub      |
//! | `pi`   | Processor Interface (interrupts) | 🔴 Stub |

#![no_std]

pub mod vi;
pub mod gx;
pub mod si;
pub mod exi;
pub mod dsp;
pub mod ai;
pub mod dvd;
pub mod pi;
