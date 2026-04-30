//! # cargo-gc
//!
//! A Cargo subcommand that wraps the full GameCube build pipeline:
//!
//! ```text
//! cargo gc build [--release] [-p PACKAGE] [--example EXAMPLE]
//!     → cargo build (cross-compile for powerpc-gekko-eabi)
//!     → elf2dol (convert ELF → DOL)
//!
//! cargo gc run [--dolphin PATH] [--net IP]
//!     → cargo gc build
//!     → launch Dolphin with the DOL, OR push via wiiload protocol
//! ```
//!
//! ## Installation
//!
//! ```text
//! cargo install --path tools/cargo-gc
//! ```
//!
//! Then invoke as `cargo gc <subcommand>`.
//!
//! **Status: Stub — Milestone 8**

// TODO (Milestone 8): Implement cargo-gc subcommand
//
// Planned implementation:
//
// 1. Parse argv (cargo passes "gc" as argv[1] when invoked as `cargo gc`)
// 2. Forward `build` to `cargo build` with:
//    - `--target targets/powerpc-gekko-eabi.json`
//    - `-Z build-std=core,compiler_builtins`
//    - `-Z build-std-features=compiler-builtins-mem`
// 3. After build, run `elf2dol` on the output ELF
// 4. For `run --dolphin`, launch `dolphin-emu -e <output.dol>`
// 5. For `run --net`, send DOL over wiiload UDP protocol

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // When invoked as `cargo gc`, Cargo passes "gc" as the first argument.
    let subcommand_idx = if args.get(1).map(|s| s.as_str()) == Some("gc") { 2 } else { 1 };
    let subcmd = args.get(subcommand_idx).map(|s| s.as_str());

    match subcmd {
        Some("build") => {
            eprintln!("cargo-gc: 'build' subcommand is not yet implemented (Milestone 8).");
            eprintln!("Use the manual build steps from README.md in the meantime.");
            std::process::exit(1);
        }
        Some("run") => {
            eprintln!("cargo-gc: 'run' subcommand is not yet implemented (Milestone 8).");
            std::process::exit(1);
        }
        Some("help") | None => {
            println!("cargo-gc — GameCube build tool (devkit-dol-rs)");
            println!();
            println!("USAGE:");
            println!("    cargo gc <SUBCOMMAND> [OPTIONS]");
            println!();
            println!("SUBCOMMANDS:");
            println!("    build   Cross-compile and convert to DOL");
            println!("    run     Build and launch in Dolphin (or push over network)");
            println!("    help    Print this message");
            println!();
            println!("STATUS: Milestone 8 — not yet implemented. See WIP.md.");
        }
        Some(other) => {
            eprintln!("cargo-gc: unknown subcommand '{}'", other);
            eprintln!("Run 'cargo gc help' for usage.");
            std::process::exit(1);
        }
    }
}
