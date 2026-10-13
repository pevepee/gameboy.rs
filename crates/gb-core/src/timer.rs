//! DIV, TIMA, TMA and TAC.
//!
//! All four registers derive from one free-running 16-bit counter. `TAC` selects
//! which counter bit clocks TIMA, and `DIV` exposes the top byte, which is why
//! writing DIV also disturbs the TIMA clock. Modelling both from one counter is
//! what makes `instr_timing` and the `mem_timing` suite agree with hardware.

/// TAC bit 2: enable TIMA counting.
const TAC_ENABLE: u8 = 1 << 2;

/// Counter bit selected by each `TAC & 0b11` value.
const TAC_PERIOD_BITS: [u16; 4] = [9, 3, 5, 7];

pub struct Timer {
    /// Free-running divider, advanced one step per T-cycle.
    counter: u16,
    /// TIMA holds its reload value for four cycles before `tma` lands.
    reload_countdown: Option<u8>,
    /// Previous state of the selected counter bit, for edge detection.
    last_clock: bool,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
}

impl Timer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: 0,
            reload_countdown: None,
            last_clock: false,
            tima: 0,
            tma: 0,
            tac: 0,
        }
    }

    /// Apply the post-boot register state the boot ROM would have left behind.
    pub fn apply_boot_state(&mut self) {
        self.counter = 0xABCC;
        self.tima = 0;
        self.tma = 0;
        self.tac = 0;
        self.reload_countdown = None;
        self.last_clock = self.clock_bit(self.counter);
    }

    /// Current divider value as visible through `FF04`.
    #[must_use]
    pub fn div(&self) -> u8 {
        (self.counter >> 8) as u8
    }

    /// Writing DIV resets the divider, which can produce a TIMA edge.
    pub fn write_div(&mut self) {
        self.counter = 0;
        self.sample_clock();
    }

    /// Advance by `cycles` T-states. Returns `true` on the cycle TIMA overflow
    /// becomes visible, which is when the interrupt is raised.
    pub fn tick(&mut self, cycles: u32) -> bool {
        let mut overflowed = false;

        for _ in 0..cycles {
            if let Some(n) = self.reload_countdown.as_mut() {
                *n -= 1;
                if *n == 0 {
                    self.reload_countdown = None;
                    self.tima = self.tma;
                }
            }

            self.counter = self.counter.wrapping_add(1);
            if self.sample_clock() {
                let (next, wrapped) = self.tima.overflowing_add(1);
                self.tima = next;
                if wrapped {
                    self.reload_countdown = Some(4);
                    overflowed = true;
                }
            }
        }

        overflowed
    }

    /// Detect a falling edge on the counter bit `TAC` selects, and increment
    /// TIMA when one occurs.
    fn sample_clock(&mut self) -> bool {
        let now = self.clock_bit(self.counter);
        let edge = self.last_clock && !now;
        self.last_clock = now;
        edge
    }

    fn clock_bit(&self, counter: u16) -> bool {
        if self.tac & TAC_ENABLE == 0 {
            return false;
        }
        let bit = TAC_PERIOD_BITS[(self.tac & 0b11) as usize];
        counter & (1 << bit) != 0
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}
