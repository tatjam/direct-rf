use heapless::Vec;
use serde::{Deserialize, Serialize};

pub const MAX_SEQUENCE_LEN: usize = 12000;
pub const MAX_DIVN_CHANGES: usize = 32;

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct PLLChange {
    pub for_ticks: usize,
    pub start_tick: usize,
    pub divn: u16,
    pub vcosel: bool,
    pub divp: u8,
    // WARNING: Only us if timer prescaler is properly configured
    pub tim_us: u32,
}

#[derive(Default)]
pub struct Sequence {
    pub fracn_buffer: Vec<u16, MAX_SEQUENCE_LEN>,
    pub pllchange_buffer: Vec<PLLChange, MAX_DIVN_CHANGES>,
}

impl Sequence {
    // Only to be used on the non-firmware side!
    pub fn expensive_copy(&self) -> Self {
        let mut out = Sequence::default();
        out.fracn_buffer.clone_from(&self.fracn_buffer);
        out.pllchange_buffer.clone_from(&self.pllchange_buffer);

        out
    }
}
