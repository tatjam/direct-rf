/*
    The sequencer uses DMA to drive the PLL at desired frequencies automatically, without any
    software effects on timing. The programmed sequence has some limitations due to this.

    To do so, a single buffer is used. The DMA writes fracn to the PLL from this buffer, driven
    at the desired frequency very precisely. The fracn changes are carried up to the divn change,
    at which point the transfer interrupt is triggered, the software changes divn, possibly delays,
    and launches the DMA pointing to the new buffer chunk.
 */
use core::cell::{Ref, RefCell};
use core::mem::MaybeUninit;
use core::ops::Div;
use core::ptr::{null, null_mut};
use cortex_m::interrupt::Mutex;
use stm32h7::{stm32h7s};
use heapless::Vec;
use cortex_m::singleton;

use serde::de::Unexpected::Seq;
use stm32h7::stm32h7s::gpdma::CH;
use stm32h7::stm32h7s::{interrupt, Interrupt};

static SEQUENCER_STATE: Mutex<RefCell<MaybeUninit<SequencerState>>> = Mutex::new(RefCell::new(MaybeUninit::uninit()));

// Do not change, code depends on this interrupt being called!
const DMA_CH: usize = 0;

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
    tim: stm32h7s::TIM1,
    dma: stm32h7s::GPDMA,
}

fn prepare_fracn_dma(state: &mut SequencerState) {
    let divn_change = &state.divn_buffer[state.divni];
    assert!(divn_change.start_tick < state.divn_buffer.len());
    assert!(divn_change.start_tick + divn_change.for_ticks < state.divn_buffer.len());
    assert!(state.dma.ch(DMA_CH).cr().read().en().bit_is_clear());

    // Set the start address and size to copy for the DMA run
    let buff_ptr = state.fracn_buffer.as_ptr();
    let start_addr = (buff_ptr as usize + divn_change.start_tick * 2) as u32;
    state.dma.ch(DMA_CH).sar().write(|w| unsafe{ w.sa().bits(start_addr) });
    state.dma.ch(DMA_CH).br1().write(|w| unsafe { w.bndt().bits(divn_change.for_ticks as u16) });
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

#[inline]
pub fn with_state<F, R>(state: &Mutex<RefCell<MaybeUninit<SequencerState>>>, f: F) -> R
where
    F: FnOnce(&mut SequencerState) -> R,
{
    return cortex_m::interrupt::free(|cs| {
        unsafe {
            f(&mut state.borrow(cs).borrow_mut().assume_init_mut())
        }
    });
}

// We take full ownership of a DMA and a timer
pub fn setup(rcc: &mut stm32h7s::RCC, tim: stm32h7s::TIM1, dma: stm32h7s::GPDMA)
             -> &'static Mutex<RefCell<MaybeUninit<SequencerState>>> {
    // Setup basic DMA
    rcc.ahb1enr().write(|w| w.gpdma1en().enabled());

    // Trigger DMA on TIM1

    // Setup interrupts
    unsafe {
        cortex_m::peripheral::NVIC::unmask(Interrupt::GPDMA1_CH0);
    }

    cortex_m::interrupt::free( |cs| {
        SEQUENCER_STATE.borrow(cs).replace(MaybeUninit::new(SequencerState{
            fracn_buffer: Vec::new(),
            divn_buffer: Vec::new(),
            divni: 0,
            tim,
            dma,
        }));
    });

    return &SEQUENCER_STATE;
}

#[interrupt]
unsafe fn GPDMA1_CH0() {
    with_state(&SEQUENCER_STATE, |state| {

    });
}