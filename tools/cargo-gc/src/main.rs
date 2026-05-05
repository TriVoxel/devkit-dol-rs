//! # cargo-gc — GameCube / Wii build tool
//!
//! A Cargo subcommand that wraps the full GameCube/Wii build pipeline.
//!
//! ## Installation
//!
//! ```sh
//! cargo install --path tools/cargo-gc
//! ```
//!
//! ## Usage
//!
//! ```text
//! cargo gc build [--release] [-p <pkg>] [--example <name>] [--wii]
//! cargo gc dol   <elf> [output.dol]
//! cargo gc run   [--release] [-p <pkg>] [--example <name>] [--dolphin <path>] [--wii]
//! cargo gc new   <project-name>
//! cargo gc help  [<subcommand>]
//! ```
//!
//! ## Project config (`Cargo.toml`)
//!
//! ```toml
//! [package.metadata.gc]
//! dolphin     = "dolphin-emu"
//! target_gc   = "targets/powerpc-gekko-eabi.json"
//! target_wii  = "targets/powerpc-broadway-eabi.json"
//! dol_out_dir = "."
//! ```

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitStatus};

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let all_args: Vec<String> = env::args().collect();

    // When invoked as `cargo gc <subcmd>`, Cargo inserts "gc" at index 1.
    let rest = if all_args.get(1).map(|s| s.as_str()) == Some("gc") {
        &all_args[2..]
    } else {
        &all_args[1..]
    };

    let subcmd = rest.first().map(|s| s.as_str()).unwrap_or("help");
    let result = match subcmd {
        "build"           => cmd_build(&rest[1..]),
        "dol"             => cmd_dol(&rest[1..]),
        "run"             => cmd_run(&rest[1..]),
        "new"             => cmd_new(&rest[1..]),
        "help"|"--help"|"-h" => { cmd_help(rest.get(1).map(|s| s.as_str())); Ok(()) }
        other => {
            eprintln!("cargo-gc: unknown subcommand '{}'", other);
            eprintln!("Run 'cargo gc help' for usage.");
            Err(1i32)
        }
    };
    if let Err(code) = result { process::exit(code); }
}

// ── Config ────────────────────────────────────────────────────────────────────

struct Cfg {
    dolphin:    String,
    target_gc:  String,
    target_wii: String,
    dol_out:    String,
}
impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            dolphin:    "dolphin-emu".into(),
            target_gc:  "targets/powerpc-gekko-eabi.json".into(),
            target_wii: "targets/powerpc-broadway-eabi.json".into(),
            dol_out:    ".".into(),
        }
    }
}

fn load_cfg() -> Cfg {
    let mut cfg = Cfg::default();
    let toml = match find_cargo_toml().and_then(|p| fs::read_to_string(p).ok()) {
        Some(t) => t,
        None    => return cfg,
    };
    let mut in_section = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') { in_section = t == "[package.metadata.gc]"; continue; }
        if !in_section { continue; }
        if let Some((k, v)) = t.split_once('=') {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            match k.trim() {
                "dolphin"     => cfg.dolphin    = v.into(),
                "target_gc"   => cfg.target_gc  = v.into(),
                "target_wii"  => cfg.target_wii = v.into(),
                "dol_out_dir" => cfg.dol_out    = v.into(),
                _ => {}
            }
        }
    }
    cfg
}

fn find_cargo_toml() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let p = dir.join("Cargo.toml");
        if p.exists() { return Some(p); }
        if !dir.pop() { return None; }
    }
}

fn ws_root() -> PathBuf {
    find_cargo_toml()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| env::current_dir().unwrap())
}

// ── Build arg parsing ─────────────────────────────────────────────────────────

struct BuildArgs {
    release:  bool,
    package:  Option<String>,
    example:  Option<String>,
    wii:      bool,
    extra:    Vec<String>,
}
impl Default for BuildArgs { fn default() -> Self { BuildArgs { release:false, package:None, example:None, wii:false, extra:vec![] } } }

fn parse_build(args: &[String]) -> BuildArgs {
    let mut b = BuildArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--release"|"-r" => b.release = true,
            "--wii"          => b.wii = true,
            "-p"|"--package" => { i+=1; b.package = args.get(i).cloned(); }
            "--example"      => { i+=1; b.example = args.get(i).cloned(); }
            s                => b.extra.push(s.into()),
        }
        i += 1;
    }
    b
}

// ── `cargo gc build` ──────────────────────────────────────────────────────────

