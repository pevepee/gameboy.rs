//! PPU registers, VRAM/OAM storage and a scanline renderer.
//!
//! Mode timing here is line-granular: the dot counter advances through modes 2,
//! 3 and 0 inside a 456-dot line, but the pixels for a line are produced in one
//! pass at the line boundary rather than through a FIFO. That is enough to
//! render correctly and to drive STAT/LYC interrupts, but it is *not* enough
//! for the `ppu` timing tests and it does not emulate mid-scanline register
//! writes. See `docs/ARCHITECTURE.md` for the FIFO rewrite plan.

use crate::interrupt;

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 144;

/// Dots in one scanline.
const DOTS_PER_LINE: u32 = 456;
/// Dot at which mode 3 ends and HBlank opens. Drawing is not modelled dot by
/// dot, so this is a fixed offset rather than a function of the sprite count.
const HBLANK_START: u32 = 252;
/// Last visible line index, inclusive.
const LAST_VISIBLE_LINE: u8 = HEIGHT as u8 - 1;
/// Last VBlank line index, inclusive.
const LAST_VBLANK_LINE: u8 = 153;

const MODE_HBLANK: u8 = 0;
const MODE_VBLANK: u8 = 1;
const MODE_OAM: u8 = 2;

/// LCDC bit 7: LCD and PPU enabled.
const LCDC_ENABLE: u8 = 1 << 7;
/// LCDC bit 6: window tile map select.
const LCDC_WINDOW_MAP: u8 = 1 << 6;
/// LCDC bit 5: window enable.
const LCDC_WINDOW_ENABLE: u8 = 1 << 5;
/// LCDC bit 4: BG and window tile data select.
const LCDC_TILE_DATA: u8 = 1 << 4;
/// LCDC bit 3: BG tile map select.
const LCDC_BG_MAP: u8 = 1 << 3;
/// LCDC bit 2: OBJ size (0 = 8x8, 1 = 8x16).
const LCDC_OBJ_SIZE: u8 = 1 << 2;
/// LCDC bit 1: OBJ enable.
const LCDC_OBJ_ENABLE: u8 = 1 << 1;

/// STAT bit 2: LY equals LYC.
const STAT_LYC_EQ: u8 = 1 << 2;
/// STAT bit 3: HBlank interrupt select.
const STAT_MODE_HBLANK_IRQ: u8 = 1 << 3;
/// STAT bit 4: VBlank interrupt select.
const STAT_MODE_VBLANK_IRQ: u8 = 1 << 4;
/// STAT bit 5: OAM scan interrupt select.
const STAT_MODE_OAM_IRQ: u8 = 1 << 5;
/// STAT bit 6: LYC interrupt select.
const STAT_LYC_IRQ: u8 = 1 << 6;

pub struct Ppu {
    pub lcdc: u8,
    /// Mode interrupt selects plus the unused bit 7. Mode bits 0-1 are read-only.
    stat_select: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,

    pub vram: [u8; 0x2000],
    pub oam: [u8; 0xA0],

    /// One byte per pixel holding the raw 2-bit color index, not a shade.
    /// The frontend applies the DMG green palette at display time.
    pub framebuffer: [u8; WIDTH * HEIGHT],

    /// Position inside the current scanline, in dots.
    dot: u32,
    mode: u8,
    /// Window line counter. Advances only on lines the window actually draws.
    window_line: u8,
    /// Set for one `tick` immediately after a frame completes.
    pub frame_ready: bool,
}

