//! The system bus: every CPU-visible address, and the peripherals behind it.

use crate::cartridge::Cartridge;
use crate::interrupt;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

/// Size of the DMG work RAM.
const WRAM_LEN: usize = 0x8000;
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

    apu_regs: [u8; 0x30], // Stub para os 48 registos de som (0xFF10 a 0xFF3F)

    wram: [u8; WRAM_LEN],
    wram_bank: u8,
    hram: [u8; HRAM_LEN],
    ie: u8,
    /// Interrupt request flags, as seen at `0xFF0F`.
    pub iflag: u8,

    /// Source address of the in-flight OAM DMA, `0xFF46 << 8`.
    dma_source: u16,
    /// T-states left in the in-flight OAM DMA.
    dma_remaining: u32,
    hdma_source: u16,
    hdma_destination: u16,
    hdma_blocks_remaining: u8,
    hdma_hblank: bool,
    hdma_active: bool,
    hdma_last_line: u8,
    hdma_stall_remaining: u32,
    key1: u8,
    ppu_cycle_remainder: u8,

    /// Writes to addresses with no backing register. A nonzero count on a
    /// passing test ROM means a peripheral is missing, not that the ROM is bad.
    pub unmapped_writes: u64,
}

impl Bus {
    /// Build a bus around a cartridge, with every peripheral in its reset state.
    #[must_use]
    pub fn new(cart: Cartridge) -> Self {
        let cgb_mode = cart.header.cgb_flag & 0x80 != 0;
        Self {
            cart,
            ppu: Ppu::with_cgb_mode(cgb_mode),
            timer: Timer::new(),
            joypad: Joypad::new(),
            serial: Serial::new(),
            apu_regs: [0; 0x30],
            wram: [0; WRAM_LEN],
            wram_bank: 1,
            hram: [0; HRAM_LEN],
            ie: 0,
            iflag: 0,
            dma_source: 0,
            dma_remaining: 0,
            hdma_source: 0,
            hdma_destination: 0x8000,
            hdma_blocks_remaining: 0,
            hdma_hblank: false,
            hdma_active: false,
            hdma_last_line: u8::MAX,
            hdma_stall_remaining: 0,
            key1: 0,
            ppu_cycle_remainder: 0,
            unmapped_writes: 0,
        }
    }

    /// Apply the peripheral state left by the boot ROM. Using this skips boot
    /// ROM emulation entirely; CPU registers are initialized by `GameBoy`.
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

    /// Whether an OAM or general-DMA transfer is currently stalling the CPU.
    #[must_use]
    pub fn dma_active(&self) -> bool {
        self.dma_remaining > 0 || self.hdma_stall_remaining > 0
    }

