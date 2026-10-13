# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- CPU: complete LR35902 base and `0xCB` opcode tables, with a T-state table for
  exact instruction timing.
- CPU: interrupt dispatch in hardware priority order, the one-instruction `EI`
  delay, and the `HALT` double-fetch bug.
- Bus: full DMG address map, including echo RAM, the unusable region, and I/O
  masking for unimplemented bits.
- Timer: `DIV`, `TIMA`, `TMA`, `TAC` derived from a single free-running counter,
  with the four-cycle reload delay.
- Cartridge: header parsing with checksum validation, and mappers for ROM-only,
  MBC1, MBC2, MBC3 (with a real-time clock) and MBC5.
- PPU: background, window and sprite rendering, palettes, `STAT` mode and `LYC`
  interrupts, and the ten-sprite per-line limit.
- Bus: OAM DMA with a 160 T-state CPU stall.
- Serial: link port as a sink, used by the test ROMs to report results.
- Frontends: a native minifb window and a `wasm-bindgen` browser binding.
- Test harness: discovers and runs every `.gb` under `test-roms/`, parsing
  Blargg's serial protocol, with `GB_REQUIRE_TEST_ROMS=1` to forbid skipping.

### Known limitations

- No APU, so `dmg_sound` fails by construction.
- No PPU FIFO, so mid-scanline effects and the `ppu` timing tests fail.
- No CGB support.
- OAM DMA is atomic rather than interleaved.

[Unreleased]: https://github.com/pevepee/gameboy-rs/commits/main
