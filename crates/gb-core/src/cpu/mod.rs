//! The LR35902 CPU: fetch, decode, execute, and interrupt dispatch.
//!
//! The CPU owns no memory. Every access goes through [`Bus`], and `step` returns
//! the T-states the instruction consumed, which the caller then feeds to the
//! peripherals. That split is what lets the test harness drive the machine
//! instruction-by-instruction without a scheduler.

mod registers;

pub use registers::{Registers, FLAG_C, FLAG_H, FLAG_N, FLAG_Z};

use crate::bus::Bus;
use crate::interrupt;

/// T-states per opcode, in the not-taken form of every conditional branch.
///
/// Entries of `0` mark opcodes the LR35902 does not implement; reaching one is
/// a hard fault, exactly as on hardware.
const BASE_CYCLES: [u8; 256] = [
    // 0x00
    4, 12, 8, 8, 4, 4, 8, 4, 20, 8, 8, 8, 4, 4, 8, 4, //
    // 0x10
    4, 12, 8, 8, 4, 4, 8, 4, 12, 8, 8, 8, 4, 4, 8, 4, //
    // 0x20
    8, 12, 8, 8, 4, 4, 8, 4, 8, 8, 8, 8, 4, 4, 8, 4, //
    // 0x30
    8, 12, 8, 8, 12, 12, 12, 4, 8, 8, 8, 8, 4, 4, 8, 4, //
    // 0x40
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0x50
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0x60
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0x70
    8, 8, 8, 8, 8, 8, 4, 8, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0x80
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0x90
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0xA0
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0xB0
    4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, //
    // 0xC0
    8, 12, 12, 16, 12, 16, 8, 16, 8, 16, 12, 4, 12, 24, 8, 16, //
    // 0xD0
    8, 12, 12, 0, 12, 16, 8, 16, 8, 16, 12, 0, 12, 0, 8, 16, //
    // 0xE0
    12, 12, 8, 0, 0, 16, 8, 16, 16, 4, 16, 0, 0, 0, 8, 16, //
    // 0xF0
    12, 12, 8, 4, 0, 16, 8, 16, 12, 8, 16, 4, 0, 0, 8, 16,
];

/// T-states for a `0xCB`-prefixed opcode.
///
/// The `(HL)` forms cost four extra cycles for the memory round trip, except the
/// `BIT` group, which only reads and so costs two extra.
#[must_use]
fn cb_cycles(cb: u8) -> u32 {
    if cb & 7 != 6 {
        return 8;
    }
    match cb >> 6 {
        0 | 3 => 16, // rotate/shift and SET/RES on (HL): 2 memory accesses
        _ => 12,     // BIT (HL): read only, no write back
    }
}

pub struct Cpu {
    pub regs: Registers,
    /// Interrupt master enable.
    pub ime: bool,
    /// HALT reached, waiting for an enabled interrupt to become pending.
    pub halted: bool,
    /// STOP reached. On a DMG this is a terminal state absent a joypad wake.
    pub stopped: bool,
    /// Countdown that delays IME by one instruction after `EI`.
    ///
    /// Set to 2 by `EI`; each completed instruction decrements it, and IME turns
    /// on when it reaches zero. `DI` clears both.
    ei_delay: u8,
    /// Set by `HALT` when IME is clear and an interrupt is already pending. The
    /// CPU keeps running and reads the following byte twice, which is the
    /// documented HALT bug ROMs such as Blargg's `halt_bug` test rely on.
    halt_bug: bool,
    /// Ticks already consumed inside `execute` before the final `bus.tick()`.
    pending_ticks: u32,
}

impl Cpu {
    #[must_use]
    pub fn new() -> Self {
        Self {
            regs: Registers::post_boot(),
            ime: false,
            halted: false,
            stopped: false,
            ei_delay: 0,
            halt_bug: false,
            pending_ticks: 0,
        }
    }

    /// Prepare to run a ROM without emulating the boot ROM.
    pub fn apply_post_boot_state(&mut self) {
        self.regs = Registers::post_boot();
        self.ime = false;
        self.halted = false;
        self.stopped = false;
        self.ei_delay = 0;
        self.halt_bug = false;
        self.pending_ticks = 0;
    }

    /// Execute one instruction, or service one interrupt.
    ///
    /// Returns the T-states consumed. The bus is ticked for those cycles before
    /// this returns, so the caller only has to count frames.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        // An OAM DMA holds the CPU off the bus for its whole duration.
        if bus.dma_active() {
            bus.tick(4);
            return 4;
        }

        if self.ime {
            if let Some(cycles) = self.service_interrupt(bus) {
                bus.tick(cycles);
                return cycles;
            }
        }

