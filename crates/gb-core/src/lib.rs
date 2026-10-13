//! A cycle-driven emulator for the Game Boy and Game Boy Color.
//!
//! The public surface is one type, [`GameBoy`], plus the pieces you need to
//! drive it: [`Button`] for input and the framebuffer slice for video.
//!
//! ```no_run
//! use gb_core::GameBoy;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let rom = std::fs::read("tetris.gb")?;
//! let mut gb = GameBoy::new(rom)?;
//!
//! for _ in 0..600 {
//!     gb.run_frame();
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The PPU exposes both raw pixel indices and resolved RGB colors. The latter
//! uses the active CGB palettes when the cartridge is color-capable.

pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod interrupt;
pub mod joypad;
pub mod ppu;
pub mod serial;
pub mod timer;

pub use bus::Bus;
pub use cartridge::{Cartridge, CartridgeError};
pub use cpu::Cpu;
pub use joypad::Button;
pub use ppu::{HEIGHT, WIDTH};

/// T-states in one frame at the DMG's 59.7 Hz refresh rate.
pub const CYCLES_PER_FRAME: u32 = 70_224;

/// The whole machine.
pub struct GameBoy {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl GameBoy {
    /// Build a machine around a ROM, validating the cartridge header checksum.
    ///
    /// # Errors
    /// Returns [`CartridgeError`] when the ROM is too short to hold a header or
    /// its header checksum does not match.
    pub fn new(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let cart = Cartridge::new(rom)?;
        Ok(Self::from_cartridge(cart))
    }

    /// Build a machine around a ROM without validating the header checksum.
    ///
    /// Several test ROMs ship with a stale checksum, and some homebrew has none.
    /// Returns `None` only when the ROM is too small to hold a header at all.
    #[must_use]
    pub fn new_lossy(rom: Vec<u8>) -> Option<Self> {
        let cart = Cartridge::new_lossy(rom)?;
        Some(Self::from_cartridge(cart))
    }

    fn from_cartridge(cart: Cartridge) -> Self {
        let mut bus = Bus::new(cart);
        bus.apply_post_boot_state();

        let mut cpu = Cpu::new();
        cpu.apply_post_boot_state();
        if bus.ppu.cgb_mode {
            cpu.regs = crate::cpu::Registers::post_boot_cgb();
        }

        Self { cpu, bus }
    }

    /// Video memory as 2-bit color indices, one byte per pixel, row-major.
    #[must_use]
    pub fn framebuffer(&self) -> &[u8] {
        &self.bus.ppu.framebuffer
    }

    /// Resolved screen colors in `0x00RRGGBB` format, row-major.
    #[must_use]
    pub fn framebuffer_rgb(&self) -> &[u32] {
        &self.bus.ppu.framebuffer_rgb
    }

    /// Whether the cartridge is running with Game Boy Color hardware enabled.
    #[must_use]
    pub fn is_cgb_mode(&self) -> bool {
        self.bus.ppu.cgb_mode
    }

    /// Everything the ROM has written to the link port, in order.
    ///
    /// Test ROMs report their results through this channel, which is why it is
    /// part of the public surface rather than a debugging affordance.
    #[must_use]
    pub fn serial_output(&self) -> &str {
        &self.bus.serial.output
    }

    pub fn press(&mut self, button: Button) {
        if self.bus.joypad.set_pressed(button, true) {
            self.bus.iflag |= interrupt::JOYPAD;
        }
    }

    pub fn release(&mut self, button: Button) {
        self.bus.joypad.set_pressed(button, false);
    }

    /// Advance exactly one frame.
    ///
    /// Returns the number of T-states consumed.
    pub fn run_frame(&mut self) -> u32 {
        let mut spent = 0u32;

        while spent < CYCLES_PER_FRAME {
            spent += self.cpu.step(&mut self.bus);
            if self.bus.ppu.frame_ready {
                break;
            }
        }

        spent
    }

    /// Run one instruction. Exposed so tests can assert on exact timing.
    pub fn step(&mut self) -> u32 {
        self.cpu.step(&mut self.bus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::header;

    /// A minimal ROM: `LD A,0x42` then an infinite `JR -2`.
    fn tiny_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0100] = 0x3E;
        rom[0x0101] = 0x42;
        rom[0x0102] = 0x18;
        rom[0x0103] = 0xFE;
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        rom[0x014D] = header::checksum(&rom).unwrap();
        rom
    }

