//! Cartridge: header, external RAM, and the memory bank controller.
//!
//! All four DMG-era controllers share one bank-switching surface, so this is a
//! single struct with a `kind` discriminant rather than a trait object. Keeping
//! the banking state inline also means save states can serialize the whole
//! cartridge without downcasting.

pub mod header;
pub mod mbc;

use std::fmt;

pub use header::{Header, HeaderError};
pub use mbc::MbcKind;

/// Size of a switchable ROM bank window.
pub const ROM_BANK_SIZE: usize = 0x4000;
/// Size of a RAM bank window.
pub const RAM_BANK_SIZE: usize = 0x2000;
/// MBC2 exposes 512 nibbles rather than full bytes.
const MBC2_RAM_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartridgeError(pub HeaderError);

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cartridge rejected: {}", self.0)
    }
}

impl std::error::Error for CartridgeError {}

pub struct Cartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    pub header: Header,
    kind: MbcKind,

    ram_enabled: bool,
    /// Low bank number as written by the ROM, before MBC1 mode adjustment.
    rom_bank: usize,
    ram_bank: usize,
    /// MBC1 mode select: 0 = 16/8 Mbit, 1 = 4/32 Mbit.
    banking_mode: u8,
    /// Upper ROM/RAM bank bits shared by MBC1 and MBC3.
    bank_hi: usize,

    /// MBC3 real-time clock registers, latched on demand.
    rtc: Rtc,
}

impl Cartridge {
    /// Build a cartridge from raw ROM bytes, validating the header checksum.
    ///
    /// # Errors
    /// Returns [`CartridgeError`] when the header cannot be parsed or the
    /// checksum does not match.
    pub fn new(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let header = Header::parse_checked(&rom).map_err(CartridgeError)?;
        Ok(Self::from_header(rom, header))
    }

    /// Build a cartridge from raw ROM bytes, accepting a bad header checksum.
    ///
    /// Used by the test harness: several Blargg builds carry a stale checksum.
    #[must_use]
    pub fn new_lossy(rom: Vec<u8>) -> Option<Self> {
        let header = Header::parse(&rom).ok()?;
        Some(Self::from_header(rom, header))
    }

    fn from_header(rom: Vec<u8>, header: Header) -> Self {
        let kind = MbcKind::from_cartridge_type(header.cartridge_type);
        let ram_len = if kind == MbcKind::Mbc2 {
            MBC2_RAM_LEN
        } else {
            header.ram_size()
        };

        Self {
            rom,
            ram: vec![0; ram_len],
            header,
            kind,
            ram_enabled: false,
            rom_bank: 1,
            ram_bank: 0,
            banking_mode: 0,
            bank_hi: 0,
            rtc: Rtc::new(),
        }
    }

    #[must_use]
    pub fn kind(&self) -> MbcKind {
        self.kind
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.header.title
    }

    /// Number of 16 KiB ROM banks actually present in the file.
    #[must_use]
    pub fn rom_banks(&self) -> usize {
        (self.rom.len() / ROM_BANK_SIZE).max(1)
    }

    /// Number of 8 KiB RAM banks actually present.
    #[must_use]
    pub fn ram_banks(&self) -> usize {
        (self.ram.len() / RAM_BANK_SIZE).max(1)
    }

    /// Read a byte from the ROM windows at `0x0000..=0x7FFF`.
    #[must_use]
    pub fn read_rom(&self, addr: u16) -> u8 {
        let bank = self.current_rom_bank(addr);
        let offset = if addr < 0x4000 {
            addr as usize
        } else {
            (addr as usize - 0x4000) + bank * ROM_BANK_SIZE
        };

        // A truncated or over-declared ROM must not panic; open bus reads 0xFF.
        self.rom.get(offset).copied().unwrap_or(0xFF)
    }

    /// Which ROM bank an address currently resolves to.
    fn current_rom_bank(&self, addr: u16) -> usize {
        let banks = self.rom_banks();

        if addr < 0x4000 {
            // Only MBC1 in mode 1 can remap the low window, and only when the
            // ROM is large enough for the upper bits to matter.
            if self.kind == MbcKind::Mbc1 && self.banking_mode == 1 && banks > 32 {
                (self.bank_hi << 5) % banks
            } else {
                0
            }
        } else {
            // Removido o `% banks` para permitir que leituras fora dos limites
            // falhem naturalmente e devolvam 0xFF (open bus).
            match self.kind {
                MbcKind::RomOnly => 1,
                MbcKind::Mbc1 => (self.bank_hi << 5) | self.rom_bank,
                MbcKind::Mbc2 | MbcKind::Mbc3 => self.rom_bank,
                MbcKind::Mbc5 => self.rom_bank,
            }
        }
    }

