# Architecture

## The one design decision that matters

**The CPU owns no memory, and the bus owns all state.**

```rust
cpu.step(&mut bus) -> u32    // returns the T-states consumed
```

`step` returns a cycle count and the caller feeds it to the peripherals. This
means:

- The CPU has no borrow of the bus beyond a single call, so there are no
  self-referential structures and no `Rc<RefCell<..>>` anywhere.
- Timing is testable in isolation. `assert_eq!(cpu.step(&mut bus), 24)` is a
  complete statement about a `CALL`.
- The peripherals advance in the same units the CPU measures, so the timer and
  PPU can never drift out of step with instruction execution.

The alternative — peripherals ticking themselves from a global clock — needs a
scheduler, and a scheduler needs the CPU to be re-entrant. Not worth it here.

## Ownership map

```
GameBoy
├── Cpu          registers, IME, EI delay, HALT bug latch
└── Bus
    ├── Cartridge  ROM, RAM, MBC state, MBC3 RTC
    ├── Ppu        VRAM, OAM, register file, framebuffer
    ├── Timer      one 16-bit divider behind DIV/TIMA/TMA/TAC
    ├── Joypad     a 2x4 active-low matrix
    └── Serial     SB/SC, accumulated output
    plus wram, hram, IE, IF, the OAM DMA stall counter
```

Everything is a plain owned value. `GameBoy` is `Send`, has no destructor
behaviour, and can be moved freely.

## Invariants worth knowing before you touch anything

**The flag register's low nibble is not storage.** `Registers::f` is private;
write through `set_flag` or `set_af`. Anything that writes `f` directly will
corrupt the `POP AF` round trip.

**Every ROM read goes through a bounds check.** `Cartridge::read_rom` returns
`0xFF` for an out-of-range bank rather than panicking. ROMs in the wild declare
sizes they do not have, and a truncated dump must not take the process down.

**Reading and writing `0xFF46` are not symmetric.** The write starts a transfer;
the read returns the source page. `start_oam_dma` performs the 160-byte copy
immediately and then stalls the CPU for 160 T-states. That is an approximation:
on hardware the copy is interleaved with the stall, which matters only to ROMs
that race the transfer. No test in the suite we run does.

**`BIT` does not touch the carry flag.** It is the one rotate-family instruction
whose carry behaviour differs, and it is a classic source of silent corruption.

**`INC` and `DEC` do not touch the carry flag** either. They set `H`, `N` and `Z`
only.

## Where the shortcuts are

Being explicit about approximations is cheaper than pretending they are exact.

### PPU: line-granular, not FIFO

The real PPU is a pixel FIFO that fetches tiles while it renders, which is why
games can change `SCX` mid-scanline and see a split screen. This implementation
renders a whole scanline in one pass at the line boundary.

Consequences:

- Background, window and sprites all render correctly in the common case.
- Mid-scanline effects (raster splits, wavy-water effects) will be wrong.
- The `ppu` timing portion of the test suite cannot pass.

The rewrite is a real FIFO with a background fetcher and a sprite fetcher, run
once per dot. It is on the roadmap and is the largest single piece of remaining
work in `gb-core`.

### Mode timing

Modes 2, 3 and 0 are modelled with the dot counter inside the 456-dot line, so
`STAT` reads report a plausible mode and `LY`/`LYC` interrupts fire. Mode 3 is
treated as ending at dot 252 for every line; on hardware its length depends on
how many sprites are on the line.

### OAM DMA

See the invariant above. The copy is atomic; the stall is accurate.

### STOP

`STOP` puts the CPU in a terminal state. On a DMG that is correct absent a
joypad wake, and no ROM in our suite depends on it.

### Audio

There is no APU. `0xFF10..=0xFF3F` reads return `0xFF` and writes are counted in
`Bus::unmapped_writes`. `dmg_sound` tests are expected to fail.

## Diagnostics

`Bus::unmapped_writes` counts writes to I/O addresses with no backing register.
A nonzero count while a test ROM fails is the fastest way to find a missing
peripheral — it means the ROM is talking to something that is not there.

## Adding an opcode

The opcode tables are complete, so this should not come up. If it does:

1. Add the arm to `Cpu::execute` or `Cpu::execute_cb`.
2. Set its T-state count in `BASE_CYCLES`, or confirm `cb_cycles` covers it.
3. The compiler will not tell you if the count is wrong. `instr_timing` will.

## Adding a mapper

1. Add a variant to `MbcKind` and map its cartridge type byte in
   `from_cartridge_type`.
2. Extend `current_rom_bank`, `write_rom` and the RAM paths in
   `Cartridge`.
3. Add a `#[cfg(test)]` case that switches banks and asserts the read lands
   where the mapper says it should. `mbc5_allows_bank_zero_in_the_high_window`
   is the template.