fn cmd_build(args: &[String]) -> Result<(), i32> {
    let cfg  = load_cfg();
    let b    = parse_build(args);
    let root = ws_root();
    let tgt  = if b.wii { &cfg.target_wii } else { &cfg.target_gc };
    let prof = if b.release { "release" } else { "debug" };

    say("Building", &format!("{} ({})", target_label(b.wii), prof));

    // 1. Cross-compile
    let st = run_cargo_build(&root, tgt, &b)?;
    if !st.success() { eprintln!("cargo-gc: build failed"); return Err(2); }

    // 2. Locate ELF
    let elf = locate_elf(&root, tgt, prof, &b)?;
    say("Compiled", &strip(&root, &elf));

    // 3. ELF → DOL
    let dol = mk_dol_path(&cfg, &root, &elf);
    run_elf2dol(&root, &elf, &dol)?;
    say("Created", &strip(&root, &dol));
    Ok(())
}

fn run_cargo_build(root: &Path, tgt: &str, b: &BuildArgs) -> Result<ExitStatus, i32> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
       .arg("+nightly").arg("build")
       .arg("-Z").arg("build-std=core,compiler_builtins")
       .arg("-Z").arg("build-std-features=compiler-builtins-mem")
       .arg("--target").arg(tgt);
    if b.release { cmd.arg("--release"); }
    if let Some(p) = &b.package { cmd.arg("-p").arg(p); }
    if let Some(e) = &b.example { cmd.arg("--example").arg(e); }
    for x in &b.extra { cmd.arg(x); }
    cmd.status().map_err(|e| { eprintln!("cargo-gc: cannot run cargo: {}", e); 2 })
}

fn locate_elf(root: &Path, tgt: &str, prof: &str, b: &BuildArgs) -> Result<PathBuf, i32> {
    let tgt_dir = Path::new(tgt).file_stem()
        .and_then(|s| s.to_str()).unwrap_or(tgt);
    let base = root.join("target").join(tgt_dir).join(prof);

    if let Some(ex) = &b.example {
        let p = base.join("examples").join(ex);
        if p.exists() && is_elf(&p) { return Ok(p); }
    }
    if let Some(pkg) = &b.package {
        let bin = pkg.replace('-', "_");
        let p = base.join(&bin);
        if p.exists() && is_elf(&p) { return Ok(p); }
    }
    newest_elf(&base).ok_or_else(|| {
        eprintln!("cargo-gc: can't find ELF in {}", base.display());
        eprintln!("  Hint: use -p <package> or --example <name>");
        4i32
    })
}

fn newest_elf(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if !p.is_file() { continue; }
        match p.extension().and_then(|x| x.to_str()) {
            Some("d"|"rlib"|"rmeta"|"a"|"so"|"dol") => continue,
            _ => {}
        }
        if !is_elf(&p) { continue; }
        let t = e.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = t {
            if best.as_ref().map(|(_, bt)| t > *bt).unwrap_or(true) {
                best = Some((p, t));
            }
        }
    }
    best.map(|(p,_)| p)
}

fn is_elf(p: &Path) -> bool {
    let mut f = match fs::File::open(p) { Ok(f) => f, Err(_) => return false };
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok();
    magic == [0x7F, b'E', b'L', b'F']
}

fn mk_dol_path(cfg: &Cfg, root: &Path, elf: &Path) -> PathBuf {
    let name = format!("{}.dol", elf.file_name().unwrap_or_default().to_string_lossy());
    root.join(&cfg.dol_out).join(name)
}

fn run_elf2dol(root: &Path, elf: &Path, dol: &Path) -> Result<(), i32> {
    let st = Command::new("cargo")
        .current_dir(root)
        .args(["run", "--quiet", "-p", "elf2dol", "--"])
        .arg(elf).arg(dol)
        .status()
        .map_err(|e| { eprintln!("cargo-gc: elf2dol failed: {}", e); 3i32 })?;
    if !st.success() { eprintln!("cargo-gc: elf2dol failed"); return Err(3); }
    Ok(())
}

// ── `cargo gc dol` ────────────────────────────────────────────────────────────

fn cmd_dol(args: &[String]) -> Result<(), i32> {
    let elf = args.first().ok_or_else(|| {
        eprintln!("Usage: cargo gc dol <elf> [output.dol]");
        1i32
    })?;
    let elf = PathBuf::from(elf);
    if !elf.exists() { eprintln!("cargo-gc: not found: {}", elf.display()); return Err(4); }
    let dol = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
        let stem = elf.file_stem().unwrap_or_default();
        elf.with_file_name(format!("{}.dol", stem.to_string_lossy()))
    });
    say("Converting", &format!("{} → {}", elf.display(), dol.display()));
    run_elf2dol(&ws_root(), &elf, &dol)?;
    say("Created", &dol.display().to_string());
    Ok(())
}