    /// Handle a write into the ROM window, which the controllers interpret as a
    /// bank-switch command rather than a memory store.
    pub fn write_rom(&mut self, addr: u16, value: u8) {
        match self.kind {
            MbcKind::RomOnly => {}
            MbcKind::Mbc1 => self.write_mbc1(addr, value),
            MbcKind::Mbc2 => self.write_mbc2(addr, value),
            MbcKind::Mbc3 => self.write_mbc3(addr, value),
            MbcKind::Mbc5 => self.write_mbc5(addr, value),
        }
    }

    fn write_mbc1(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,
            0x2000..=0x3FFF => {
                // Bank 0 is not addressable in the switchable window.
                let bank = (value & 0x1F) as usize;
                self.rom_bank = if bank == 0 { 1 } else { bank };
            }
            0x4000..=0x5FFF => self.bank_hi = (value & 0x03) as usize,
            0x6000..=0x7FFF => self.banking_mode = value & 0x01,
            _ => {}
        }
    }

    fn write_mbc2(&mut self, addr: u16, value: u8) {
        if addr & 0x0100 == 0 {
            self.ram_enabled = value & 0x0F == 0x0A;
        } else {
            let bank = (value & 0x0F) as usize;
            self.rom_bank = if bank == 0 { 1 } else { bank };
        }
    }

    fn write_mbc3(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,
            0x2000..=0x3FFF => {
                let bank = (value & 0x7F) as usize;
                self.rom_bank = if bank == 0 { 1 } else { bank };
            }
            0x4000..=0x5FFF => self.ram_bank = (value & 0x0F) as usize,
            0x6000..=0x7FFF => self.rtc.latch(value),
            _ => {}
        }
    }

    fn write_mbc5(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,
            0x2000..=0x2FFF => self.rom_bank = (self.rom_bank & 0x100) | value as usize,
            0x3000..=0x3FFF => {
                self.rom_bank = (self.rom_bank & 0x0FF) | ((value as usize & 0x01) << 8);
            }
            0x4000..=0x5FFF => self.ram_bank = (value & 0x0F) as usize,
            _ => {}
        }
    }

    /// Read external RAM at `0xA000..=0xBFFF`.
    #[must_use]
    pub fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled {
            return 0xFF;
        }

        let offset = addr as usize - 0xA000;

        if self.kind == MbcKind::Mbc2 {
            // MBC2 stores four bits per address and mirrors every 512 bytes.
            let byte = self.ram.get(offset % MBC2_RAM_LEN).copied().unwrap_or(0);
            return 0xF0 | (byte & 0x0F);
        }

        // MBC3 register 0x08..=0x0C selects an RTC register instead of RAM.
        if self.kind == MbcKind::Mbc3 && (0x08..=0x0C).contains(&self.ram_bank) {
            return self.rtc.read(self.ram_bank);
        }

        let index = self.ram_bank * RAM_BANK_SIZE + offset;
        self.ram.get(index).copied().unwrap_or(0xFF)
    }

    /// Write external RAM at `0xA000..=0xBFFF`.
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if !self.ram_enabled {
            return;
        }

        let offset = addr as usize - 0xA000;

        if self.kind == MbcKind::Mbc2 {
            let index = offset % MBC2_RAM_LEN;
            if let Some(slot) = self.ram.get_mut(index) {
                *slot = value & 0x0F;
            }
            return;
        }

        if self.kind == MbcKind::Mbc3 && (0x08..=0x0C).contains(&self.ram_bank) {
            self.rtc.write(self.ram_bank, value);
            return;
        }

        let index = self.ram_bank * RAM_BANK_SIZE + offset;
        if let Some(slot) = self.ram.get_mut(index) {
            *slot = value;
        }
    }

    /// Advance the real-time clock. `cycles` are T-states; 4_194_304 per second.
    pub fn tick(&mut self, cycles: u32) {
        if self.kind == MbcKind::Mbc3 {
            self.rtc.tick(cycles);
        }
    }

    /// Battery-backed contents, for writing a `.sav` alongside the ROM.
    #[must_use]
    pub fn save_ram(&self) -> &[u8] {
        &self.ram
    }

    /// Restore battery-backed contents.
    pub fn load_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }
}

/// MBC3's real-time clock: five counters plus a latch.
struct Rtc {
    seconds: u8,
    minutes: u8,
    hours: u8,
    days_low: u8,
    days_high: u8,
    /// Sub-second T-state accumulator.
    sub_second: u32,
    /// Latched copies, served while a latch is active.
    latched: [u8; 5],
}

impl Rtc {
    const CYCLES_PER_SECOND: u32 = 4_194_304;

    fn new() -> Self {
        Self {
            seconds: 0,
            minutes: 0,
            hours: 0,
            days_low: 0,
            days_high: 0,
            sub_second: 0,
            latched: [0; 5],
        }
    }

    fn tick(&mut self, cycles: u32) {
        self.sub_second += cycles;
        while self.sub_second >= Self::CYCLES_PER_SECOND {
            self.sub_second -= Self::CYCLES_PER_SECOND;
            self.advance_second();
        }
    }