    /// Reads are blocked from OAM while a DMA is in flight.
    #[must_use]
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => {
                self.ppu.vram[usize::from(self.ppu.vram_bank)][(addr - 0x8000) as usize]
            }
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xDFFF => self.wram[self.wram_index(addr)],
            // Echo RAM mirrors work RAM.
            0xE000..=0xFDFF => self.wram[self.wram_index(addr - 0x2000)],
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
            0x8000..=0x9FFF => {
                self.ppu.vram[usize::from(self.ppu.vram_bank)][(addr - 0x8000) as usize] = value;
            }
            0xA000..=0xBFFF => self.cart.write_ram(addr, value),
            0xC000..=0xDFFF => {
                let index = self.wram_index(addr);
                self.wram[index] = value;
            }
            0xE000..=0xFDFF => {
                let index = self.wram_index(addr - 0x2000);
                self.wram[index] = value;
            }
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
            0xFF4D if self.ppu.cgb_mode => 0x7E | self.key1,
            0xFF4F if self.ppu.cgb_mode => 0xFE | self.ppu.vram_bank,
            0xFF51 if self.ppu.cgb_mode => (self.hdma_source >> 8) as u8,
            0xFF52 if self.ppu.cgb_mode => (self.hdma_source as u8 & 0xF0) | 0x0F,
            0xFF53 if self.ppu.cgb_mode => ((self.hdma_destination >> 8) as u8 & 0x1F) | 0xE0,
            0xFF54 if self.ppu.cgb_mode => (self.hdma_destination as u8 & 0xF0) | 0x0F,
            0xFF55 if self.ppu.cgb_mode => self.hdma_status(),
            0xFF68 if self.ppu.cgb_mode => self.ppu.bg_palette_index(),
            0xFF69 if self.ppu.cgb_mode => self.ppu.bg_palette_data(),
            0xFF6A if self.ppu.cgb_mode => self.ppu.obj_palette_index(),
            0xFF6B if self.ppu.cgb_mode => self.ppu.obj_palette_data(),
            0xFF70 if self.ppu.cgb_mode => 0xF8 | self.wram_bank,
            0xFF10..=0xFF3F => self.apu_regs[(addr - 0xFF10) as usize],
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
            0xFF4D if self.ppu.cgb_mode => self.key1 = (self.key1 & 0x80) | (value & 0x01),
            0xFF4F if self.ppu.cgb_mode => self.ppu.vram_bank = value & 0x01,
            0xFF51 if self.ppu.cgb_mode => {
                self.hdma_source = (u16::from(value) << 8) | (self.hdma_source & 0x00F0);
            }
            0xFF52 if self.ppu.cgb_mode => {
                self.hdma_source = (self.hdma_source & 0xFF00) | u16::from(value & 0xF0);
            }
            0xFF53 if self.ppu.cgb_mode => {
                self.hdma_destination =
                    0x8000 | (u16::from(value & 0x1F) << 8) | (self.hdma_destination & 0x00F0);
            }
            0xFF54 if self.ppu.cgb_mode => {
                self.hdma_destination = (self.hdma_destination & 0x9F00) | u16::from(value & 0xF0);
            }
            0xFF55 if self.ppu.cgb_mode => self.start_hdma(value),
            0xFF68 if self.ppu.cgb_mode => self.ppu.write_bg_palette_index(value),
            0xFF69 if self.ppu.cgb_mode => self.ppu.write_bg_palette_data(value),
            0xFF6A if self.ppu.cgb_mode => self.ppu.write_obj_palette_index(value),
            0xFF6B if self.ppu.cgb_mode => self.ppu.write_obj_palette_data(value),
            0xFF70 if self.ppu.cgb_mode => self.wram_bank = (value & 0x07).max(1),
            0xFF10..=0xFF3F => self.apu_regs[(addr - 0xFF10) as usize] = value,
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

    fn hdma_status(&self) -> u8 {
        if self.hdma_active {
            self.hdma_blocks_remaining.saturating_sub(1) & 0x7F
        } else if self.hdma_blocks_remaining > 0 {
            0x80 | (self.hdma_blocks_remaining - 1)
        } else {
            0xFF
        }
    }

    fn start_hdma(&mut self, value: u8) {
        if self.hdma_active {
            if self.hdma_hblank && value & 0x80 == 0 {
                self.hdma_active = false;
            }
            return;
        }

        self.hdma_blocks_remaining = (value & 0x7F) + 1;
        self.hdma_hblank = value & 0x80 != 0;
        self.hdma_active = true;
        self.hdma_last_line = u8::MAX;

        if !self.hdma_hblank {
            while self.hdma_blocks_remaining > 0 {
                self.transfer_hdma_block();
            }
        }
    }

    fn transfer_hdma_block(&mut self) {
        for offset in 0..16u16 {
            let source = self.hdma_source.wrapping_add(offset);
            let destination = self.hdma_destination.wrapping_add(offset);
            let value = self.read(source);
            let index = usize::from(destination.wrapping_sub(0x8000) & 0x1FFF);
            self.ppu.vram[usize::from(self.ppu.vram_bank)][index] = value;
        }

        self.hdma_source = self.hdma_source.wrapping_add(16);
        self.hdma_destination = 0x8000 | (self.hdma_destination.wrapping_add(16) & 0x1FF0);
        self.hdma_blocks_remaining -= 1;
        self.hdma_stall_remaining += if self.double_speed() { 64 } else { 32 };

        if self.hdma_blocks_remaining == 0 {
            self.hdma_active = false;
        }
    }

    fn wram_index(&self, addr: u16) -> usize {
        let offset = usize::from(addr & 0x0FFF);
        if addr < 0xD000 {
            offset
        } else {
            usize::from(self.wram_bank) * 0x1000 + offset
        }
    }

    #[must_use]
    pub fn double_speed(&self) -> bool {
        self.key1 & 0x80 != 0
    }

