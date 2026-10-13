//! P1/JOYP. The register is a 2x4 matrix: one select line at a time, active low.

/// JOYP bit 4. Writing 0 selects the d-pad row.
const SELECT_DPAD: u8 = 1 << 4;
/// JOYP bit 5. Writing 0 selects the button row.
const SELECT_BUTTONS: u8 = 1 << 5;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

impl Button {
    /// Bit index inside the internal pressed mask.
    const fn mask(self) -> u8 {
        match self {
            Button::Right => 1 << 0,
            Button::Left => 1 << 1,
            Button::Up => 1 << 2,
            Button::Down => 1 << 3,
            Button::A => 1 << 4,
            Button::B => 1 << 5,
            Button::Select => 1 << 6,
            Button::Start => 1 << 7,
        }
    }
}

pub struct Joypad {
    /// Only bits 4 and 5 are meaningful; the rest read back as 1.
    select: u8,
    /// Set bit means the button is held down.
    pressed: u8,
}

impl Joypad {
    #[must_use]
    pub fn new() -> Self {
        Self {
            select: 0x30,
            pressed: 0,
        }
    }

    /// Post-boot state: both rows deselected.
    pub fn apply_boot_state(&mut self) {
        *self = Self::new();
    }

    pub fn write_p1(&mut self, value: u8) {
        self.select = value & 0x30;
    }

    /// Active-low read. Unselected rows read as all ones.
    #[must_use]
    pub fn read_p1(&self) -> u8 {
        let mut value = 0xC0 | self.select;

        if self.select & SELECT_DPAD == 0 {
            for (line, button) in [
                (0, Button::Right),
                (1, Button::Left),
                (2, Button::Up),
                (3, Button::Down),
            ] {
                if self.pressed & button.mask() == 0 {
                    value |= 1 << line;
                }
            }
        }

        if self.select & SELECT_BUTTONS == 0 {
            for (line, button) in [
                (0, Button::A),
                (1, Button::B),
                (2, Button::Select),
                (3, Button::Start),
            ] {
                if self.pressed & button.mask() == 0 {
                    value |= 1 << line;
                }
            }
        }

        value
    }

    /// Returns `true` when this press transitions the line from released to held,
    /// which is when the JOYPAD interrupt is requested.
    pub fn set_pressed(&mut self, button: Button, down: bool) -> bool {
        let mask = button.mask();
        let was = self.pressed & mask != 0;

        if down {
            self.pressed |= mask;
        } else {
            self.pressed &= !mask;
        }

        down && !was
    }

    /// Buttons currently held, for the frontend to render or drive headless runs.
    #[must_use]
    pub fn is_pressed(&self, button: Button) -> bool {
        self.pressed & button.mask() != 0
    }
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}
