//! Browser bindings.
//!
//! The JavaScript side owns the canvas and the keyboard. This module exposes the
//! framebuffer as a raw pointer into WASM linear memory so the page can build an
//! `ImageData` view over it without a copy per frame.
//!
//! ```js
//! import init, { Emulator, screen_width, screen_height } from "./pkg/gb_wasm.js";
//!
//! await init();
//! const emu = new Emulator(new Uint8Array(await (await fetch("tetris.gb")).arrayBuffer()));
//! const ptr = emu.framebuffer_ptr();
//! const rgba = new Uint8ClampedArray(wasmMemory.buffer, ptr, screen_width() * screen_height() * 4);
//! ```

use gb_core::{Button, GameBoy, HEIGHT, WIDTH};
use wasm_bindgen::prelude::*;

/// Screen width in pixels. Exposed as a function because `const` does not cross
/// the WASM boundary.
#[wasm_bindgen]
#[must_use]
pub fn screen_width() -> usize {
    WIDTH
}

/// Screen height in pixels.
#[wasm_bindgen]
#[must_use]
pub fn screen_height() -> usize {
    HEIGHT
}

/// Button identifiers shared with the JavaScript key map.
#[wasm_bindgen]
#[derive(Copy, Clone)]
pub enum PadButton {
    Right = 0,
    Left = 1,
    Up = 2,
    Down = 3,
    A = 4,
    B = 5,
    Select = 6,
    Start = 7,
}

impl From<PadButton> for Button {
    fn from(value: PadButton) -> Self {
        match value {
            PadButton::Right => Button::Right,
            PadButton::Left => Button::Left,
            PadButton::Up => Button::Up,
            PadButton::Down => Button::Down,
            PadButton::A => Button::A,
            PadButton::B => Button::B,
            PadButton::Select => Button::Select,
            PadButton::Start => Button::Start,
        }
    }
}

#[wasm_bindgen]
pub struct Emulator {
    gb: GameBoy,
    /// RGBA8 copy of the framebuffer, rebuilt once per frame.
    ///
    /// The core produces one byte per pixel of color index; the browser needs
    /// RGBA. Keeping the expanded buffer here means the JS side never converts.
    rgba: Vec<u8>,
}

#[wasm_bindgen]
impl Emulator {
    /// Build an emulator from raw ROM bytes.
    ///
    /// # Errors
    /// Returns a JS error when the ROM is too small to hold a cartridge header.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsError> {
        let gb = GameBoy::new_lossy(rom.to_vec())
            .ok_or_else(|| JsError::new("ROM is too small to be a cartridge"))?;
        Ok(Self {
            gb,
            rgba: vec![0; WIDTH * HEIGHT * 4],
        })
    }

    /// Advance exactly one frame.
    pub fn run_frame(&mut self) {
        self.gb.run_frame();
        self.expand_framebuffer();
    }

    /// Pointer into WASM memory holding `width * height * 4` RGBA bytes.
    ///
    /// The pointer is stable for the lifetime of the emulator.
    #[must_use]
    pub fn framebuffer_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// Byte length of the RGBA framebuffer.
    #[must_use]
    pub fn framebuffer_len(&self) -> usize {
        self.rgba.len()
    }

    /// Press or release one button.
    pub fn set_button(&mut self, button: PadButton, down: bool) {
        let button = Button::from(button);
        if down {
            self.gb.press(button);
        } else {
            self.gb.release(button);
        }
    }

    /// Everything the ROM has written to the link port.
    #[must_use]
    pub fn serial_output(&self) -> String {
        self.gb.serial_output().to_string()
    }

    /// Cartridge title from the header.
    #[must_use]
    pub fn title(&self) -> String {
        self.gb.bus.cart.title().to_string()
    }

    /// Expand the 2-bit color indices into RGBA using the DMG LCD shades.
    fn expand_framebuffer(&mut self) {
        const PALETTE: [[u8; 3]; 4] = [
            [0x0F, 0x38, 0x0F],
            [0x30, 0x62, 0x30],
            [0x8B, 0xAC, 0x0F],
            [0x9B, 0xBC, 0x0F],
        ];

        for (index, &color) in self.gb.framebuffer().iter().enumerate() {
            let [r, g, b] = PALETTE[usize::from(color & 0b11)];
            let base = index * 4;
            self.rgba[base] = r;
            self.rgba[base + 1] = g;
            self.rgba[base + 2] = b;
            self.rgba[base + 3] = 0xFF;
        }
    }
}
