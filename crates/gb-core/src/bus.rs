//! The system bus: every CPU-visible address, and the peripherals behind it.

use crate::cartridge::Cartridge;
use crate::interrupt;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

/// Size of the DMG work RAM.
const WRAM_LEN: usize = 0x2000;
/// Size of high RAM.
const HRAM_LEN: usize = 0x7F;
/// T-states an OAM DMA transfer occupies.
const DMA_CYCLES: u32 = 160;

pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub timer: Timer,
    pub joypad: Joypad,
    pub serial: Serial,

    wram: [u8; WRAM_LEN],
    hram: [u8; HRAM_LEN],
    ie: u8,
    /// Interrupt request flags, as seen at `0xFF0F`.
    pub iflag: u8,

    /// Source address of the in-flight OAM DMA, `0xFF46 << 8`.
    dma_source: u16,
    /// T-states left in the in-flight OAM DMA.
    dma_remaining: u32,

    /// Writes to addresses with no backing register. A nonzero count on a
    /// passing test ROM means a peripheral is missing, not that the ROM is bad.
    pub unmapped_writes: u64,
}

impl Bus {
    /// Build a bus around a cartridge, with every peripheral in its reset state.
    #[must_use]
    pub fn new(cart: Cartridge) -> Self {
        Self {
            cart,
            ppu: Ppu::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            serial: Serial::new(),
            wram: [0; WRAM_LEN],
            hram: [0; HRAM_LEN],
            ie: 0,
            iflag: 0,
            dma_source: 0,
            dma_remaining: 0,
            unmapped_writes: 0,
        }
    }

    /// Apply the register state the boot ROM leaves behind, and start execution
    /// at `0x0100`. Using this skips boot ROM emulation entirely.
    ///
    /// The accumulator and flags are patched to the DMG values unless the header
    /// checksum is the one Nintendo's boot ROM compares against, in which case a
    /// DMG sets A=0x01 and F=0xB0 and a CGB sets A=0x11.
    pub fn apply_post_boot_state(&mut self) {
        self.ppu.apply_boot_state();
        self.timer.apply_boot_state();
        self.joypad.apply_boot_state();
        // The boot ROM leaves a pending VBlank request. Bits 5-7 are not
        // storage, so only the implemented bit is written here.
        self.iflag = interrupt::VBLANK;
        self.ie = 0x00;
        self.dma_remaining = 0;
    }

    #[must_use]
    pub fn ie(&self) -> u8 {
        self.ie
    }

    pub fn set_iflag(&mut self, value: u8) {
        self.iflag = value & interrupt::ALL;
    }

    /// Whether an OAM DMA is in flight, during which the CPU is stalled.
    #[must_use]
    pub fn dma_active(&self) -> bool {
        self.dma_remaining > 0
    }

