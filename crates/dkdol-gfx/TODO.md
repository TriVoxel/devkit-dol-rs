# TODO: dkdol-gfx — Graphics (Remaining Work)

## Completed (Milestone 0)

- [x] `Xfb` struct (YCbCr 4:2:2 framebuffer abstraction)
- [x] `YcbcrPair` pixel type with common color constants
- [x] `Console` scrolling text console (putchar, print_str, fmt::Write)
- [x] 8×8 bitmap font (full printable ASCII 0x20–0x7E)
- [x] `Xfb::clear` and `Console::flush` with dcbf cache flush

## Remaining (Future Milestones)

### Milestone 1

- [ ] **Debug overlay** — a small persistent status bar showing frame count,
  CPU time, memory usage. Rendered on top of the main framebuffer.

### Milestone 3 (GX)

- [ ] **GX 2D drawing** — accelerated fills and blits using GX quads.
  Replace the software `Xfb::clear` with a GPU quad covering the screen.
- [ ] **Texture rendering** — display TGA/raw texture data via GX.
- [ ] **Sprite system** — `Sprite { texture, x, y, scale, rotation }` drawn
  with GX quads.

### Milestone 3+

- [ ] **8×16 font** — taller glyphs for improved readability. Port the
  libogc2 console_font_8x16 data (public domain).
- [ ] **Color themes** — light/dark mode, retro CGA palette, etc.
- [ ] **Unicode** — extend font coverage beyond ASCII (Latin-1 Supplement at least).