// ── `cargo gc run` ────────────────────────────────────────────────────────────

fn cmd_run(args: &[String]) -> Result<(), i32> {
    let cfg = load_cfg();

    // Extract --dolphin before forwarding to build
    let mut dolphin_ov: Option<String> = None;
    let mut fwd: Vec<String> = Vec::new();
    let mut i = 0;
    let av: Vec<String> = args.to_vec();
    while i < av.len() {
        if av[i] == "--dolphin" { i += 1; dolphin_ov = av.get(i).cloned(); }
        else { fwd.push(av[i].clone()); }
        i += 1;
    }

    cmd_build(&fwd)?;

    let b    = parse_build(&fwd);
    let tgt  = if b.wii { &cfg.target_wii } else { &cfg.target_gc };
    let prof = if b.release { "release" } else { "debug" };
    let root = ws_root();
    let elf  = locate_elf(&root, tgt, prof, &b)?;
    let dol  = mk_dol_path(&cfg, &root, &elf);

    if !dol.exists() { eprintln!("cargo-gc: DOL not found: {}", dol.display()); return Err(4); }

    let dolphin = dolphin_ov.unwrap_or(cfg.dolphin);
    say("Launching", &format!("{} {}", dolphin, dol.display()));

    Command::new(&dolphin)
        .arg("-e").arg(&dol)
        .status()
        .map_err(|e| {
            eprintln!("cargo-gc: cannot launch '{}': {}", dolphin, e);
            eprintln!("  Set dolphin path in [package.metadata.gc] dolphin = \"...\"");
            4i32
        })?;
    Ok(())
}

// ── `cargo gc new` ────────────────────────────────────────────────────────────

fn cmd_new(args: &[String]) -> Result<(), i32> {
    let name = args.first().ok_or_else(|| {
        eprintln!("Usage: cargo gc new <project-name>");
        1i32
    })?;
    let dir = PathBuf::from(name);
    if dir.exists() { eprintln!("cargo-gc: '{}' already exists", name); return Err(5); }
    scaffold(&dir, name)?;
    println!();
    say("Created", &format!("GameCube project '{}'", name));
    println!("  cd {name}");
    println!("  cargo gc build --release --example hello");
    println!("  cargo gc run   --example hello");
    println!();
    Ok(())
}

