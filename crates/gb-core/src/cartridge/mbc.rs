//! Memory bank controller selection from the cartridge type byte.

/// How a cartridge maps its ROM and RAM into the CPU address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbcKind {
    /// No mapper: 32 KiB ROM, optional 8 KiB RAM, no bank switching.
    RomOnly,
    /// MBC1: up to 2 MiB ROM, 32 KiB RAM, two banking modes.
    Mbc1,
    /// MBC2: up to 256 KiB ROM, 512 x 4 bits of built-in RAM.
    Mbc2,
    /// MBC3: up to 2 MiB ROM, 32 KiB RAM, optional real-time clock.
    Mbc3,
    /// MBC5: up to 8 MiB ROM, 128 KiB RAM, 9-bit ROM bank number.
    Mbc5,
}

impl MbcKind {
    /// Map a header cartridge-type byte onto a controller.
    ///
    /// Types whose controller is not implemented fall back to [`MbcKind::RomOnly`]
    /// so that a homebrew ROM with an unusual header still boots.
    #[must_use]
    pub fn from_cartridge_type(byte: u8) -> Self {
        match byte {
            0x00 | 0x08 | 0x09 => Self::RomOnly,
            0x01..=0x03 => Self::Mbc1,
            0x05 | 0x06 => Self::Mbc2,
            0x0F..=0x13 => Self::Mbc3,
            0x19..=0x1E => Self::Mbc5,
            _ => Self::RomOnly,
        }
    }

    /// Whether the cartridge type byte implies battery-backed RAM.
    #[must_use]
    pub fn has_battery(byte: u8) -> bool {
        matches!(byte, 0x03 | 0x06 | 0x09 | 0x0D | 0x0F | 0x10 | 0x13 | 0x1B | 0x1E)
    }

    /// Number of 16 KiB ROM banks the mapper can address.
    #[must_use]
    pub fn max_rom_banks(self) -> usize {
        match self {
            Self::RomOnly => 2,
            Self::Mbc1 | Self::Mbc3 => 128,
            Self::Mbc2 => 16,
            Self::Mbc5 => 512,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cart_type_maps_to_controller() {
        assert_eq!(MbcKind::from_cartridge_type(0x00), MbcKind::RomOnly);
        assert_eq!(MbcKind::from_cartridge_type(0x01), MbcKind::Mbc1);
        assert_eq!(MbcKind::from_cartridge_type(0x06), MbcKind::Mbc2);
        assert_eq!(MbcKind::from_cartridge_type(0x11), MbcKind::Mbc3);
        assert_eq!(MbcKind::from_cartridge_type(0x1B), MbcKind::Mbc5);
    }

    #[test]
    fn unknown_types_degrade_to_rom_only() {
        assert_eq!(MbcKind::from_cartridge_type(0xFF), MbcKind::RomOnly);
        assert_eq!(MbcKind::from_cartridge_type(0x22), MbcKind::RomOnly);
    }

    #[test]
    fn battery_bit_is_detected() {
        assert!(MbcKind::has_battery(0x03));
        assert!(!MbcKind::has_battery(0x02));
    }
}