        if self.halted {
            if bus.iflag & bus.ie() & interrupt::ALL != 0 {
                self.halted = false;
            } else {
                bus.tick(4);
                return 4;
            }
        }

        if self.stopped {
            bus.tick(4);
            return 4;
        }

        self.pending_ticks = 0;
        let opcode = self.fetch_u8(bus);
        let cycles = self.execute(bus, opcode);

        // `EI` arms the delay; the instruction after it lands IME.
        if self.ei_delay > 0 {
            self.ei_delay -= 1;
            if self.ei_delay == 0 {
                self.ime = true;
            }
        }

        let remaining = cycles.saturating_sub(self.pending_ticks);
        bus.tick(remaining);
        
        cycles
    }

    /// Push the return address and jump to the highest-priority vector.
    /// Push the return address and jump to the highest-priority vector.
    fn service_interrupt(&mut self, bus: &mut Bus) -> Option<u32> {
        let iflag = bus.iflag;
        let ie = bus.ie();
        
        // Verifica se existe alguma interrupção pendente e ativada
        if iflag & ie & interrupt::ALL == 0 {
            return None;
        }

        let (bit, vector) = interrupt::pending(iflag, ie)?;

        self.ime = false;
        self.halted = false;
        self.stopped = false;
        self.ei_delay = 0;

        bus.set_iflag(iflag & !bit);

        let pc = self.regs.pc;
        self.push_u16(bus, pc);
        self.regs.pc = vector;

        Some(20)
    }

    fn fetch_u8(&mut self, bus: &mut Bus) -> u8 {
        let value = bus.read(self.regs.pc);
        if self.halt_bug {
            // The HALT bug: this byte is not consumed, so it is fetched again.
            self.halt_bug = false;
        } else {
            self.regs.pc = self.regs.pc.wrapping_add(1);
        }
        value
    }

    fn fetch_u16(&mut self, bus: &mut Bus) -> u16 {
        let low = u16::from(self.fetch_u8(bus));
        let high = u16::from(self.fetch_u8(bus));
        low | (high << 8)
    }

    fn push_u16(&mut self, bus: &mut Bus, value: u16) {
        self.regs.sp = self.regs.sp.wrapping_sub(2);
        let sp = self.regs.sp;
        bus.write(sp, value as u8);
        bus.write(sp.wrapping_add(1), (value >> 8) as u8);
    }

    fn pop_u16(&mut self, bus: &mut Bus) -> u16 {
        let sp = self.regs.sp;
        let low = u16::from(bus.read(sp));
        let high = u16::from(bus.read(sp.wrapping_add(1)));
        self.regs.sp = sp.wrapping_add(2);
        low | (high << 8)
    }

    /// Read a register operand by its 3-bit encoding. Index 6 is `(HL)`.
    fn read_r(&mut self, bus: &mut Bus, index: u8) -> u8 {
        match index {
            0 => self.regs.b,
            1 => self.regs.c,
            2 => self.regs.d,
            3 => self.regs.e,
            4 => self.regs.h,
            5 => self.regs.l,
            6 => {
                bus.tick(4);
                self.pending_ticks += 4;
                bus.read(self.regs.hl())
            }
            _ => self.regs.a,
        }
    }

    fn write_r(&mut self, bus: &mut Bus, index: u8, value: u8) {
        match index {
            0 => self.regs.b = value,
            1 => self.regs.c = value,
            2 => self.regs.d = value,
            3 => self.regs.e = value,
            4 => self.regs.h = value,
            5 => self.regs.l = value,
            6 => {
                bus.tick(4);
                self.pending_ticks += 4;
                let hl = self.regs.hl();
                bus.write(hl, value);
            }
            _ => self.regs.a = value,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, bus: &mut Bus, op: u8) -> u32 {
        match op {
            0x00 => {}

            0x76 => {
                // HALT with IME clear and an interrupt already pending does not
                // halt: it triggers the double-fetch bug instead.
                let pending = bus.iflag & bus.ie() & interrupt::ALL != 0;
                if !self.ime && pending {
                    self.halt_bug = true;
                } else {
                    self.halted = true;
                }
            }

            0x10 => {
                // STOP is two bytes wide; the padding byte is discarded.
                let _ = self.fetch_u8(bus);
                self.stopped = true;
            }

            // LD rr, nn
            0x01 => {
                let value = self.fetch_u16(bus);
                self.regs.set_bc(value);
            }
            0x11 => {
                let value = self.fetch_u16(bus);
                self.regs.set_de(value);
            }
            0x21 => {
                let value = self.fetch_u16(bus);
                self.regs.set_hl(value);
            }
            0x31 => {
                let value = self.fetch_u16(bus);
                self.regs.sp = value;
            }

            // LD (rr), A
            0x02 => {
                let bc = self.regs.bc();
                let a = self.regs.a;
                bus.tick(4);
                self.pending_ticks += 4;
                bus.write(bc, a);
            }
            0x12 => {
                let de = self.regs.de();
                let a = self.regs.a;
                bus.tick(4);
                self.pending_ticks += 4;
                bus.write(de, a);
            }
            0x22 => {
                let hl = self.regs.hl();
                let a = self.regs.a;
                bus.tick(4);
                self.pending_ticks += 4;
                bus.write(hl, a);
                self.regs.set_hl(hl.wrapping_add(1));
            }
            0x32 => {
                let hl = self.regs.hl();
                let a = self.regs.a;
                bus.tick(4);
                self.pending_ticks += 4;
                bus.write(hl, a);
                self.regs.set_hl(hl.wrapping_sub(1));
            }

            // LD A, (rr)
            0x0A => {
                let bc = self.regs.bc();
                bus.tick(4);
                self.pending_ticks += 4;
                self.regs.a = bus.read(bc);
            }
            0x1A => {
                let de = self.regs.de();
                bus.tick(4);
                self.pending_ticks += 4;
                self.regs.a = bus.read(de);
            }
            0x2A => {
                let hl = self.regs.hl();
                bus.tick(4);
                self.pending_ticks += 4;
                self.regs.a = bus.read(hl);
                self.regs.set_hl(hl.wrapping_add(1));
            }
            0x3A => {
                let hl = self.regs.hl();
                bus.tick(4);
                self.pending_ticks += 4;
                self.regs.a = bus.read(hl);
                self.regs.set_hl(hl.wrapping_sub(1));
            }

            // LD (nn), SP
            0x08 => {
                let addr = self.fetch_u16(bus);
                let sp = self.regs.sp;
                bus.tick(8);
                self.pending_ticks += 8;
                bus.write(addr, sp as u8);
                bus.write(addr.wrapping_add(1), (sp >> 8) as u8);
            }

            // INC rr
            0x03 => {
                let value = self.regs.bc().wrapping_add(1);
                self.regs.set_bc(value);
            }
            0x13 => {
                let value = self.regs.de().wrapping_add(1);
                self.regs.set_de(value);
            }
            0x23 => {
                let value = self.regs.hl().wrapping_add(1);
                self.regs.set_hl(value);
            }
            0x33 => self.regs.sp = self.regs.sp.wrapping_add(1),

            // DEC rr
            0x0B => {
                let value = self.regs.bc().wrapping_sub(1);
                self.regs.set_bc(value);
            }
            0x1B => {
                let value = self.regs.de().wrapping_sub(1);
                self.regs.set_de(value);
            }
            0x2B => {
                let value = self.regs.hl().wrapping_sub(1);
                self.regs.set_hl(value);
            }
            0x3B => self.regs.sp = self.regs.sp.wrapping_sub(1),

            // INC r
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
                let index = (op >> 3) & 7;
                let value = self.read_r(bus, index);
                let result = self.inc8(value);
                self.write_r(bus, index, result);
            }

            // DEC r
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
                let index = (op >> 3) & 7;
                let value = self.read_r(bus, index);
                let result = self.dec8(value);
                self.write_r(bus, index, result);
            }

            // LD r, n
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
                let index = (op >> 3) & 7;
                let value = self.fetch_u8(bus);
                
                if index == 6 {
                    bus.tick(8);
                    self.pending_ticks += 8;
                }
                
                self.write_r(bus, index, value);
            }

            // Rotates and flag-only operations on A.
            0x07 => self.rlca(),
            0x0F => self.rrca(),
            0x17 => self.rla(),
            0x1F => self.rra(),
            0x27 => self.daa(),
            0x2F => self.cpl(),
            0x37 => self.scf(),
            0x3F => self.ccf(),

            // JR e
            0x18 => {
                let offset = self.fetch_u8(bus) as i8;
                self.jump_relative(offset);
            }

            // JR cc, e
            0x20 | 0x28 | 0x30 | 0x38 => {
                let offset = self.fetch_u8(bus) as i8;
                if self.regs.condition((op >> 3) & 3) {
                    self.jump_relative(offset);
                    return 12;
                }
            }

            // LD r, r'
            0x40..=0x7F => {
                let dst = (op >> 3) & 7;
                let src = op & 7;
                let value = self.read_r(bus, src);
                self.write_r(bus, dst, value);
            }

            // ALU A, r
            0x80..=0xBF => {
                let src = op & 7;
                let value = self.read_r(bus, src);
                self.alu((op >> 3) & 7, value);
            }

            // RET cc
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                if self.regs.condition((op >> 3) & 3) {
                    let addr = self.pop_u16(bus);
                    self.regs.pc = addr;
                    return 20;
                }
            }

            // POP rr
            0xC1 => {
                let value = self.pop_u16(bus);
                self.regs.set_bc(value);
            }
            0xD1 => {
                let value = self.pop_u16(bus);
                self.regs.set_de(value);
            }
            0xE1 => {
                let value = self.pop_u16(bus);
                self.regs.set_hl(value);
            }
            0xF1 => {
                let value = self.pop_u16(bus);
                self.regs.set_af(value);
            }

            // JP cc, nn
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let addr = self.fetch_u16(bus);
                if self.regs.condition((op >> 3) & 3) {
                    self.regs.pc = addr;
                    return 16;
                }
            }

            // JP nn
            0xC3 => {
                let addr = self.fetch_u16(bus);
                self.regs.pc = addr;
            }

            // CALL cc, nn
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let addr = self.fetch_u16(bus);
                if self.regs.condition((op >> 3) & 3) {
                    let pc = self.regs.pc;
                    self.push_u16(bus, pc);
                    self.regs.pc = addr;
                    return 24;
                }
            }

            // PUSH rr
            0xC5 => {
                let value = self.regs.bc();
                self.push_u16(bus, value);
            }
            0xD5 => {
                let value = self.regs.de();
                self.push_u16(bus, value);
            }
            0xE5 => {
                let value = self.regs.hl();
                self.push_u16(bus, value);
            }
            0xF5 => {
                let value = self.regs.af();
                self.push_u16(bus, value);
            }

            // ALU A, n
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let value = self.fetch_u8(bus);
                self.alu((op >> 3) & 7, value);
            }

            // RST
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                let pc = self.regs.pc;
                self.push_u16(bus, pc);
                self.regs.pc = u16::from(op & 0x38);
            }

            // RET
            0xC9 => {
                let addr = self.pop_u16(bus);
                self.regs.pc = addr;
            }

            // RETI
            0xD9 => {
                let addr = self.pop_u16(bus);
                self.regs.pc = addr;
                self.ime = true;
                self.ei_delay = 0;
            }

            // CALL nn
            0xCD => {
                let addr = self.fetch_u16(bus);
                let pc = self.regs.pc;
                self.push_u16(bus, pc);
                self.regs.pc = addr;
            }

            // LDH (n), A
            0xE0 => {
                let offset = self.fetch_u8(bus);
                let a = self.regs.a;
                bus.tick(8);
                self.pending_ticks += 8;
                bus.write(0xFF00 + u16::from(offset), a);
            }

            // LDH A, (n)
            0xF0 => {
                let offset = self.fetch_u8(bus);
                bus.tick(8);
                self.pending_ticks += 8;
                self.regs.a = bus.read(0xFF00 + u16::from(offset));
            }

            // LD (C), A
            0xE2 => {
                let a = self.regs.a;
                let port = 0xFF00 + u16::from(self.regs.c);
                bus.tick(4);
                self.pending_ticks += 4;
                bus.write(port, a);
            }

            // LD A, (C)
            0xF2 => {
                let port = 0xFF00 + u16::from(self.regs.c);
                bus.tick(4);
                self.pending_ticks += 4;
                self.regs.a = bus.read(port);
            }

            // LD (nn), A
            0xEA => {
                let addr = self.fetch_u16(bus);
                let a = self.regs.a;
                bus.tick(12);
                self.pending_ticks += 12;
                bus.write(addr, a);
            }

            // LD A, (nn)
            0xFA => {
                let addr = self.fetch_u16(bus);
                bus.tick(12);
                self.pending_ticks += 12;
                self.regs.a = bus.read(addr);
            }

            // ADD SP, e8
            0xE8 => return self.add_sp_e8(bus),

            // LD HL, SP + e8
            0xF8 => return self.ld_hl_sp_e8(bus),

            // JP (HL)
            0xE9 => self.regs.pc = self.regs.hl(),

            // LD SP, HL
            0xF9 => self.regs.sp = self.regs.hl(),

            // DI
            0xF3 => {
                self.ime = false;
                self.ei_delay = 0;
            }

            // EI
            0xFB => self.ei_delay = 2,

            // CB prefix
            0xCB => {
                let cb = self.fetch_u8(bus);
                return self.execute_cb(bus, cb);
            }

            0x29 => {
                // ADD HL, HL
                let hl = self.regs.hl();
                let result = hl.wrapping_add(hl);

                self.regs.set_n(false);
                self.regs.set_h(((hl & 0x0FFF) + (hl & 0x0FFF)) > 0x0FFF);
                self.regs.set_c((u32::from(hl) + u32::from(hl)) > 0xFFFF);
                self.regs.set_hl(result);

                return 8;
            }

            0x39 => {
                // ADD HL, SP
                let hl = self.regs.hl();
                let sp = self.regs.sp;
                let result = hl.wrapping_add(sp);

                self.regs.set_n(false);
                self.regs.set_h(((hl & 0x0FFF) + (sp & 0x0FFF)) > 0x0FFF);
                self.regs.set_c((u32::from(hl) + u32::from(sp)) > 0xFFFF);
                self.regs.set_hl(result);

                return 8;
            }

            0x09 => {
                // ADD HL, BC
                let hl = self.regs.hl();
                let bc = self.regs.bc();
                let result = hl.wrapping_add(bc);

                self.regs.set_n(false);
                self.regs.set_h(((hl & 0x0FFF) + (bc & 0x0FFF)) > 0x0FFF);
                self.regs.set_c((u32::from(hl) + u32::from(bc)) > 0xFFFF);
                self.regs.set_hl(result);

                return 8;
            }

            0x19 => {
                // ADD HL, DE
                let hl = self.regs.hl();
                let de = self.regs.de();
                let result = hl.wrapping_add(de);

                self.regs.set_n(false);
                self.regs.set_h(((hl & 0x0FFF) + (de & 0x0FFF)) > 0x0FFF);
                self.regs.set_c((u32::from(hl) + u32::from(de)) > 0xFFFF);
                self.regs.set_hl(result);

                return 8;
            }

            _ => panic!(
                "unimplemented opcode {op:#04X} at {:#06X}",
                self.regs.pc.wrapping_sub(1)
            ),
        }

        u32::from(BASE_CYCLES[op as usize])
    }

    fn execute_cb(&mut self, bus: &mut Bus, cb: u8) -> u32 {
        let index = cb & 7;
        let bit = (cb >> 3) & 7;
        let value = self.read_r(bus, index);

        match cb >> 6 {
            // Rotate and shift group
            0 => {
                let result = match (cb >> 3) & 7 {
                    0 => self.rlc(value),
                    1 => self.rrc(value),
                    2 => self.rl(value),
                    3 => self.rr(value),
                    4 => self.sla(value),
                    5 => self.sra(value),
                    6 => self.swap(value),
                    _ => self.srl(value),
                };
                self.write_r(bus, index, result);
            }
            // BIT: reads only
            1 => self.bit(value, bit),
            // RES
            2 => {
                let result = value & !(1 << bit);
                self.write_r(bus, index, result);
            }
            // SET
            _ => {
                let result = value | (1 << bit);
                self.write_r(bus, index, result);
            }
        }

        cb_cycles(cb)
    }

    fn jump_relative(&mut self, offset: i8) {
        self.regs.pc = self.regs.pc.wrapping_add(offset as i16 as u16);
    }

    fn alu(&mut self, index: u8, value: u8) {
        match index {
            0 => self.add_a(value),
            1 => self.adc_a(value),
            2 => self.sub_a(value),
            3 => self.sbc_a(value),
            4 => self.and_a(value),
            5 => self.xor_a(value),
            6 => self.or_a(value),
            _ => self.cp_a(value),
        }
    }

    fn add_a(&mut self, value: u8) {
        let a = self.regs.a;
        let result = a.wrapping_add(value);
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h((a & 0x0F) + (value & 0x0F) > 0x0F);
        self.regs.set_c(u16::from(a) + u16::from(value) > 0xFF);
        self.regs.a = result;
    }

    fn adc_a(&mut self, value: u8) {
        let a = self.regs.a;
        let carry = u8::from(self.regs.c());
        let result = a.wrapping_add(value).wrapping_add(carry);
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h((a & 0x0F) + (value & 0x0F) + carry > 0x0F);
        self.regs
            .set_c(u16::from(a) + u16::from(value) + u16::from(carry) > 0xFF);
        self.regs.a = result;
    }

    fn sub_a(&mut self, value: u8) {
        let a = self.regs.a;
        let result = a.wrapping_sub(value);
        self.regs.set_z(result == 0);
        self.regs.set_n(true);
        self.regs.set_h((a & 0x0F) < (value & 0x0F));
        self.regs.set_c(a < value);
        self.regs.a = result;
    }

    fn sbc_a(&mut self, value: u8) {
        let a = self.regs.a;
        let carry = u8::from(self.regs.c());
        let result = a.wrapping_sub(value).wrapping_sub(carry);
        self.regs.set_z(result == 0);
        self.regs.set_n(true);
        self.regs.set_h((a & 0x0F) < (value & 0x0F) + carry);
        self.regs
            .set_c(u16::from(a) < u16::from(value) + u16::from(carry));
        self.regs.a = result;
    }

    fn and_a(&mut self, value: u8) {
        let result = self.regs.a & value;
        self.regs.a = result;
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(true);
        self.regs.set_c(false);
    }

    fn xor_a(&mut self, value: u8) {
        let result = self.regs.a ^ value;
        self.regs.a = result;
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(false);
    }

    fn or_a(&mut self, value: u8) {
        let result = self.regs.a | value;
        self.regs.a = result;
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(false);
    }

    fn cp_a(&mut self, value: u8) {
        let a = self.regs.a;
        self.regs.set_z(a == value);
        self.regs.set_n(true);
        self.regs.set_h((a & 0x0F) < (value & 0x0F));
        self.regs.set_c(a < value);
    }

    fn inc8(&mut self, value: u8) -> u8 {
        let result = value.wrapping_add(1);
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(value & 0x0F == 0x0F);
        result
    }

    fn dec8(&mut self, value: u8) -> u8 {
        let result = value.wrapping_sub(1);
        self.regs.set_z(result == 0);
        self.regs.set_n(true);
        self.regs.set_h(value & 0x0F == 0x00);
        result
    }

    fn rlca(&mut self) {
        let a = self.regs.a;
        let carry = a & 0x80 != 0;
        self.regs.a = a.rotate_left(1);
        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn rrca(&mut self) {
        let a = self.regs.a;
        let carry = a & 0x01 != 0;
        self.regs.a = a.rotate_right(1);
        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn rla(&mut self) {
        let a = self.regs.a;
        let carry = a & 0x80 != 0;
        self.regs.a = (a << 1) | u8::from(self.regs.c());
        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn rra(&mut self) {
        let a = self.regs.a;
        let carry = a & 0x01 != 0;
        self.regs.a = (a >> 1) | (u8::from(self.regs.c()) << 7);
        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn rlc(&mut self, value: u8) -> u8 {
        let carry = value & 0x80 != 0;
        let result = value.rotate_left(1);
        self.finish_rotate(result, carry);
        result
    }

    fn rrc(&mut self, value: u8) -> u8 {
        let carry = value & 0x01 != 0;
        let result = value.rotate_right(1);
        self.finish_rotate(result, carry);
        result
    }

    fn rl(&mut self, value: u8) -> u8 {
        let carry = value & 0x80 != 0;
        let result = (value << 1) | u8::from(self.regs.c());
        self.finish_rotate(result, carry);
        result
    }

    fn rr(&mut self, value: u8) -> u8 {
        let carry = value & 0x01 != 0;
        let result = (value >> 1) | (u8::from(self.regs.c()) << 7);
        self.finish_rotate(result, carry);
        result
    }

    fn sla(&mut self, value: u8) -> u8 {
        let carry = value & 0x80 != 0;
        let result = value << 1;
        self.finish_rotate(result, carry);
        result
    }

    fn sra(&mut self, value: u8) -> u8 {
        let carry = value & 0x01 != 0;
        let result = (value >> 1) | (value & 0x80);
        self.finish_rotate(result, carry);
        result
    }

    fn swap(&mut self, value: u8) -> u8 {
        let result = value.rotate_left(4);
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(false);
        result
    }

    fn srl(&mut self, value: u8) -> u8 {
        let carry = value & 0x01 != 0;
        let result = value >> 1;
        self.finish_rotate(result, carry);
        result
    }

    fn finish_rotate(&mut self, result: u8, carry: bool) {
        self.regs.set_z(result == 0);
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn bit(&mut self, value: u8, bit: u8) {
        self.regs.set_z(value & (1 << bit) == 0);
        self.regs.set_n(false);
        self.regs.set_h(true);
    }

    fn daa(&mut self) {
        let mut correction = 0u8;
        let mut carry = self.regs.c();

        if self.regs.h() || (!self.regs.n() && (self.regs.a & 0x0F) > 9) {
            correction |= 0x06;
        }
        if carry || (!self.regs.n() && self.regs.a > 0x99) {
            correction |= 0x60;
            carry = true;
        }

        self.regs.a = if self.regs.n() {
            self.regs.a.wrapping_sub(correction)
        } else {
            self.regs.a.wrapping_add(correction)
        };

        self.regs.set_z(self.regs.a == 0);
        self.regs.set_h(false);
        self.regs.set_c(carry);
    }

    fn cpl(&mut self) {
        self.regs.a = !self.regs.a;
        self.regs.set_n(true);
        self.regs.set_h(true);
    }

    fn scf(&mut self) {
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(true);
    }

    fn ccf(&mut self) {
        let carry = self.regs.c();
        self.regs.set_n(false);
        self.regs.set_h(false);
        self.regs.set_c(!carry);
    }

    fn add_sp_e8(&mut self, bus: &mut Bus) -> u32 {
        let offset = self.fetch_u8(bus);
        let sp = self.regs.sp;
        let low = sp as u8;

        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h((low & 0x0F) + (offset & 0x0F) > 0x0F);
        self.regs.set_c(u16::from(low) + u16::from(offset) > 0xFF);

        self.regs.sp = sp.wrapping_add(offset as i8 as i16 as u16);
        16
    }

    fn ld_hl_sp_e8(&mut self, bus: &mut Bus) -> u32 {
        let offset = self.fetch_u8(bus);
        let sp = self.regs.sp;
        let low = sp as u8;

        self.regs.set_z(false);
        self.regs.set_n(false);
        self.regs.set_h((low & 0x0F) + (offset & 0x0F) > 0x0F);
        self.regs.set_c(u16::from(low) + u16::from(offset) > 0xFF);

        let result = sp.wrapping_add(offset as i8 as i16 as u16);
        self.regs.set_hl(result);
        12
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    fn bus_with(program: &[u8]) -> Bus {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0100..0x0100 + program.len()].copy_from_slice(program);
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        rom[0x014D] = crate::cartridge::header::checksum(&rom).unwrap();
        let mut bus = Bus::new(Cartridge::new(rom).unwrap());
        bus.apply_post_boot_state();
        bus
    }

    fn run(program: &[u8], steps: usize) -> (Cpu, Bus) {
        let mut bus = bus_with(program);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        for _ in 0..steps {
            cpu.step(&mut bus);
        }
        (cpu, bus)
    }

    #[test]
    fn ld_rr_nn_sets_the_pair_little_endian() {
        let (cpu, _) = run(&[0x01, 0x34, 0x12], 1);
        assert_eq!(cpu.regs.bc(), 0x1234);
    }

    #[test]
    fn ld_r_r_copies_between_registers() {
        let (cpu, _) = run(&[0x01, 0x34, 0x12, 0x78], 2);
        assert_eq!(cpu.regs.a, 0x12);
    }

    #[test]
    fn ld_hl_pointer_writes_through_the_bus() {
        let (_, bus) = run(&[0x21, 0x00, 0xC0, 0x3E, 0x9A, 0x77], 3);
        assert_eq!(bus.read(0xC000), 0x9A);
    }

    #[test]
    fn add_sets_half_carry_and_carry() {
        let (cpu, _) = run(&[0x3E, 0xFF, 0xC6, 0x01], 2);
        assert_eq!(cpu.regs.a, 0x00);
        assert!(cpu.regs.z());
        assert!(cpu.regs.h());
        assert!(cpu.regs.c());
        assert!(!cpu.regs.n());
    }

    #[test]
    fn sub_sets_subtract_and_borrow_flags() {
        let (cpu, _) = run(&[0x3E, 0x10, 0xD6, 0x01], 2);
        assert_eq!(cpu.regs.a, 0x0F);
        assert!(cpu.regs.n());
        assert!(cpu.regs.h());
        assert!(!cpu.regs.c());
    }

    #[test]
    fn inc_preserves_carry_and_sets_half_carry_on_nibble_overflow() {
        let (cpu, _) = run(&[0x37, 0x3E, 0x0F, 0x3C], 3);
        assert_eq!(cpu.regs.a, 0x10);
        assert!(cpu.regs.h());
        assert!(cpu.regs.c(), "INC must not clear carry");
        assert!(!cpu.regs.n());
    }

    #[test]
    fn bit_leaves_carry_alone() {
        let (cpu, _) = run(&[0x37, 0x3E, 0x00, 0xCB, 0x47], 3);
        assert!(cpu.regs.z());
        assert!(cpu.regs.h());
        assert!(cpu.regs.c(), "BIT must not clear carry");
    }

    #[test]
    fn swap_zeroes_all_but_zero_flag() {
        let (cpu, _) = run(&[0x3E, 0xF0, 0xCB, 0x37], 2);
        assert_eq!(cpu.regs.a, 0x0F);
        assert!(!cpu.regs.c());
        assert!(!cpu.regs.h());
    }

    #[test]
    fn push_then_pop_restores_a_pair() {
        let (cpu, _) = run(&[0x01, 0xEF, 0xBE, 0xC5, 0xD1], 3);
        assert_eq!(cpu.regs.de(), 0xBEEF);
        assert_eq!(cpu.regs.sp, 0xFFFE);
    }

    #[test]
    fn pop_af_masks_the_low_flag_nibble() {
        let (cpu, _) = run(&[0x31, 0x00, 0xC0, 0x21, 0xFF, 0x00, 0xE5, 0xF1], 4);
        assert_eq!(cpu.regs.f & 0x0F, 0);
    }

    #[test]
    fn call_then_ret_round_trips() {
        let (cpu, _) = run(&[0xCD, 0x06, 0x01, 0x00, 0x00, 0x00, 0xC9], 2);
        assert_eq!(cpu.regs.pc, 0x0103, "RET must land on the byte after CALL");
        assert_eq!(cpu.regs.sp, 0xFFFE, "the stack must be balanced");
    }

    #[test]
    fn ret_cc_not_taken_costs_eight_cycles() {
        let mut bus = bus_with(&[0x37, 0xC0]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), 8);
    }

    #[test]
    fn jr_backwards_is_signed() {
        let (cpu, _) = run(&[0x00, 0x18, 0xFD], 2);
        assert_eq!(cpu.regs.pc, 0x0100);
    }

    #[test]
    fn add_sp_e8_uses_low_byte_carry() {
        let (cpu, _) = run(&[0x31, 0x0F, 0x00, 0xE8, 0x01], 2);
        assert_eq!(cpu.regs.sp, 0x0010);
        assert!(cpu.regs.h());
        assert!(!cpu.regs.c());
        assert!(!cpu.regs.z());
        assert!(!cpu.regs.n());
    }

    #[test]
    fn add_sp_e8_treats_the_offset_as_signed() {
        let (cpu, _) = run(&[0x31, 0x10, 0x00, 0xE8, 0xFF], 2);
        assert_eq!(cpu.regs.sp, 0x000F);
    }

    #[test]
    fn ldh_round_trips_through_high_memory() {
        let (cpu, bus) = run(&[0x3E, 0x42, 0xE0, 0x80, 0xF0, 0x80], 3);
        assert_eq!(bus.read(0xFF80), 0x42);
        assert_eq!(cpu.regs.a, 0x42);
    }

    #[test]
    fn di_clears_ime_immediately() {
        let mut bus = bus_with(&[0xFB, 0x00, 0xF3]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();

        cpu.step(&mut bus);
        assert!(!cpu.ime);
        cpu.step(&mut bus);
        assert!(cpu.ime);
        cpu.step(&mut bus);
        assert!(!cpu.ime);
    }

    #[test]
    fn halt_bug_triggers_when_ime_is_clear_and_an_irq_is_pending() {
        let mut bus = bus_with(&[0x76, 0x3E, 0x01]);
        bus.write(0xFFFF, 0x04);
        bus.write(0xFF0F, 0x04);

        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        cpu.step(&mut bus);

        assert!(!cpu.halted);
        assert_eq!(cpu.regs.pc, 0x0101);
    }

    #[test]
    fn interrupts_dispatch_to_the_vector_and_clear_the_flag() {
        let mut bus = bus_with(&[0x00]);
        bus.write(0xFFFF, 0x01);
        bus.write(0xFF0F, 0x01);

        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        cpu.ime = true;

        let cycles = cpu.step(&mut bus);
        assert_eq!(cycles, 20);
        assert_eq!(cpu.regs.pc, interrupt::VBLANK_VECTOR);
        assert_eq!(bus.iflag & interrupt::VBLANK, 0);
        assert!(!cpu.ime);
    }

    #[test]
    fn interrupt_priority_follows_the_bit_order() {
        let mut bus = bus_with(&[0x00]);
        bus.write(0xFFFF, 0x05);
        bus.write(0xFF0F, 0x05);

        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        cpu.ime = true;
        cpu.step(&mut bus);

        assert_eq!(cpu.regs.pc, interrupt::VBLANK_VECTOR);
        assert_ne!(bus.iflag & interrupt::TIMER, 0);
    }

    #[test]
    fn conditional_call_costs_more_when_taken() {
        let mut bus = bus_with(&[0x37, 0xDC, 0x06, 0x01, 0x00, 0x00, 0xC9]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();

        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), 24);
    }

    #[test]
    fn cb_hl_forms_cost_more_than_register_forms() {
        let mut bus = bus_with(&[0x21, 0x00, 0xC0, 0xCB, 0x06]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();

        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), 16);

        let mut bus = bus_with(&[0xCB, 0x00]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        assert_eq!(cpu.step(&mut bus), 8);
    }

    #[test]
    fn bit_hl_costs_less_than_other_hl_forms() {
        let mut bus = bus_with(&[0x21, 0x00, 0xC0, 0xCB, 0x46]);
        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();

        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), 12);
    }
}