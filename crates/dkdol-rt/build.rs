// dkdol-rt/build.rs
//
// This build script does two things:
//
// 1. Locates the workspace-level linker script (link/gcn.ld) using an absolute
//    path derived from CARGO_MANIFEST_DIR, then emits a `rustc-link-arg` so that
//    any binary depending on `dkdol-rt` gets the correct linker script.
//
// 2. Tells Cargo to re-run this script if the linker script changes.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set — is this running under cargo?");

    // dkdol-rt lives at  <workspace>/crates/dkdol-rt/
    // workspace root is two levels up.
    let workspace_root = PathBuf::from(&manifest_dir)
        .parent()   // crates/
        .expect("dkdol-rt has no parent directory?")
        .parent()   // workspace root
        .expect("crates/ has no parent directory?")
        .to_path_buf();

    let linker_script = workspace_root.join("link").join("gcn.ld");

    println!(
        "cargo:rerun-if-changed={}",
        linker_script.display()
    );

    // Pass the linker script to rust-lld via the -T flag.
    // This is propagated to any binary that has dkdol-rt as a (transitive) dependency.
    println!(
        "cargo:rustc-link-arg=-T{}",
        linker_script.display()
    );

    // Ensure we re-run if this build script itself changes.
    println!("cargo:rerun-if-changed=build.rs");
}