impl Ppu {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lcdc: 0,
            stat_select: 0,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0,
            obp0: 0,
            obp1: 0,
            wy: 0,
            wx: 0,
            vram: [0; 0x2000],
            oam: [0; 0xA0],
            framebuffer: [0; WIDTH * HEIGHT],
            dot: 0,
            mode: MODE_OAM,
            window_line: 0,
            frame_ready: false,
        }
    }

    pub fn write_lyc(&mut self, value: u8) {
        self.lyc = value;
    }

    pub fn apply_boot_state(&mut self) {
        self.lcdc = 0x91;
        self.stat_select = 0x80;
        self.scy = 0;
        self.scx = 0;
        self.ly = 0;
        self.lyc = 0;
        self.bgp = 0xFC;
        self.obp0 = 0xFF;
        self.obp1 = 0xFF;
        self.wy = 0;
        self.wx = 0;
        self.dot = 0;
        self.mode = MODE_OAM;
        self.window_line = 0;
        self.frame_ready = false;
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.lcdc & LCDC_ENABLE != 0
    }

    #[must_use]
    pub fn mode(&self) -> u8 {
        self.mode
    }

    /// STAT as the CPU reads it: mode in bits 0-1, LYC flag in bit 2.
    #[must_use]
    pub fn stat(&self) -> u8 {
        let lyc = if self.ly == self.lyc { STAT_LYC_EQ } else { 0 };
        self.stat_select | 0x80 | lyc | (self.mode & 0b11)
    }

    pub fn write_stat(&mut self, value: u8) {
        self.stat_select = value & 0x78;
    }

    /// Switch the LCD on or off, resetting the line counter on either edge.
    pub fn write_lcdc(&mut self, value: u8) {
        let was_on = self.enabled();
        self.lcdc = value;
        let now_on = self.enabled();

        if was_on != now_on {
            self.ly = 0;
            self.dot = 0;
            self.window_line = 0;
            self.mode = if now_on { MODE_OAM } else { MODE_HBLANK };
        }
    }

    /// Advance by `cycles` dots. Returns the interrupt bits to raise.
    pub fn tick(&mut self, cycles: u32) -> u8 {
        self.frame_ready = false;

        if !self.enabled() {
            return 0;
        }

        let mut irq = 0;
        let mut remaining = cycles;

        while remaining > 0 {
            let step = remaining.min(DOTS_PER_LINE - self.dot);
            self.dot += step;
            remaining -= step;

            // Mode 3 has a fixed length here, so HBlank opens at the same dot on
            // every visible line and the mode 0 interrupt stays usable.
            if self.mode == MODE_OAM && self.dot >= HBLANK_START {
                self.mode = MODE_HBLANK;
                if self.ly <= LAST_VISIBLE_LINE && self.stat_select & STAT_MODE_HBLANK_IRQ != 0 {
                    irq |= interrupt::LCD_STAT;
                }
            }

            if self.dot == DOTS_PER_LINE {
                self.dot = 0;
                irq |= self.end_line();
            }
        }

        irq
    }

    /// Finish the current scanline, render it when visible, and open the next.
    fn end_line(&mut self) -> u8 {
        if self.ly <= LAST_VISIBLE_LINE {
            self.render_scanline(self.ly);
        }

        self.ly = self.ly.wrapping_add(1);

        if self.ly == HEIGHT as u8 {
            self.mode = MODE_VBLANK;
            // The visible area is complete, so a frontend may show the frame
            // without waiting for the rest of VBlank.
            self.frame_ready = true;
        } else if self.ly > LAST_VBLANK_LINE {
            self.ly = 0;
            self.window_line = 0;
            self.mode = MODE_OAM;
        } else if self.ly <= LAST_VISIBLE_LINE {
            self.mode = MODE_OAM;
        }

        let mut irq = 0;
        match self.mode {
            MODE_VBLANK => {
                if self.ly == HEIGHT as u8 {
                    irq |= interrupt::VBLANK;
                }
                if self.stat_select & STAT_MODE_VBLANK_IRQ != 0 {
                    irq |= interrupt::LCD_STAT;
                }
            }
            MODE_OAM if self.stat_select & STAT_MODE_OAM_IRQ != 0 => {
                irq |= interrupt::LCD_STAT;
            }
            _ => {}
        }

        if self.ly == self.lyc && self.stat_select & STAT_LYC_IRQ != 0 {
            irq |= interrupt::LCD_STAT;
        }

        irq
    }

    /// True once the dot counter has consumed dot `HBLANK_START` of the
    /// current line, which is where mode 3 ends and HBlank begins.
    #[must_use]
    pub fn in_hblank(&self) -> bool {
        self.dot >= HBLANK_START
    }

    fn render_scanline(&mut self, line: u8) {
        let mut bg_color_index = [0u8; WIDTH];
        self.render_bg_line(line, &mut bg_color_index);

        if self.render_window_line(line, &mut bg_color_index) {
            self.window_line = self.window_line.wrapping_add(1);
        }

        self.render_obj_line(line, &bg_color_index);
    }

    fn render_bg_line(&mut self, line: u8, color_out: &mut [u8; WIDTH]) {
        let row = line.wrapping_add(self.scy);
        let map_base = if self.lcdc & LCDC_BG_MAP != 0 {
            0x1C00
        } else {
            0x1800
        };

        for (x, slot) in color_out.iter_mut().enumerate() {
            let col = (x as u8).wrapping_add(self.scx);
            let tile = self.tile_index(map_base, row, col);
            let bits = self.tile_row_bits(tile, col, row);
            *slot = bits;
            self.framebuffer[line as usize * WIDTH + x] = self.apply_palette(bits, self.bgp);
        }
    }

    /// Returns `true` when this line drew window pixels, so the caller can
    /// advance the window line counter.
    fn render_window_line(&mut self, line: u8, color_out: &mut [u8; WIDTH]) -> bool {
        if self.lcdc & LCDC_WINDOW_ENABLE == 0 || self.wy > line {
            return false;
        }
        // WX is offset by 7: WX = 7 places window pixel 0 at screen x 0.
        let start = self.wx.saturating_sub(7) as usize;
        if start >= WIDTH {
            return false;
        }

        let map_base = if self.lcdc & LCDC_WINDOW_MAP != 0 {
            0x1C00
        } else {
            0x1800
        };
        let row = self.window_line;

        for (x, slot) in color_out.iter_mut().enumerate().skip(start) {
            let col = (x - start) as u8;
            let tile = self.tile_index(map_base, row, col);
            let bits = self.tile_row_bits(tile, col, row);
            *slot = bits;
            self.framebuffer[line as usize * WIDTH + x] = self.apply_palette(bits, self.bgp);
        }

        true
    }

    fn render_obj_line(&mut self, line: u8, bg_color: &[u8; WIDTH]) {
        if self.lcdc & LCDC_OBJ_ENABLE == 0 {
            return;
        }

        let tall = self.lcdc & LCDC_OBJ_SIZE != 0;
        let height: i16 = if tall { 16 } else { 8 };

        // At most ten sprites are considered per line, in OAM order.
        let mut chosen: [usize; 10] = [0; 10];
        let mut count = 0usize;

        for index in 0..40usize {
            if count == 10 {
                break;
            }
            let base = index * 4;
            let sprite_y = self.oam[base] as i16 - 16;
            if (line as i16) < sprite_y || (line as i16) >= sprite_y + height {
                continue;
            }
            chosen[count] = index;
            count += 1;
        }

        // Painter's order: a lower OAM index wins, so draw the tail first.
        for &index in chosen[..count].iter().rev() {
            let base = index * 4;
            let sprite_x = self.oam[base + 1] as i16 - 8;
            if sprite_x <= -8 || sprite_x >= WIDTH as i16 {
                continue;
            }

            let raw_tile = self.oam[base + 2];
            let attr = self.oam[base + 3];
            let sprite_y = self.oam[base] as i16 - 16;

            let flip_y = attr & 0x40 != 0;
            let flip_x = attr & 0x20 != 0;
            let behind_bg = attr & 0x80 != 0;
            let palette = if attr & 0x10 != 0 {
                self.obp1
            } else {
                self.obp0
            };

            let tile = if tall { raw_tile & 0xFE } else { raw_tile };

            let mut row = (line as i16 - sprite_y) as u8;
            if flip_y {
                row = (height as u8 - 1) - row;
            }

            for px in 0..8u8 {
                let screen_x = sprite_x + px as i16;
                if !(0..WIDTH as i16).contains(&screen_x) {
                    continue;
                }
                let sx = screen_x as usize;

                let bit = if flip_x { 7 - px } else { px };
                let bits = self.tile_row_bits(tile, bit, row);

                // Color 0 is transparent for sprites.
                if bits == 0 {
                    continue;
                }
                // BG-to-OBJ priority: with bit 7 set the sprite hides behind
                // any non-zero BG color index.
                if behind_bg && bg_color[sx] != 0 {
                    continue;
                }

                self.framebuffer[line as usize * WIDTH + sx] = self.apply_palette(bits, palette);
            }
        }
    }

    /// Tile map entry for a position in the 256x256 background map.
    fn tile_index(&self, map_base: usize, row: u8, col: u8) -> u8 {
        let map_x = (col / 8) as usize;
        let map_y = (row / 8) as usize;
        self.vram[map_base + map_y * 32 + map_x]
    }

    /// Two-bit color index for one pixel of a tile row.
    ///
    /// `row` is relative to the tile, not to the screen. An 8x16 sprite passes
    /// 0-15 here, so rows 8-15 read into the tile that follows — which is how
    /// the hardware stores a tall sprite.
    ///
    /// In signed tile data mode (LCDC bit 4 clear) the index is read as `i8` and
    /// offsets from `0x9000`, which lands in VRAM 0x0800-0x17F0.
    fn tile_row_bits(&self, tile: u8, col: u8, row: u8) -> u8 {
        let base = if self.lcdc & LCDC_TILE_DATA != 0 {
            tile as usize * 16
        } else {
            (0x1000i32 + (tile as i8 as i32) * 16) as usize
        };

        let r = row as usize;
        let low = self.vram[base + r * 2];
        let high = self.vram[base + r * 2 + 1];
        let bit = 7 - (col % 8);

        (((high >> bit) & 1) << 1) | ((low >> bit) & 1)
    }

    fn apply_palette(&self, color: u8, palette: u8) -> u8 {
        (palette >> (color * 2)) & 0b11
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}