fn scaffold(dir: &Path, name: &str) -> Result<(), i32> {
    let w = |p: &Path, s: &str| -> Result<(), i32> {
        fs::create_dir_all(p.parent().unwrap()).map_err(|_| 5)?;
        fs::write(p, s).map_err(|_| 5)
    };

    w(&dir.join("Cargo.toml"), &format!(
r#"[package]
name    = "{name}"
version = "0.1.0"
edition = "2021"

# Replace path deps with git/version once published:
# dkdol-rt = "0.1"
[dependencies]
dkdol-rt  = {{ path = "vendor/devkit-dol-rs/crates/dkdol-rt"  }}
dkdol-hal = {{ path = "vendor/devkit-dol-rs/crates/dkdol-hal" }}
dkdol-gfx = {{ path = "vendor/devkit-dol-rs/crates/dkdol-gfx" }}

[[example]]
name = "hello"

[profile.release]
opt-level     = 2
lto           = true
panic         = "abort"
codegen-units = 1

[profile.dev]
opt-level = 1
panic     = "abort"

[package.metadata.gc]
dolphin     = "dolphin-emu"
dol_out_dir = "."
"#))?;

    w(&dir.join(".cargo").join("config.toml"), r#"[build]
target = "targets/powerpc-gekko-eabi.json"

[target.powerpc-gekko-eabi]
rustflags = ["-C", "link-arg=--nmagic"]
"#)?;

    w(&dir.join("rust-toolchain.toml"), r#"[toolchain]
channel    = "nightly"
components = ["rust-src"]
"#)?;

    // Hello world example
    w(&dir.join("examples").join("hello.rs"), &format!(
r#"//! Hello World — {name}
#![no_std]
#![no_main]

use dkdol_gfx::{{Console, Xfb, YcbcrPair}};
use dkdol_hal::vi;
use core::fmt::Write;

const W: u32 = 640;
const H: u32 = 480;

#[repr(C, align(32))]
struct Fb([u32; (W * H / 2) as usize]);
static mut FB: Fb = Fb([0; (W * H / 2) as usize]);

#[no_mangle]
pub extern "C" fn main() -> ! {{
    unsafe {{
        vi::init_ntsc_480i();
        let ptr = FB.0.as_mut_ptr();
        let bg  = YcbcrPair::new(16, 128, 16, 128);
        let mut xfb = Xfb::from_raw(ptr, W, H);
        xfb.clear(bg);
        let mut con = Console::new(&mut xfb);
        con.set_bg(bg);
        con.set_fg(YcbcrPair::WHITE);
        let _ = write!(con, "\n  Hello from {name}!\n  Built with devkit-dol-rs.");
        con.flush();
        vi::set_framebuffer(ptr, W * 2);
        vi::flush();
        loop {{}}
    }}
}}
"#))?;

    w(&dir.join("targets").join("powerpc-gekko-eabi.json"), GC_TARGET_JSON)?;
    w(&dir.join("targets").join("powerpc-broadway-eabi.json"), WII_TARGET_JSON)?;
    w(&dir.join("link").join("gcn.ld"), GCN_LD)?;
    w(&dir.join(".gitignore"), "/target\n*.dol\n")?;
    w(&dir.join("README.md"), &format!(
r#"# {name}

A GameCube homebrew project built with [devkit-dol-rs](https://github.com/your-org/devkit-dol-rs).

## Build

```sh
cargo gc build --release --example hello
cargo gc run   --example hello
```

Requires `cargo-gc` to be installed: `cargo install --path vendor/devkit-dol-rs/tools/cargo-gc`
"#))?;

    Ok(())
}

// ── Help ──────────────────────────────────────────────────────────────────────

fn cmd_help(sub: Option<&str>) {
    println!("{}", match sub {
        Some("build") => HELP_BUILD,
        Some("dol")   => HELP_DOL,
        Some("run")   => HELP_RUN,
        Some("new")   => HELP_NEW,
        _             => HELP_MAIN,
    });
}

const HELP_MAIN: &str = "\
cargo-gc — GameCube/Wii build tool  (devkit-dol-rs)

USAGE:
    cargo gc <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    build    Cross-compile and convert ELF → DOL
    dol      Convert an existing ELF binary to DOL
    run      Build, convert, then launch in Dolphin
    new      Scaffold a new GC homebrew project
    help     Show this message or help for a subcommand

GLOBAL FLAGS:
    --wii    Target Wii (Broadway 729 MHz) instead of GameCube (Gekko 486 MHz)

PROJECT CONFIG (add to Cargo.toml):
    [package.metadata.gc]
    dolphin     = \"dolphin-emu\"    # Dolphin executable
    dol_out_dir = \".\"              # Where to write .dol files

EXAMPLES:
    cargo gc build --release --example spinning_triangle
    cargo gc run   --example hello_world
    cargo gc dol   target/powerpc-gekko-eabi/release/my_game
    cargo gc new   my_game
";

const HELP_BUILD: &str = "\
cargo gc build — Cross-compile and convert ELF → DOL

USAGE:
    cargo gc build [OPTIONS] [-- CARGO_ARGS...]

OPTIONS:
    --release           Optimized build
    -p, --package <P>   Build package P
    --example <E>       Build example E
    --wii               Target Wii (powerpc-broadway-eabi)

Runs:
    cargo +nightly build \\
        -Z build-std=core,compiler_builtins \\
        -Z build-std-features=compiler-builtins-mem \\
        --target targets/powerpc-gekko-eabi.json \\
        [--release] [-p P] [--example E]

Then converts the ELF output to DOL using the bundled elf2dol tool.
";

const HELP_DOL: &str = "\
cargo gc dol — Convert an ELF binary to GameCube DOL format

USAGE:
    cargo gc dol <elf> [output.dol]
";

const HELP_RUN: &str = "\
cargo gc run — Build, convert, then launch in Dolphin Emulator

USAGE:
    cargo gc run [BUILD_OPTIONS] [--dolphin <path>]

OPTIONS:
    Same as 'build', plus:
    --dolphin <path>    Override Dolphin executable path

Dolphin path resolution order:
    1. --dolphin flag
    2. [package.metadata.gc] dolphin in Cargo.toml
    3. \"dolphin-emu\" on $PATH
";

const HELP_NEW: &str = "\
cargo gc new — Scaffold a new GameCube homebrew project

USAGE:
    cargo gc new <name>

Creates <name>/ with:
    Cargo.toml              dkdol-rt/dkdol-hal/dkdol-gfx deps, metadata.gc section
    examples/hello.rs       Hello World
    .cargo/config.toml      target + rustflags
    rust-toolchain.toml     nightly + rust-src
    targets/*.json          GC and Wii target specs
    link/gcn.ld             Linker script
    README.md
    .gitignore
";

// ── Helpers ───────────────────────────────────────────────────────────────────

fn target_label(wii: bool) -> &'static str {
    if wii { "Wii (Broadway)" } else { "GameCube (Gekko)" }
}

fn say(verb: &str, msg: &str) {
    // Green bold verb like cargo's output style
    let green = env::var("NO_COLOR").is_err() && env::var("TERM").is_ok();
    if green {
        eprintln!("\x1b[1;32m{:>12}\x1b[0m {}", verb, msg);
    } else {
        eprintln!("{:>12} {}", verb, msg);
    }
}

fn strip(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

// ── Embedded target specs and linker script (used by `cargo gc new`) ─────────

const GC_TARGET_JSON: &str = r#"{
    "arch": "powerpc",
    "cpu": "750",
    "features": "+hard-float,+fprnd,+fsqrt",
    "data-layout": "E-m:e-p:32:32-Fn32-i64:64-n32",
    "llvm-target": "powerpc-unknown-none",
    "target-endian": "big",
    "target-pointer-width": "32",
    "target-c-int-width": "32",
    "os": "none",
    "env": "eabi",
    "vendor": "nintendo",
    "linker-flavor": "ld.lld",
    "linker": "rust-lld",
    "panic-strategy": "abort",
    "executables": true,
    "relocation-model": "static",
    "disable-redzone": true,
    "frame-pointer": "always",
    "target-family": [],
    "pre-link-args": { "ld.lld": ["-Tlink/gcn.ld"] },
    "supported-sanitizers": []
}
"#;

const WII_TARGET_JSON: &str = r#"{
    "arch": "powerpc",
    "cpu": "750",
    "features": "+hard-float,+fprnd,+fsqrt",
    "data-layout": "E-m:e-p:32:32-Fn32-i64:64-n32",
    "llvm-target": "powerpc-unknown-none",
    "target-endian": "big",
    "target-pointer-width": "32",
    "target-c-int-width": "32",
    "os": "none",
    "env": "eabi",
    "vendor": "nintendo-wii",
    "linker-flavor": "ld.lld",
    "linker": "rust-lld",
    "panic-strategy": "abort",
    "executables": true,
    "relocation-model": "static",
    "disable-redzone": true,
    "frame-pointer": "always",
    "target-family": [],
    "pre-link-args": { "ld.lld": ["-Tlink/gcn.ld"] },
    "supported-sanitizers": []
}
"#;

const GCN_LD: &str = r#"OUTPUT_FORMAT("elf32-powerpc","elf32-powerpc","elf32-powerpc")
OUTPUT_ARCH(powerpc)
ENTRY(_start)

MEMORY {
    MEM1 (rwx) : ORIGIN = 0x80003100, LENGTH = (0x01800000 - 0x3100 - 0x10000)
    STACK (rw) : ORIGIN = 0x817F0000, LENGTH = 0x00010000
}

__mem1_start   = 0x80000000;
__mem1_end     = 0x81800000;
__stack_top    = 0x817FFFF0;
__stack_bottom = 0x817F0000;
_SDA_BASE_     = 0;
_SDA2_BASE_    = 0;

SECTIONS {
    .crt0   ORIGIN(MEM1) : { KEEP(*(.crt0 .crt0.*)) } > MEM1
    .text   ALIGN(32)    : { *(.text .text.*) *(.gnu.linkonce.t.*) . = ALIGN(32); } > MEM1
    .rodata ALIGN(32)    : { *(.rodata .rodata.*) *(.gnu.linkonce.r.*) . = ALIGN(32); } > MEM1
    .data   ALIGN(32)    : { *(.data .data.*) *(.gnu.linkonce.d.*) . = ALIGN(32); } > MEM1
    .bss    ALIGN(32) (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*) *(.gnu.linkonce.b.*) *(COMMON)
        . = ALIGN(32);
        __bss_end = .;
    } > MEM1
    __heap_start = ALIGN(32);
    __heap_end   = __stack_bottom;
    /DISCARD/ : { *(.comment) *(.gnu.attributes) *(.note*) }
}
"#;
