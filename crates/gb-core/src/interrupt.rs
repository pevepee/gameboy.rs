//! Interrupt sources, their vectors, and the fixed hardware priority order.

/// Vertical blanking started.
pub const VBLANK: u8 = 1 << 0;
/// LCD controller entered a mode the STAT register is configured to signal.
pub const LCD_STAT: u8 = 1 << 1;
/// TIMA overflowed.
pub const TIMER: u8 = 1 << 2;
/// A serial byte finished shifting.
pub const SERIAL: u8 = 1 << 3;
/// A joypad line went low.
pub const JOYPAD: u8 = 1 << 4;

/// Every interrupt bit the hardware implements.
pub const ALL: u8 = VBLANK | LCD_STAT | TIMER | SERIAL | JOYPAD;

pub const VBLANK_VECTOR: u16 = 0x0040;
pub const LCD_STAT_VECTOR: u16 = 0x0048;
pub const TIMER_VECTOR: u16 = 0x0050;
pub const SERIAL_VECTOR: u16 = 0x0058;
pub const JOYPAD_VECTOR: u16 = 0x0060;

/// Hardware dispatch order. A lower bit number wins when several are pending.
pub const ORDER: [(u8, u16); 5] = [
    (VBLANK, VBLANK_VECTOR),
    (LCD_STAT, LCD_STAT_VECTOR),
    (TIMER, TIMER_VECTOR),
    (SERIAL, SERIAL_VECTOR),
    (JOYPAD, JOYPAD_VECTOR),
];

/// Highest-priority pending, enabled interrupt, as `(bit, vector)`.
///
/// Returns `None` when nothing is both requested and enabled.
#[must_use]
pub fn pending(iflag: u8, ie: u8) -> Option<(u8, u16)> {
    let active = iflag & ie & ALL;
    ORDER.iter().copied().find(|(bit, _)| active & bit != 0)
}