    #[test]
    fn new_rejects_a_bad_checksum() {
        let mut rom = tiny_rom();
        rom[0x014D] ^= 0xFF;
        assert!(GameBoy::new(rom).is_err());
    }

    #[test]
    fn new_lossy_accepts_a_bad_checksum() {
        let mut rom = tiny_rom();
        rom[0x014D] ^= 0xFF;
        assert!(GameBoy::new_lossy(rom).is_some());
    }

    #[test]
    fn run_frame_ends_at_the_vblank_boundary() {
        let mut gb = GameBoy::new(tiny_rom()).unwrap();
        let spent = gb.run_frame();

        // 144 visible lines at 456 dots each. The PPU is only ticked at
        // instruction granularity, so the count can overshoot by one instruction.
        let vblank_start = 144 * 456;
        assert!(
            (vblank_start..vblank_start + 24).contains(&spent),
            "expected {vblank_start}..{} cycles, got {spent}",
            vblank_start + 24
        );
    }

    #[test]
    fn ten_frames_accumulate_about_ten_frames_of_cycles() {
        let mut gb = GameBoy::new(tiny_rom()).unwrap();

        let mut total = 0u32;
        for _ in 0..10 {
            total += gb.run_frame();
        }

        // The first call stops at the VBlank boundary and therefore omits the
        // 10 VBlank lines, so allow exactly one frame of slack at the low end.
        let ten_frames = 10 * CYCLES_PER_FRAME;
        assert!(
            total > ten_frames - CYCLES_PER_FRAME && total <= ten_frames,
            "ten frames produced {total} cycles, expected within one frame of {ten_frames}"
        );
    }

    #[test]
    fn program_state_survives_a_frame() {
        let mut gb = GameBoy::new(tiny_rom()).unwrap();
        gb.step(); // LD A,0x42
        assert_eq!(gb.cpu.regs.a, 0x42);
        gb.run_frame();
        assert_eq!(gb.cpu.regs.a, 0x42, "the spin loop must not clobber A");
    }

    #[test]
    fn joypad_press_raises_the_interrupt_flag() {
        let mut gb = GameBoy::new(tiny_rom()).unwrap();
        gb.press(Button::A);
        assert_ne!(gb.bus.iflag & interrupt::JOYPAD, 0);
        assert!(gb.bus.joypad.is_pressed(Button::A));
    }

    #[test]
    fn framebuffer_is_screen_sized() {
        let gb = GameBoy::new(tiny_rom()).unwrap();
        assert_eq!(gb.framebuffer().len(), WIDTH * HEIGHT);
        assert_eq!(gb.framebuffer_rgb().len(), WIDTH * HEIGHT);
    }

    #[test]
    fn cgb_stop_switches_speed_when_key1_is_prepared() {
        let mut rom = tiny_rom();
        rom[header::CGB_FLAG] = 0x80;
        rom[0x0100..0x0104].copy_from_slice(&[0x10, 0x00, 0x10, 0x00]);
        rom[0x014D] = header::checksum(&rom).unwrap();
        let mut gb = GameBoy::new_lossy(rom).unwrap();

        gb.bus.write(0xFF4D, 1);
        gb.step();
        assert!(gb.bus.double_speed());
        assert!(!gb.cpu.stopped);

        gb.step();
        assert!(
            gb.cpu.stopped,
            "STOP without KEY1 prepare must stop the CPU"
        );
    }

    #[test]
    fn cgb_cartridge_selects_color_hardware_and_boot_registers() {
        let mut rom = tiny_rom();
        rom[header::CGB_FLAG] = 0x80;
        rom[0x014D] = header::checksum(&rom).unwrap();

        let gb = GameBoy::new(rom).unwrap();

        assert!(gb.is_cgb_mode());
        assert_eq!(gb.cpu.regs.af(), 0x1180);
        assert_eq!(gb.cpu.regs.bc(), 0x0000);
        assert_eq!(gb.cpu.regs.de(), 0xFF56);
        assert_eq!(gb.cpu.regs.hl(), 0x000D);
    }
}
