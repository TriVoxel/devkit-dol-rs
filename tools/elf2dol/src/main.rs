//! # elf2dol
//!
//! Converts a linked PowerPC ELF binary (`.elf`) into the Nintendo GameCube
//! DOL executable format (`.dol`).
//!
//! ## Usage
//!
//! ```text
//! elf2dol <input.elf> <output.dol>
//! ```
//!
//! ## DOL Format
//!
//! The DOL header is 256 bytes and describes up to 18 sections:
//! - 7 **text** sections (executable code)
//! - 11 **data** sections (initialized data, read-only data)
//!
//! Each section has:
//! - A file offset within the DOL
//! - A load address (where the IPL copies it in RAM)
//! - A byte length
//!
//! After all sections, the header contains:
//! - BSS start address and size (zeroed by the loader)
//! - Entry point address (`_start`)
//!
//! The DOL header is always padded to 256 bytes. Section data follows
//! immediately after, aligned to 32 bytes.
//!
//! Reference: <https://www.gc-forever.com/yagcd/chap13.html>

use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: elf2dol <input.elf> <output.dol>");
        std::process::exit(1);
    }

    let input  = &args[1];
    let output = &args[2];

    let elf_bytes = fs::read(input).unwrap_or_else(|e| {
        eprintln!("Error reading '{}': {}", input, e);
        std::process::exit(1);
    });

    let dol = elf_to_dol(&elf_bytes).unwrap_or_else(|e| {
        eprintln!("Conversion error: {}", e);
        std::process::exit(1);
    });

    fs::write(output, &dol).unwrap_or_else(|e| {
        eprintln!("Error writing '{}': {}", output, e);
        std::process::exit(1);
    });

    let elf_size = elf_bytes.len();
    let dol_size = dol.len();
    println!(
        "elf2dol: {} ({} bytes) → {} ({} bytes)",
        input, elf_size, output, dol_size
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ELF parsing helpers (big-endian, 32-bit)
// ─────────────────────────────────────────────────────────────────────────────

fn u16be(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off+1]])
}

fn u32be(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off+1], b[off+2], b[off+3]])
}

const ELF_MAGIC:   &[u8] = &[0x7F, b'E', b'L', b'F'];
const ET_EXEC:     u16   = 2;
const EM_PPC:      u16   = 20;
const PT_LOAD:     u32   = 1;
const SHT_NOBITS:  u32   = 8;   // BSS
const SHF_ALLOC:   u32   = 0x2;

struct ElfSect {
    _name_idx: u32,
    sh_type:  u32,
    flags:    u32,
    addr:     u32,
    _offset:  u32,
    size:     u32,
}

struct ElfPhdr {
    p_type:   u32,
    offset:   u32,
    vaddr:    u32,
    filesz:   u32,
    _memsz:   u32,
    flags:    u32,
    _align:   u32,
}