    pub fn switch_speed_if_prepared(&mut self) -> bool {
        if self.ppu.cgb_mode && self.key1 & 0x01 != 0 {
            self.key1 = (self.key1 ^ 0x80) & 0x80;
            true
        } else {
            false
        }
    }

    /// Advance every peripheral by `cycles` T-states and latch their interrupts.
    pub fn tick(&mut self, cycles: u32) {
        if self.dma_remaining > 0 {
            self.dma_remaining = self.dma_remaining.saturating_sub(cycles);
        }
        if self.hdma_stall_remaining > 0 {
            self.hdma_stall_remaining = self.hdma_stall_remaining.saturating_sub(cycles);
        }

        let ppu_cycles = if self.double_speed() {
            let total = cycles + u32::from(self.ppu_cycle_remainder);
            self.ppu_cycle_remainder = (total & 1) as u8;
            total / 2
        } else {
            self.ppu_cycle_remainder = 0;
            cycles
        };

        let ppu_irq = self.ppu.tick(ppu_cycles);
        self.iflag |= ppu_irq;

        if self.hdma_active
            && self.hdma_hblank
            && self.ppu.ly < 144
            && self.ppu.in_hblank()
            && self.hdma_last_line != self.ppu.ly
        {
            self.hdma_last_line = self.ppu.ly;
            self.transfer_hdma_block();
        }

        if self.timer.tick(cycles) {
            self.iflag |= interrupt::TIMER;
        }

        self.cart.tick(ppu_cycles);
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

    fn cgb_bus() -> Bus {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0x80;
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        rom[0x014D] = crate::cartridge::header::checksum(&rom).unwrap();
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

    #[test]
    fn cgb_registers_select_independent_vram_and_wram_banks() {
        let mut bus = cgb_bus();

        bus.write(0x8000, 0x12);
        bus.write(0xFF4F, 1);
        bus.write(0x8000, 0x34);
        assert_eq!(bus.read(0x8000), 0x34);
        bus.write(0xFF4F, 0);
        assert_eq!(bus.read(0x8000), 0x12);

        bus.write(0xD000, 0x56);
        bus.write(0xFF70, 2);
        bus.write(0xD000, 0x78);
        assert_eq!(bus.read(0xD000), 0x78);
        bus.write(0xFF70, 1);
        assert_eq!(bus.read(0xD000), 0x56);
    }

    #[test]
    fn cgb_speed_switch_clears_prepare_bit_and_toggles_speed() {
        let mut bus = cgb_bus();
        bus.write(0xFF4D, 1);

        assert!(bus.switch_speed_if_prepared());
        assert_eq!(bus.read(0xFF4D) & 0x81, 0x80);
        assert!(!bus.switch_speed_if_prepared());
    }

    #[test]
    fn cgb_general_dma_copies_aligned_blocks_and_stalls_cpu() {
        let mut bus = cgb_bus();
        for offset in 0..32u16 {
            bus.write(0xC000 + offset, offset as u8);
        }
        bus.write(0xFF51, 0xC0);
        bus.write(0xFF52, 0x00);
        bus.write(0xFF53, 0x00);
        bus.write(0xFF54, 0x00);
        bus.write(0xFF55, 0x01);

        for offset in 0..32u16 {
            assert_eq!(bus.read(0x8000 + offset), offset as u8);
        }
        assert_eq!(bus.read(0xFF55), 0xFF);
        assert!(bus.dma_active());
        bus.tick(64);
        assert!(!bus.dma_active());
    }

    #[test]
    fn cgb_hblank_dma_copies_one_block_in_hblank() {
        let mut bus = cgb_bus();
        for offset in 0..16u16 {
            bus.write(0xC000 + offset, (offset + 1) as u8);
        }
        bus.ppu.write_lcdc(0x91);
        bus.write(0xFF51, 0xC0);
        bus.write(0xFF52, 0x00);
        bus.write(0xFF53, 0x00);
        bus.write(0xFF54, 0x00);
        bus.write(0xFF55, 0x80);

        bus.tick(251);
        assert_eq!(bus.read(0x8000), 0);
        bus.tick(1);

        for offset in 0..16u16 {
            assert_eq!(bus.read(0x8000 + offset), (offset + 1) as u8);
        }
        assert_eq!(bus.read(0xFF55), 0xFF);
    }
}
