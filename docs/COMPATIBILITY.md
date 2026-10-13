# Compatibility

Two tables: conformance suites, which are pass/fail, and commercial games, which
are a judgement call.

> **Status: not yet measured.** This file is the template and the method. Run the
> suite and fill in the real numbers before the first public commit — an honest
> low score is worth more than a guessed high one, and the CI badge in the README
> should match this file.

## Regenerating

```sh
bash scripts/fetch-test-roms.sh
cargo test --release -p gb-core --test blargg -- --nocapture
```

The harness walks `test-roms/` recursively, runs each `.gb` until it prints
`Passed` or `Failed` over the link port, and fails the run listing every ROM that
did not pass. Copy the result into the tables below.

## Conformance suites

### Blargg

| Suite | ROM | Status | Notes |
|---|---|---|---|
| cpu_instrs | `cpu_instrs/individual/01-special.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/02-interrupts.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/03-op sp,hl.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/04-op r,imm.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/05-op rp.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/06-ld r,r.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/07-jr,jp,call,ret,rst.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/08-misc instrs.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/09-op r,r.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/10-bit ops.gb` | — | |
| cpu_instrs | `cpu_instrs/individual/11-op a,(hl).gb` | — | |
| instr_timing | `instr_timing/instr_timing.gb` | — | Fails on any wrong cycle count |
| mem_timing | `mem_timing/mem_timing.gb` | — | |
| mem_timing | `mem_timing-2/mem_timing.gb` | — | |
| halt_bug | `halt_bug.gb` | — | Exercises the HALT double-fetch |
| interrupt_time | `interrupt_time/*.gb` | — | CGB-oriented; may not apply |
| dmg_sound | `dmg_sound/*.gb` | ✗ | No APU yet |
| oam_bug | `oam_bug/*.gb` | — | Requires exact OAM corruption behaviour |

### mooneye

| Group | Status | Notes |
|---|---|---|
| `acceptance/timer/*` | — | |
| `acceptance/ppu/*` | ✗ | Needs the FIFO rewrite |
| `acceptance/bits/*` | — | |
| `acceptance/instr/*` | — | |
| `acceptance/oam_dma/*` | — | |

## Commercial games

The bar for "working" is: boots, reaches gameplay, and survives ten minutes
without a visual or audio-timing defect that changes the outcome.

| Game | Mapper | Status | Notes |
|---|---|---|---|
| Tetris | ROM-only | — | |
| Super Mario Land | ROM-only | — | |
| Pokémon Red | MBC3 | — | Needs the RTC for time-of-day events |
| The Legend of Zelda: Link's Awakening | MBC1 | — | |
| Donkey Kong | ROM-only | — | |

Status legend: ✓ working · ~ playable with defects · ✗ not working · — untested

## Known limitations

These are architectural, not bugs. See [ARCHITECTURE.md](ARCHITECTURE.md).

1. **No PPU FIFO.** Mid-scanline register writes have no effect. Raster effects
   and the `ppu` timing tests cannot pass until this is rewritten.
2. **No APU.** All `dmg_sound` tests fail by construction.
3. **Mode 3 length is fixed.** It does not account for sprites on the line.
4. **No CGB support.** Color-only games will not boot.
5. **OAM DMA is atomic.** Games that read OAM during a transfer see the final
   state rather than a partial one.
