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

struct PLLChange {
    for_ticks: usize,
    start_tick: usize,
    divn: u16,
    vcosel: bool,
    output_pre: u8,
    tim_count: u16,
}

pub struct SequencerState {
    fracn_buffer: Vec<u16, MAX_SEQUENCE_LEN>,
    pllchange_buffer: Vec<PLLChange, MAX_DIVN_CHANGES>,
    pllchangei: usize,
    tim: stm32h7s::TIM2,
    dma: stm32h7s::GPDMA,
    rcc: stm32h7s::RCC,
}

fn set_pllchange(state: &mut SequencerState) {
    let change = &state.pllchange_buffer[state.pllchangei];

    assert!(state.rcc.cr().read().pll2on().bit_is_clear());
}

fn setup_pll(state: &mut SequencerState) {

    // Output PLL on MCO2, dividing the PLL VCO freq as convenient
    state.rcc.cfgr().modify(|_, w| w.mco2().pll2_p().mco2pre().set(1));
    state.rcc.pllcfgr().modify(|_, w| w.divp2en().enabled());

    // Input clock is HSE, which is 24MHz, and we drive the PLL
    // with 12MHz, because it's outside the band of interest and
    // is overall a pretty nice number (its divisible by 1, 2, 3, 4, 6 and 12)
    // which allows us to obtain neat round frequencies without the ΣΔ modulator.
    state.rcc.pllckselr().modify(|_, w| w.divm2().set(2).pllsrc().hse());
    state.rcc.pllcfgr().modify(|_, w| w.pll2rge().range8());

    // Use the 150 to 420MHz VCO for default settings
    state.rcc.pllcfgr().modify(|_, w| w.pll2vcosel().set_bit());

    // 12 MHz of reference are multiplied by 20 to get 240MHz on the VCO,
    // which are then divided by 30 to get 8Mhz on the p output
    state.rcc.pll2divr1().modify(|_, w| unsafe { w.divn2().bits(20 - 1).divp().bits(30-1) });

    // Enable PLL and wait for ready TODO: REMOVE
    state.rcc.cr().modify(|_, w| w.pll2on().set_bit());

    // Wait for PLL ready
    while state.rcc.cr().read().pll2rdy().bit_is_clear() {}

    defmt::info!("PLL2 is ready!");


}

fn prepare_fracn_dma(state: &mut SequencerState) {
    let change = &state.pllchange_buffer[state.pllchangei];

    assert!(change.start_tick < state.pllchange_buffer.len());
    assert!(change.start_tick + change.for_ticks < state.pllchange_buffer.len());
    assert!(state.dma.ch(DMA_CH).cr().read().en().bit_is_clear());

    // Set the start address and size to copy for the DMA run
    let buff_ptr = state.fracn_buffer.as_ptr();
    let start_addr = (buff_ptr as usize + change.start_tick * 2) as u32;
    state.dma.ch(DMA_CH).sar().modify(|_, w| unsafe{ w.sa().bits(start_addr) });
    state.dma.ch(DMA_CH).br1().modify(|_, w| unsafe { w.bndt().bits(change.for_ticks as u16) });
}

fn step(state: &mut SequencerState) {
    state.pllchangei = state.pllchangei + 1;
    if state.pllchangei == state.pllchange_buffer.len() {
        // We ran out of the buffer, restart
        state.pllchangei = 0;
    }

    // Disable the PLL and output
    state.rcc.pllcfgr().modify(|_, w| w.divp2en().disabled());
    state.rcc.cr().modify(|_, w| w.pll2on().clear_bit());

    // Prepare fracn for next run
    prepare_fracn_dma(state);

    set_dma_timer(state);

    // Configure PLL divn
    set_pllchange(state);

    // Enable PLL
    state.rcc.cr().modify(|_, w| w.pll2on().set_bit());

    // Wait for PLL ready
    while state.rcc.cr().read().pll2rdy().bit_is_clear() {}

    // Enable outputs
    state.rcc.pllcfgr().modify(|_, w| w.divp1en().enabled());
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
    cortex_m::interrupt::free(|cs| {
        unsafe {
            f(&mut state.borrow(cs).borrow_mut().assume_init_mut())
        }
    })
}

pub fn setup(rcc: stm32h7s::RCC, tim: stm32h7s::TIM2, dma: stm32h7s::GPDMA)
             -> &'static Mutex<RefCell<MaybeUninit<SequencerState>>> {
    // Setup basic DMA
    rcc.ahb1enr().modify(|_, w| w.gpdma1en().enabled());

    // Setup TIM2
    rcc.apb1lenr().modify(|_, w| w.tim2en().set_bit());

    // Drive TIM2 with HSE, for reduced jitter

    // Trigger DMA on tim2_trgo rising edge
    dma.ch(0).tr2().modify(|_, w| unsafe {
        w.trigpol().bits(1).trigsel().bits(47)
    });

    // Point DMA to write to RCC fracn
    let rcc_addr = rcc.pll2fracr().as_ptr() as u32;
    dma.ch(0).dar().modify(|_, w| unsafe {
        w.da().bits(rcc_addr)
    });

    // Setup interrupts
    dma.ch(0).cr().modify(|_, w| w.tcie().set_bit());
    unsafe {
        cortex_m::peripheral::NVIC::unmask(Interrupt::GPDMA1_CH0);
    }

    // The interrupt should not run now as TIM is disabled, but the guard is needed
    cortex_m::interrupt::free( |cs| {
        SEQUENCER_STATE.borrow(cs).replace(MaybeUninit::new(SequencerState{
            fracn_buffer: Vec::new(),
            pllchange_buffer: Vec::new(),
            pllchangei: 0,
            rcc,
            tim,
            dma,
        }));
    });

    with_state(&SEQUENCER_STATE, |state| {
        setup_pll(state);
    });

    &SEQUENCER_STATE
}

#[interrupt]
unsafe fn GPDMA1_CH0() {
    with_state(&SEQUENCER_STATE, |state| {
        if state.dma.ch(0).sr().read().tcf().bit_is_clear() {
            return;
        }

        step(state);
        state.dma.ch(0).fcr().write(|w| w.tcf().set_bit());
    });
}