    fn advance_second(&mut self) {
        self.seconds = self.seconds.wrapping_add(1);
        if self.seconds < 60 {
            return;
        }
        self.seconds = 0;
        self.minutes = self.minutes.wrapping_add(1);
        if self.minutes < 60 {
            return;
        }
        self.minutes = 0;
        self.hours = self.hours.wrapping_add(1);
        if self.hours < 24 {
            return;
        }
        self.hours = 0;

        // 9-bit day counter; bit 7 of days_high is the carry flag.
        let day = (((self.days_high & 1) as u16) << 8) | self.days_low as u16;
        let next = day.wrapping_add(1) & 0x1FF;
        self.days_low = (next & 0xFF) as u8;
        self.days_high = (self.days_high & 0xFE) | ((next >> 8) as u8 & 1);
        if next == 0 {
            self.days_high |= 0x80;
        }
    }

    /// Writing 0 then 1 to `0x6000..=0x7FFF` copies the live counters.
    fn latch(&mut self, value: u8) {
        if value != 0 {
            self.latched = [
                self.seconds,
                self.minutes,
                self.hours,
                self.days_low,
                self.days_high,
            ];
        }
    }

    /// Register numbers follow the mapper's convention: `0x08` seconds through
    /// `0x0C` for the day high byte, which also holds the carry flag.
    fn read(&self, register: usize) -> u8 {
        if (0x08..=0x0C).contains(&register) {
            return self.latched[register - 0x08];
        }
        0xFF
    }

    fn write(&mut self, register: usize, value: u8) {
        match register {
            0x08 => self.seconds = value % 60,
            0x09 => self.minutes = value % 60,
            0x0A => self.hours = value % 24,
            0x0B => self.days_low = value,
            0x0C => self.days_high = value & 0xC1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64 KiB MBC1 ROM with a valid checksum.
    fn mbc1_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x10000];
        rom[0x0147] = 0x01;
        rom[0x0148] = 0x01;
        rom[0x0149] = 0x02; // <-- Alterado de 0x00 para 0x02 para declarar RAM

        // Mark each bank so reads can be attributed.
        for bank in 0..4usize {
            rom[bank * ROM_BANK_SIZE] = bank as u8;
        }

        let checksum = header::checksum(&rom).unwrap();
        rom[0x014D] = checksum;
        rom
    }

    #[test]
    fn accepts_a_valid_header() {
        let cart = Cartridge::new(mbc1_rom()).unwrap();
        assert_eq!(cart.kind(), MbcKind::Mbc1);
        assert_eq!(cart.rom_banks(), 4);
    }

    #[test]
    fn bank_zero_is_unaddressable_in_the_switchable_window() {
        let mut cart = Cartridge::new(mbc1_rom()).unwrap();
        cart.write_rom(0x2000, 0x00);
        // A write of 0 must select bank 1, not bank 0.
        assert_eq!(cart.read_rom(0x4000), 1);
    }

    #[test]
    fn mbc1_selects_the_requested_bank() {
        let mut cart = Cartridge::new(mbc1_rom()).unwrap();
        cart.write_rom(0x2000, 0x03);
        assert_eq!(cart.read_rom(0x4000), 3);
    }

    #[test]
    fn ram_reads_as_open_bus_until_enabled() {
        let mut cart = Cartridge::new(mbc1_rom()).unwrap();
        assert_eq!(cart.read_ram(0xA000), 0xFF);

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x42);
        assert_eq!(cart.read_ram(0xA000), 0x42);
    }

    #[test]
    fn short_rom_does_not_panic_on_bank_switch() {
        let mut rom = mbc1_rom();
        rom.truncate(ROM_BANK_SIZE);
        let mut cart = Cartridge::new_lossy(rom).unwrap();
        cart.write_rom(0x2000, 0x07);
        // Bank 7 does not exist; the read must return open bus, not panic.
        assert_eq!(cart.read_rom(0x4000), 0xFF);
    }

    #[test]
    fn mbc2_ram_keeps_only_the_low_nibble() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x06;
        rom[0x0148] = 0x00;
        rom[0x014D] = header::checksum(&rom).unwrap();
        let mut cart = Cartridge::new(rom).unwrap();

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0xAB);
        assert_eq!(cart.read_ram(0xA000), 0xFB);
    }

    #[test]
    fn mbc5_allows_bank_zero_in_the_high_window() {
        let mut rom = vec![0u8; 0x20000];
        rom[0x0147] = 0x19;
        rom[0x0148] = 0x02;
        for bank in 0..8usize {
            rom[bank * ROM_BANK_SIZE] = bank as u8;
        }
        rom[0x014D] = header::checksum(&rom).unwrap();

        let mut cart = Cartridge::new(rom).unwrap();
        cart.write_rom(0x2000, 0x00);
        assert_eq!(cart.read_rom(0x4000), 0);
    }
}
