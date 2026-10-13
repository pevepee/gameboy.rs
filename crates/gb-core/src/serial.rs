//! SB/SC link port.
//!
//! No cable is emulated. The register file exists because the Blargg test ROMs
//! report their results through it: write the byte to `SB`, then write `0x81` to
//! `SC` to shift it out.

/// SC bit 7: transfer in progress. Cleared once the byte is shifted out.
const SC_START: u8 = 1 << 7;

pub struct Serial {
    pub sb: u8,
    pub sc: u8,
    /// Everything the ROM has shifted out, in order.
    pub output: String,
}

impl Serial {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sb: 0,
            sc: 0,
            output: String::new(),
        }
    }

    pub fn write_sb(&mut self, value: u8) {
        self.sb = value;
    }

    /// Shift the pending byte out.
    ///
    /// The transfer completes instantly; the 8-cycle shift time a real link port
    /// takes only matters to ROMs that poll `SC`, and none that we test do.
    pub fn write_sc(&mut self, value: u8) {
        self.sc = value;
        if value & SC_START != 0 {
            self.output.push(self.sb as char);
            self.sc &= !SC_START;
        }
    }
}

impl Default for Serial {
    fn default() -> Self {
        Self::new()
    }
}
