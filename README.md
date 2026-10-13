# gameboy-rs

> A Game Boy (DMG) emulator in Rust, built to pass the test suites rather than to look like it works.

[![Blargg suite](https://img.shields.io/badge/Blargg-0%2F11-red)](docs/COMPATIBILITY.md)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)](rust-toolchain.toml)


![Pokémon Red running in gameboy-rs](https://i.giphy.com/U2nN0ridM4lXy.webp)

---

## Why this exists

Most emulator projects stop when the first game boots. This one is built the
other way around: the acceptance criteria are third-party conformance suites,
and the README reports the score. Every claim below is reproducible by running
`cargo test` and `scripts/fetch-test-roms.sh`.

Two things follow from that:

- **Timing is a first-class concern, not a later fix.** The Blargg `instr_timing`
  and `mem_timing` suites fail on any instruction whose cycle count is wrong by
  one, so the CPU carries a real cycle table from day one.
- **Failures are public.** [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) lists
  what does not work yet, with the failing test named.

## What works

| Subsystem | State |
|---|---|
| LR35902 CPU — full base and `0xCB` opcode tables | Working |
| Clock-exact instruction timing | Working |
| Interrupts, including the `EI` delay and the HALT bug | Working |
| Timer (DIV/TIMA/TMA/TAC) with the shared divider | Working |
| Cartridge mappers: ROM-only, MBC1, MBC2, MBC3 (+RTC), MBC5 | Working |
| PPU: background, window, sprites, palettes, STAT/LYC interrupts | Working |
| OAM DMA with the CPU stall | Working |
| Link port as a serial sink | Working |
| Audio (APU) | Not started |
| CGB (Game Boy Color) | Working |

## Quick start

```sh
git clone https://github.com/pevepee/gameboy-rs
cd gameboy-rs

# Run the unit tests — no network, no ROMs needed.
cargo test

# Fetch the third-party conformance suites and run them.
bash scripts/fetch-test-roms.sh        # or: pwsh -File scripts/fetch-test-roms.ps1
cargo test --release --test blargg -- --nocapture

# Play something.
cargo run --release -p gb-frontend -- path/to/tetris.gb --scale 4
```

### Controls                

| Key | Button |
|---|---|
| Arrow keys | D-pad |
| `Z` | A |
| `X` | B |
| `Enter` | Start |
| `Backspace` | Select |
| `Esc` | Quit |

### POKEMON CONTROLS (extra)

| Key | Button |
|---|---|
| Arrow keys | D-pad |
| `Z` | A |
| `X` | B |
| `Enter` | Start |
| `v` | Select |
| `Esc` | Quit |
| `DPAD_UP` | up arrow |
| `DPAD_DOWN` | down arrow |
| `DPAD_LEFT` | left arrow |
| `DPAD_RIGHT` | right arrow |


## Layout

```
crates/
  gb-core/        the emulator; no I/O, no rendering, no OS dependencies
  gb-frontend/    native window via minifb
  gb-wasm/        browser bindings via wasm-bindgen
docs/
  ARCHITECTURE.md  how the pieces fit and where the shortcuts are
  COMPATIBILITY.md per-ROM status, regenerated from a real run
scripts/
  fetch-test-roms.*  populate test-roms/ (gitignored)
```

`gb-core` is the whole emulator. The frontends are thin: they own a window and a
keyboard, and nothing else.

## Using it as a library

```rust
use gb_core::{Button, GameBoy};

let rom = std::fs::read("tetris.gb")?;
let mut gb = GameBoy::new(rom)?;

loop {
    // framebuffer() is 160x144 bytes of 2-bit color indices, row-major.
    let _screen = gb.framebuffer();
    gb.run_frame();
}

// Test ROMs report results over the link port.
println!("{}", gb.serial_output());
```

## Testing

```sh
cargo test                     # unit tests, no ROMs required
cargo test --release           # the same, with optimizations
GB_REQUIRE_TEST_ROMS=1 \
  cargo test --test blargg     # fail instead of skip when ROMs are absent
```

Test ROMs are **not** committed. The harness discovers every `.gb` under
`test-roms/` and skips cleanly when the directory is empty, so a fresh clone is
green without a network round trip. CI runs the suite in a dedicated job with
`GB_REQUIRE_TEST_ROMS=1`, so a silent skip cannot hide a regression.

Current score: see [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md).

## Roadmap

- [x] CPU, all opcodes, exact timing
- [x] Mappers MBC1/MBC2/MBC3/MBC5
- [x] PPU with background, window and sprites
- [ ] PPU FIFO, for mid-scanline register effects
- [ ] APU, all four channels
- [ ] Save states
- [ ] Battery-backed `.sav` persistence in the frontend
- [ ] Playable web build on GitHub Pages

## References

- [Pan Docs](https://gbdev.io/pandocs/) — the hardware reference
- [gbdev opcode table](https://gbdev.io/gb-opcodes/optables/)
- [The Cycle-Accurate Game Boy Docs](https://github.com/AntonioND/giibiiadvance/blob/master/docs/TCAGBD.pdf)
- [retrio/gb-test-roms](https://github.com/retrio/gb-test-roms) — Blargg's suites
- [Gekkio/mooneye-test-suite](https://github.com/Gekkio/mooneye-test-suite)

## License

MIT. See [LICENSE](LICENSE).
