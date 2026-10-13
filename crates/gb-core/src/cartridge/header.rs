//! Cartridge header parsing at `0x0100..=0x014F`.

use std::fmt;
use std::ops::Range;

/// Bytes covered by the header checksum at `0x014D`.
pub const CHECKSUM_RANGE: Range<usize> = 0x0134..0x014D;
/// Title field, 16 bytes on DMG-era carts and shorter on later ones.
pub const TITLE_RANGE: Range<usize> = 0x0134..0x0144;
/// CGB capability flag at `0x0143`, which overlaps the title on old carts.
pub const CGB_FLAG: usize = 0x0143;

const LICENSOR_NEW: usize = 0x0144;
const CARTRIDGE_TYPE: usize = 0x0147;
const ROM_SIZE_CODE: usize = 0x0148;
const RAM_SIZE_CODE: usize = 0x0149;
const HEADER_CHECKSUM: usize = 0x014D;

/// Smallest ROM the header may describe.
const MIN_ROM_LEN: usize = 0x0150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// The file is too short to contain a header at all.
    TooShort { len: usize },
    /// The stored header checksum does not match the computed one.
    ChecksumMismatch { stored: u8, computed: u8 },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderError::TooShort { len } => {
                write!(f, "ROM is {len} bytes, need at least {MIN_ROM_LEN}")
            }
            HeaderError::ChecksumMismatch { stored, computed } => write!(
                f,
                "header checksum mismatch: stored {stored:#04X}, computed {computed:#04X}"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub cartridge_type: u8,
    pub rom_size_code: u8,
    pub ram_size_code: u8,
    pub cgb_flag: u8,
    pub new_licensor: [u8; 2],
}

impl Header {
    /// Parse the header fields without validating the checksum.
    ///
    /// # Errors
    /// Returns [`HeaderError::TooShort`] when `rom` cannot hold a header.
    pub fn parse(rom: &[u8]) -> Result<Self, HeaderError> {
        if rom.len() < MIN_ROM_LEN {
            return Err(HeaderError::TooShort { len: rom.len() });
        }

        Ok(Self {
            title: parse_title(rom),
            cartridge_type: rom[CARTRIDGE_TYPE],
            rom_size_code: rom[ROM_SIZE_CODE],
            ram_size_code: rom[RAM_SIZE_CODE],
            cgb_flag: rom[CGB_FLAG],
            new_licensor: [rom[LICENSOR_NEW], rom[LICENSOR_NEW + 1]],
        })
    }

    /// Parse and reject a ROM whose header checksum is wrong.
    ///
    /// # Errors
    /// Returns [`HeaderError`] for a short ROM or a checksum mismatch.
    pub fn parse_checked(rom: &[u8]) -> Result<Self, HeaderError> {
        let header = Self::parse(rom)?;
        let computed = checksum(rom).expect("length was checked by parse");
        let stored = rom[HEADER_CHECKSUM];

        if computed != stored {
            return Err(HeaderError::ChecksumMismatch { stored, computed });
        }

        Ok(header)
    }

    /// ROM size in bytes, derived from the size code.
    #[must_use]
    pub fn rom_size(&self) -> usize {
        32 * 1024 << self.rom_size_code.min(8)
    }

    /// External RAM size in bytes, derived from the size code.
    #[must_use]
    pub fn ram_size(&self) -> usize {
        match self.ram_size_code {
            0x00 => 0,
            0x01 => 2 * 1024, // MBC2's built-in nibble RAM
            0x02 => 8 * 1024,
            0x03 => 32 * 1024,
            0x04 => 128 * 1024,
            0x05 => 64 * 1024,
            _ => 0,
        }
    }
}

/// DMG header checksum over `0x0134..=0x014C`.
///
/// Returns `None` when the slice is too short to cover the range.
#[must_use]
pub fn checksum(rom: &[u8]) -> Option<u8> {
    let bytes = rom.get(CHECKSUM_RANGE)?;
    let mut x: u8 = 0;
    for &byte in bytes {
        x = x.wrapping_sub(byte).wrapping_sub(1);
    }
    Some(x)
}

/// Title bytes, trimmed at the first NUL and at the CGB flag byte.
fn parse_title(rom: &[u8]) -> String {
    let raw = rom.get(TITLE_RANGE).unwrap_or(&[]);
    // On CGB carts byte 0x0143 is the flag, not part of the title.
    let end = if rom[CGB_FLAG] & 0x80 != 0 {
        raw.len() - 1
    } else {
        raw.len()
    };

    let text: String = raw[..end]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                ' '
            }
        })
        .collect();

    text.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header whose fields are all zero except the pieces under test.
    ///
    /// Short titles are right-padded, because the title field is a fixed 16-byte
    /// window and `copy_from_slice` requires exact lengths.
    fn synthetic(title: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        let width = TITLE_RANGE.len();
        let used = title.len().min(width);
        let start = TITLE_RANGE.start;
        rom[start..start + used].copy_from_slice(&title[..used]);
        rom
    }

    #[test]
    fn checksum_is_self_consistent() {
        let mut rom = synthetic(b"TEST");
        let computed = checksum(&rom).unwrap();
        rom[HEADER_CHECKSUM] = computed;
        assert!(Header::parse_checked(&rom).is_ok());
    }

    #[test]
    fn checksum_rejects_corruption() {
        let mut rom = synthetic(b"TEST");
        rom[HEADER_CHECKSUM] = checksum(&rom).unwrap();
        rom[0x0135] ^= 0xFF;
        assert!(matches!(
            Header::parse_checked(&rom),
            Err(HeaderError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn short_rom_is_rejected() {
        assert!(matches!(
            Header::parse(&[0u8; 16]),
            Err(HeaderError::TooShort { .. })
        ));
    }

    #[test]
    fn title_stops_at_nul() {
        let rom = synthetic(b"POKEMON\0\0\0\0\0\0\0\0\0");
        assert_eq!(Header::parse(&rom).unwrap().title, "POKEMON");
    }

    #[test]
    fn rom_size_follows_the_code() {
        let mut rom = synthetic(b"X");
        rom[ROM_SIZE_CODE] = 0;
        assert_eq!(Header::parse(&rom).unwrap().rom_size(), 32 * 1024);
        rom[ROM_SIZE_CODE] = 4;
        assert_eq!(Header::parse(&rom).unwrap().rom_size(), 512 * 1024);
    }
}
