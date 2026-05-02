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
//! | `storage` | Unified BlockDevice trait + scanner | ✅ All devices      |

#![no_std]
#![feature(asm_experimental_arch)]

pub mod ai;
pub mod dsp;
pub mod dvd;
pub mod exi;
pub mod gx;
pub mod memcard;
pub mod pi;
pub mod sd;
pub mod si;
pub mod storage;
pub mod vi;
