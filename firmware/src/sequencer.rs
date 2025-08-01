/*
    The sequencer uses DMA to drive the PLL at desired frequencies automatically, without any
    software effects on timing. The programmed sequence has some limitations due to this.

    To do so, a single buffer is used. The DMA writes fracn to the PLL from this buffer, driven
    at the desired frequency very precisely. The fracn changes are carried up to the divn change,
    at which point the transfer interrupt is triggered, the software changes divn, possibly delays,
    and launches the DMA pointing to the new buffer chunk.
 */
use core::ops::Div;
use stm32h7::{stm32h7s};
use heapless::Vec;
use cortex_m::singleton;

use serde::de::Unexpected::Seq;

const MAX_SEQUENCE_LEN: usize = 128;
const MAX_DIVN_CHANGES: usize = 32;

struct DivnChange {
    for_ticks: usize,
    start_tick: usize,
    divn: u16,
}

pub struct SequencerState {
    fracn_buffer: Vec<u16, MAX_SEQUENCE_LEN>,
    divn_buffer: Vec<DivnChange, MAX_DIVN_CHANGES>,
    divni: usize,
}

fn prepare_fracn_dma(state: &mut SequencerState) {
    let divn_change = &state.divn_buffer[state.divni];
    assert!(divn_change.start_tick < state.divn_buffer.len());
    assert!(divn_change.start_tick + divn_change.for_ticks < state.divn_buffer.len());



}

fn step(state: &mut SequencerState) {
    state.divni = state.divni + 1;
    if state.divni == state.divn_buffer.len() {
        // We ran out of the buffer, restart
        state.divni = 0;
    }

    prepare_fracn_dma(state);
}

fn set_dma_timer(state: &mut SequencerState) {

}

pub fn launch(state: &mut SequencerState) {
    stop(state);
    set_dma_timer(state);
}

pub fn stop(state: &mut SequencerState) {

}

pub fn setup(periph: &mut stm32h7s::Peripherals) -> &'static mut SequencerState {
    // Setup basic DMA
    periph.RCC.ahb1enr().write(|w| w.gpdma1en().enabled());

    // Setup interrupts

    return singleton!(: SequencerState = SequencerState {
        divn_buffer: Vec::new(),
        fracn_buffer: Vec::new(),
        divni: 0,
    }).unwrap();
}