fn elf_to_dol(elf: &[u8]) -> Result<Vec<u8>, String> {
    // ── Validate ELF header ──────────────────────────────────────────────
    if elf.len() < 52 {
        return Err("ELF too small".into());
    }
    if &elf[0..4] != ELF_MAGIC {
        return Err("Not an ELF file".into());
    }
    if elf[4] != 1 { return Err("Expected 32-bit ELF (ELFCLASS32)".into()); }
    if elf[5] != 2 { return Err("Expected big-endian ELF (ELFDATA2MSB)".into()); }

    let e_type    = u16be(elf, 16);
    let e_machine = u16be(elf, 18);
    if e_type    != ET_EXEC { return Err(format!("e_type={:#x}, expected ET_EXEC", e_type)); }
    if e_machine != EM_PPC  { return Err(format!("e_machine={:#x}, expected EM_PPC (20)", e_machine)); }

    let e_entry   = u32be(elf, 24);
    let e_phoff   = u32be(elf, 28) as usize;
    let e_shoff   = u32be(elf, 32) as usize;
    let e_phentsize = u16be(elf, 42) as usize;
    let e_phnum     = u16be(elf, 44) as usize;
    let e_shentsize = u16be(elf, 46) as usize;
    let e_shnum     = u16be(elf, 48) as usize;
    let _e_shstrndx = u16be(elf, 50) as usize;

    // ── Parse program headers (PT_LOAD segments) ─────────────────────────
    let mut phdrs: Vec<ElfPhdr> = Vec::new();
    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        let ph = ElfPhdr {
            p_type: u32be(elf, off),
            offset:  u32be(elf, off + 4),
            vaddr:  u32be(elf, off + 8),
            filesz: u32be(elf, off + 16),
            _memsz:  u32be(elf, off + 20),
            flags:  u32be(elf, off + 24),
            _align:  u32be(elf, off + 28),
        };
        if ph.p_type == PT_LOAD { phdrs.push(ph); }
    }

    // ── Parse section headers (to find BSS) ──────────────────────────────
    let mut bss_start: u32 = 0;
    let mut bss_size:  u32 = 0;
    let mut sects: Vec<ElfSect> = Vec::new();
    for i in 0..e_shnum {
        let off = e_shoff + i * e_shentsize;
        let s = ElfSect {
            _name_idx: u32be(elf, off),
            sh_type:  u32be(elf, off + 4),
            flags:    u32be(elf, off + 8),
            addr:     u32be(elf, off + 12),
            _offset:   u32be(elf, off + 16),
            size:     u32be(elf, off + 20),
        };
        if s.sh_type == SHT_NOBITS && s.flags & SHF_ALLOC != 0 && s.size > 0 {
            // Found a BSS-style section.
            if bss_size == 0 || s.addr < bss_start {
                bss_start = s.addr;
            }
            bss_size += s.size;
        }
        sects.push(s);
    }

    // ── Build DOL sections from PT_LOAD segments ─────────────────────────
    // DOL has at most 7 text + 11 data sections (18 total).
    // We map executable segments to text slots, others to data slots.

    const MAX_TEXT: usize = 7;
    const MAX_DATA: usize = 11;

    let mut text_sections: Vec<(u32, u32, Vec<u8>)> = Vec::new(); // (load_addr, file_offset_placeholder, data)
    let mut data_sections: Vec<(u32, u32, Vec<u8>)> = Vec::new();

    for ph in &phdrs {
        if ph.filesz == 0 { continue; }
        let seg_data = elf[ph.offset as usize .. (ph.offset + ph.filesz) as usize].to_vec();
        let is_exec  = ph.flags & 1 != 0; // PF_X
        if is_exec {
            if text_sections.len() < MAX_TEXT {
                text_sections.push((ph.vaddr, 0, seg_data));
            } else {
                return Err("Too many text segments (max 7)".into());
            }
        } else {
            if data_sections.len() < MAX_DATA {
                data_sections.push((ph.vaddr, 0, seg_data));
            } else {
                return Err("Too many data segments (max 11)".into());
            }
        }
    }

    if text_sections.is_empty() && data_sections.is_empty() {
        return Err("No loadable segments found in ELF".into());
    }

    // ── Assemble the DOL ─────────────────────────────────────────────────
    // Header: 256 bytes
    // Then: sections in order (each 32-byte aligned)

    let header_size: u32 = 0x100;
    let mut body: Vec<u8>  = Vec::new();
    let mut file_offsets_text = [0u32; MAX_TEXT];
    let mut sizes_text        = [0u32; MAX_TEXT];
    let mut addrs_text        = [0u32; MAX_TEXT];
    let mut file_offsets_data = [0u32; MAX_DATA];
    let mut sizes_data        = [0u32; MAX_DATA];
    let mut addrs_data        = [0u32; MAX_DATA];

    for (i, (load_addr, _, data)) in text_sections.iter().enumerate() {
        // Align to 32 bytes
        while body.len() % 32 != 0 { body.push(0); }
        file_offsets_text[i] = header_size + body.len() as u32;
        sizes_text[i]        = data.len() as u32;
        addrs_text[i]        = *load_addr;
        body.extend_from_slice(data);
    }

    for (i, (load_addr, _, data)) in data_sections.iter().enumerate() {
        while body.len() % 32 != 0 { body.push(0); }
        file_offsets_data[i] = header_size + body.len() as u32;
        sizes_data[i]        = data.len() as u32;
        addrs_data[i]        = *load_addr;
        body.extend_from_slice(data);
    }

    // ── Write the 256-byte header ─────────────────────────────────────────
    let mut hdr = vec![0u8; 0x100];

    // Offsets of text sections:   0x000 – 0x01B (7 × 4 bytes)
    for i in 0..MAX_TEXT {
        let off = i * 4;
        hdr[0x000 + off .. 0x000 + off + 4].copy_from_slice(&file_offsets_text[i].to_be_bytes());
    }
    // Offsets of data sections:   0x01C – 0x047 (11 × 4 bytes)
    for i in 0..MAX_DATA {
        let off = i * 4;
        hdr[0x01C + off .. 0x01C + off + 4].copy_from_slice(&file_offsets_data[i].to_be_bytes());
    }
    // Load addresses of text:     0x048 – 0x063
    for i in 0..MAX_TEXT {
        let off = i * 4;
        hdr[0x048 + off .. 0x048 + off + 4].copy_from_slice(&addrs_text[i].to_be_bytes());
    }
    // Load addresses of data:     0x064 – 0x08F
    for i in 0..MAX_DATA {
        let off = i * 4;
        hdr[0x064 + off .. 0x064 + off + 4].copy_from_slice(&addrs_data[i].to_be_bytes());
    }
    // Sizes of text sections:     0x090 – 0x0AB
    for i in 0..MAX_TEXT {
        let off = i * 4;
        hdr[0x090 + off .. 0x090 + off + 4].copy_from_slice(&sizes_text[i].to_be_bytes());
    }
    // Sizes of data sections:     0x0AC – 0x0D7
    for i in 0..MAX_DATA {
        let off = i * 4;
        hdr[0x0AC + off .. 0x0AC + off + 4].copy_from_slice(&sizes_data[i].to_be_bytes());
    }
    // BSS address:                0x0D8
    hdr[0x0D8 .. 0x0DC].copy_from_slice(&bss_start.to_be_bytes());
    // BSS size:                   0x0DC
    hdr[0x0DC .. 0x0E0].copy_from_slice(&bss_size.to_be_bytes());
    // Entry point:                0x0E0
    hdr[0x0E0 .. 0x0E4].copy_from_slice(&e_entry.to_be_bytes());
    // 0x0E4 – 0x0FF: padding (zeros)

    let mut dol = hdr;
    dol.extend_from_slice(&body);
    Ok(dol)
}
