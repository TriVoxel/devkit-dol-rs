//! Serial Interface (SI) — GameCube controller ports.
//!
//! The SI bus connects the four controller ports (bottom of the console).
//! It handles GC pad, keyboard, steering wheel, bongos, and GBA link.
//!
//! **Status: Stub — see TODO.md**

// TODO (Milestone 2): Implement SI polling and controller state
// See crates/gc-hal/src/si/TODO.md

#![allow(dead_code, unused_variables)]

pub const SI_BASE: usize = 0xCC006400;

/// Controller port index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port { P1 = 0, P2 = 1, P3 = 2, P4 = 3 }
