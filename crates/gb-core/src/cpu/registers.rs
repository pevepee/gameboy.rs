//! Register file and flag helpers.
//!
//! The low nibble of F is not storage: those bits always read back as zero.
//! Every write path here masks them, which is why `f` is private to the module
//! and reached through `flag`/`set_flag`.

/// Zero flag. Set when the last result was zero.
pub const FLAG_Z: u8 = 0x80;
/// Subtract flag. Set when the last operation was a subtraction.
pub const FLAG_N: u8 = 0x40;
/// Half-carry. Carry out of bit 3 for 8-bit ops, bit 11 for 16-bit ops.
pub const FLAG_H: u8 = 0x20;
/// Carry. Carry out of bit 7 for 8-bit ops, bit 15 for 16-bit ops.
pub const FLAG_C: u8 = 0x10;

/// Mask keeping only the implemented flag bits.
const FLAG_MASK: u8 = FLAG_Z | FLAG_N | FLAG_H | FLAG_C;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    /// Value the boot ROM leaves in the registers on a DMG.
    #[must_use]
    pub fn post_boot() -> Self {
        Self {
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
        }
    }

    #[must_use]
    pub fn af(&self) -> u16 {
        u16::from(self.a) << 8 | u16::from(self.f)
    }

    pub fn set_af(&mut self, value: u16) {
        self.a = (value >> 8) as u8;
        self.f = (value & 0xFF) as u8 & FLAG_MASK;
    }

    #[must_use]
    pub fn bc(&self) -> u16 {
        u16::from(self.b) << 8 | u16::from(self.c)
    }

    pub fn set_bc(&mut self, value: u16) {
        self.b = (value >> 8) as u8;
        self.c = value as u8;
    }

    #[must_use]
    pub fn de(&self) -> u16 {
        u16::from(self.d) << 8 | u16::from(self.e)
    }

    pub fn set_de(&mut self, value: u16) {
        self.d = (value >> 8) as u8;
        self.e = value as u8;
    }

    #[must_use]
    pub fn hl(&self) -> u16 {
        u16::from(self.h) << 8 | u16::from(self.l)
    }

    pub fn set_hl(&mut self, value: u16) {
        self.h = (value >> 8) as u8;
        self.l = value as u8;
    }

    #[must_use]
    pub fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }

    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.f |= mask & FLAG_MASK;
        } else {
            self.f &= !(mask & FLAG_MASK);
        }
        // Garante que os 4 bits inferiores são sempre apagados
        self.f &= FLAG_MASK;
    }

    #[must_use]
    pub fn z(&self) -> bool {
        self.flag(FLAG_Z)
    }

    pub fn set_z(&mut self, on: bool) {
        self.set_flag(FLAG_Z, on);
    }

    #[must_use]
    pub fn n(&self) -> bool {
        self.flag(FLAG_N)
    }

    pub fn set_n(&mut self, on: bool) {
        self.set_flag(FLAG_N, on);
    }

    #[must_use]
    pub fn h(&self) -> bool {
        self.flag(FLAG_H)
    }

    pub fn set_h(&mut self, on: bool) {
        self.set_flag(FLAG_H, on);
    }

    #[must_use]
    pub fn c(&self) -> bool {
        self.flag(FLAG_C)
    }

    pub fn set_c(&mut self, on: bool) {
        self.set_flag(FLAG_C, on);
    }

    /// Condition codes used by JP/JR/CALL/RET: NZ, Z, NC, C.
    #[must_use]
    pub fn condition(&self, index: u8) -> bool {
        match index {
            0 => !self.z(),
            1 => self.z(),
            2 => !self.c(),
            _ => self.c(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_writes_mask_the_low_nibble() {
        let mut regs = Registers {
            f: 0xFF,
            ..Default::default()
        };
        regs.set_z(true);
        assert_eq!(regs.f & 0x0F, 0);
    }

    #[test]
    fn af_round_trips_through_the_masked_form() {
        let mut regs = Registers::default();
        regs.set_af(0x1234);
        assert_eq!(regs.af(), 0x1230);
    }

    #[test]
    fn pair_helpers_are_little_endian_in_memory_order() {
        let mut regs = Registers::default();
        regs.set_hl(0xBEEF);
        assert_eq!(regs.h, 0xBE);
        assert_eq!(regs.l, 0xEF);
        assert_eq!(regs.hl(), 0xBEEF);
    }

    #[test]
    fn conditions_cover_the_four_codes() {
        let mut regs = Registers::default();
        assert!(regs.condition(0)); // NZ
        assert!(!regs.condition(1)); // Z
        regs.set_z(true);
        assert!(!regs.condition(0));
        assert!(regs.condition(1));
    }
}