    /// Reads are blocked from OAM while a DMA is in flight.
    #[must_use]
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.vram[(addr - 0x8000) as usize],
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            // Echo RAM mirrors work RAM.
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
            0xFE00..=0xFE9F if self.dma_active() => 0xFF,
            0xFE00..=0xFE9F => self.ppu.oam[(addr - 0xFE00) as usize],
            // The unusable region between OAM and IO reads as open bus.
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_rom(addr, value),
            0x8000..=0x9FFF => self.ppu.vram[(addr - 0x8000) as usize] = value,
            0xA000..=0xBFFF => self.cart.write_ram(addr, value),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,
            0xFE00..=0xFE9F if !self.dma_active() => {
                self.ppu.oam[(addr - 0xFE00) as usize] = value;
            }
            0xFE00..=0xFE9F => {}
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => self.write_io(addr, value),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            0xFFFF => self.ie = value & interrupt::ALL,
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => self.joypad.read_p1(),
            0xFF01 => self.serial.sb,
            0xFF02 => self.serial.sc | 0x7E,
            0xFF04 => self.timer.div(),
            0xFF05 => self.timer.tima,
            0xFF06 => self.timer.tma,
            0xFF07 => self.timer.tac | 0xF8,
            0xFF0F => self.iflag | 0xE0,
            0xFF40 => self.ppu.lcdc,
            0xFF41 => self.ppu.stat(),
            0xFF42 => self.ppu.scy,
            0xFF43 => self.ppu.scx,
            0xFF44 => self.ppu.ly,
            0xFF45 => self.ppu.lyc,
            0xFF46 => (self.dma_source >> 8) as u8,
            0xFF47 => self.ppu.bgp,
            0xFF48 => self.ppu.obp0,
            0xFF49 => self.ppu.obp1,
            0xFF4A => self.ppu.wy,
            0xFF4B => self.ppu.wx,
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF00 => self.joypad.write_p1(value),
            0xFF01 => self.serial.write_sb(value),
            0xFF02 => self.serial.write_sc(value),
            0xFF04 => self.timer.write_div(),
            0xFF05 => self.timer.tima = value,
            0xFF06 => self.timer.tma = value,
            0xFF07 => self.timer.tac = value & 0x07,
            0xFF0F => self.iflag = value & interrupt::ALL,
            0xFF40 => self.ppu.write_lcdc(value),
            0xFF41 => self.ppu.write_stat(value),
            0xFF42 => self.ppu.scy = value,
            0xFF43 => self.ppu.scx = value,
            // LY is read-only.
            0xFF44 => {}
            0xFF45 => self.ppu.write_lyc(value),
            0xFF46 => self.start_oam_dma(value),
            0xFF47 => self.ppu.bgp = value,
            0xFF48 => self.ppu.obp0 = value,
            0xFF49 => self.ppu.obp1 = value,
            0xFF4A => self.ppu.wy = value,
            0xFF4B => self.ppu.wx = value,
            _ => self.unmapped_writes += 1,
        }
    }

    /// `0xFF46` copies 160 bytes from `value << 8` into OAM.
    ///
    /// The copy itself is instantaneous; the 160-cycle stall that follows is
    /// modelled so that ROMs polling for DMA completion see the delay.
    fn start_oam_dma(&mut self, value: u8) {
        self.dma_source = u16::from(value) << 8;
        let base = self.dma_source as usize;

        for offset in 0..160usize {
            // Source reads go through the normal map, so a DMA from ROM or from
            // the echo region behaves as the hardware does.
            let byte = self.read((base + offset) as u16);
            self.ppu.oam[offset] = byte;
        }

        self.dma_remaining = DMA_CYCLES;
    }

    /// Advance every peripheral by `cycles` T-states and latch their interrupts.
    pub fn tick(&mut self, cycles: u32) {
        if self.dma_remaining > 0 {
            self.dma_remaining = self.dma_remaining.saturating_sub(cycles);
        }

        let ppu_irq = self.ppu.tick(cycles);
        self.iflag |= ppu_irq;

        if self.timer.tick(cycles) {
            self.iflag |= interrupt::TIMER;
        }

        self.cart.tick(cycles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::ROM_BANK_SIZE;

    fn bus() -> Bus {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        let checksum = crate::cartridge::header::checksum(&rom).unwrap();
        rom[0x014D] = checksum;
        Bus::new(Cartridge::new(rom).unwrap())
    }

    #[test]
    fn work_ram_echoes_into_the_echo_region() {
        let mut bus = bus();
        bus.write(0xC123, 0x9A);
        assert_eq!(bus.read(0xE123), 0x9A);
    }

    #[test]
    fn unusable_region_reads_as_open_bus() {
        let bus = bus();
        assert_eq!(bus.read(0xFEA0), 0xFF);
    }

    #[test]
    fn ie_register_masks_to_implemented_bits() {
        let mut bus = bus();
        bus.write(0xFFFF, 0xFF);
        assert_eq!(bus.ie(), interrupt::ALL);
    }

    #[test]
    fn if_register_masks_to_implemented_bits() {
        let mut bus = bus();
        bus.write(0xFF0F, 0xFF);
        assert_eq!(bus.iflag, interrupt::ALL);
    }

    #[test]
    fn oam_dma_copies_one_hundred_and_sixty_bytes() {
        let mut bus = bus();
        for offset in 0..160u16 {
            bus.write(0xC000 + offset, offset as u8);
        }
        bus.write(0xFF46, 0xC0);

        for offset in 0..160usize {
            assert_eq!(bus.ppu.oam[offset], offset as u8);
        }
        assert!(bus.dma_active());
    }

    #[test]
    fn oam_reads_are_blocked_during_dma() {
        let mut bus = bus();
        bus.write(0xFF46, 0xC0);
        assert_eq!(bus.read(0xFE00), 0xFF);
    }

    #[test]
    fn dma_stall_expires_after_the_transfer_length() {
        let mut bus = bus();
        bus.write(0xFF46, 0xC0);
        bus.tick(DMA_CYCLES);
        assert!(!bus.dma_active());
    }

    #[test]
    fn cart_rom_window_is_reachable_through_the_bus() {
        let mut rom = vec![0u8; ROM_BANK_SIZE * 2];
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        rom[0x014D] = crate::cartridge::header::checksum(&rom).unwrap();
        rom[0x3FFF] = 0x77;

        let bus = Bus::new(Cartridge::new(rom).unwrap());
        assert_eq!(bus.read(0x3FFF), 0x77);
    }
}
