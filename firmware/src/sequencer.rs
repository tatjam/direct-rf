/*
   The sequencer uses TIM to drive the PLL at desired frequencies automatically, without any
   significant software effects on timing.
*/
use crate::util;
use crate::util::InterruptAccessible;
use common::sequence::{PLLChange, Sequence};
use core::mem::MaybeUninit;
use heapless::Vec;
use stm32h7::stm32h7s;
use stm32h7::stm32h7s::{Interrupt, interrupt};

static SEQUENCER_STATE: InterruptAccessible<SequencerState> = InterruptAccessible::new();

pub struct DoubleBuffer<T: Default> {
    buffers: [T; 2],
    active_0: bool,
}

impl<T: Default> DoubleBuffer<T> {
    fn get_active(&self) -> &T {
        if self.active_0 {
            &self.buffers[0]
        } else {
            &self.buffers[1]
        }
    }

    fn get_back(&mut self) -> &mut T {
        if self.active_0 {
            &mut self.buffers[0]
        } else {
            &mut self.buffers[1]
        }
    }

    fn new() -> Self {
        Self {
            buffers: Default::default(),
            active_0: true,
        }
    }
}

pub struct SequencerState {
    seqs: DoubleBuffer<Sequence>,
    pllchangei: isize,
    tim: stm32h7s::TIM2,
    rcc: stm32h7s::RCC,

    fracn_rem: usize,
    fracn_i: usize,

    is_running: bool,

    uploading: bool,
}

fn set_pllchange(state: &mut SequencerState) {
    assert!(state.rcc.cr().read().pll2on().bit_is_clear());
    let change = &state.seqs.get_active().pllchange_buffer[state.pllchangei as usize];

    // Output PLL on MCO2, dividing the PLL VCO freq as convenient
    state
        .rcc
        .cfgr()
        .modify(|_, w| w.mco2().pll2_p().mco2pre().set(1));
    state.rcc.pllcfgr().modify(|_, w| w.divp2en().enabled());

    state
        .rcc
        .pll2divr1()
        .modify(|_, w| unsafe { w.divn2().bits(change.divn).divp().bits(change.divp) });

    // Set the fracn initial value
    let fracn0 = state.seqs.get_active().fracn_buffer[change.start_tick];
    state
        .rcc
        .pllcfgr()
        .modify(|_, w| w.pll2fracen().clear_bit());
    state
        .rcc
        .pll2fracr()
        .modify(|_, w| unsafe { w.fracn().bits(fracn0) });
    state.rcc.pllcfgr().modify(|_, w| w.pll2fracen().set_bit());

    // TODO: We must wait 5us for stability, do so, or not?

    state.rcc.cr().modify(|_, w| w.pll2on().set_bit());

    // Wait for PLL ready
    while state.rcc.cr().read().pll2rdy().bit_is_clear() {}
}

fn setup_pll(state: &mut SequencerState) {
    // Output PLL on MCO2, dividing the PLL VCO freq as convenient
    state
        .rcc
        .cfgr()
        .modify(|_, w| w.mco2().pll2_p().mco2pre().set(1));
    state.rcc.pllcfgr().modify(|_, w| w.divp2en().enabled());

    // Input clock is HSE, which is 24MHz, and we drive the PLL
    // with 12MHz, because it's outside the band of interest and
    // is overall a pretty nice number (its divisible by 1, 2, 3, 4, 6 and 12)
    // which allows us to obtain neat round frequencies without the ΣΔ modulator.
    state
        .rcc
        .pllckselr()
        .modify(|_, w| w.divm2().set(2).pllsrc().hse());
    state.rcc.pllcfgr().modify(|_, w| w.pll2rge().range8());

    // Use the 150 to 420MHz VCO for default settings
    state.rcc.pllcfgr().modify(|_, w| w.pll2vcosel().set_bit());

    // 12 MHz of reference are multiplied by 20 to get 240MHz on the VCO,
    // which are then divided by 30 to get 8Mhz on the p output
    state
        .rcc
        .pll2divr1()
        .modify(|_, w| unsafe { w.divn2().bits(20 - 1).divp().bits(30 - 1) });

    defmt::info!("PLL2 is ready to go!");
}

fn step(state: &mut SequencerState) {
    assert!(state.is_running);

    state.pllchangei += 1;
    assert!(state.pllchangei >= 0);

    if state.pllchangei as usize == state.seqs.get_active().pllchange_buffer.len() {
        // Swap the buffer right now, if it's still being uploaded we panic!
        assert!(!state.uploading);
        state.seqs.active_0 = !state.seqs.active_0;
    }

    // Disable TIM
    state.tim.cr1().modify(|_, w| w.cen().disabled());

    // Disable the PLL and output
    state.rcc.pllcfgr().modify(|_, w| w.divp2en().disabled());
    state.rcc.cr().modify(|_, w| w.pll2on().clear_bit());

    // Configure PLL divn (and initial fracn)
    set_pllchange(state);

    // Enable PLL
    state.rcc.cr().modify(|_, w| w.pll2on().set_bit());

    // Wait for PLL ready
    while state.rcc.cr().read().pll2rdy().bit_is_clear() {}

    // Enable outputs
    state.rcc.pllcfgr().modify(|_, w| w.divp1en().enabled());

    // This re-enables TIM with the correct new counter
    set_timer(state);
}

fn set_timer(state: &mut SequencerState) {
    let change = &state.seqs.get_active().pllchange_buffer[state.pllchangei as usize];
    state.tim.cnt().modify(|_, w| w.set(change.tim_us));
    state.tim.arr().modify(|_, w| w.set(change.tim_us));

    state.fracn_rem = change.for_ticks - 1;
    state.fracn_i = change.start_tick;

    // Launch the timer
    state.tim.cr1().modify(|_, w| w.cen().enabled());
}

pub fn launch(state: &mut SequencerState) {
    stop(state);
    // Set up the PLL for the first state
    state.pllchangei = -1;
    state.is_running = true;

    step(state);
}

pub fn clear_buffers(state: &mut SequencerState) {
    stop(state);
    state.seqs.get_back().fracn_buffer.clear();
    state.seqs.get_back().pllchange_buffer.clear();
}

pub fn stop(state: &mut SequencerState) {
    state.is_running = false;

    // Disable timer auto-rearm and let it finish
    state.tim.cr1().modify(|_, w| w.opm().enabled());

    state.tim.cr1().modify(|_, w| w.cen().disabled());
    state.tim.cr1().modify(|_, w| w.opm().disabled());
}

pub fn push_fracn(state: &mut SequencerState, fracn: &[u16]) {
    state.uploading = true;
    for fracnv in fracn {
        state.seqs.get_back().fracn_buffer.push(*fracnv).unwrap();
    }
}

pub fn notify_upload_done(state: &mut SequencerState) {
    state.uploading = false;
}

pub fn push_pllchange(state: &mut SequencerState, change: PLLChange) {
    state.uploading = true;

    state
        .seqs
        .get_back()
        .pllchange_buffer
        .push(change)
        .unwrap_or_else(|_| panic!());
}

pub fn setup(
    rcc: stm32h7s::RCC,
    tim: stm32h7s::TIM2,
) -> &'static InterruptAccessible<SequencerState> {
    // Setup TIM2
    rcc.apb1lenr().modify(|_, w| w.tim2en().set_bit());

    // Drive TIM2 with the system clock, divided by 120, such that each clock tick is 1us
    // TODO: Change this if system clock changes!
    tim.psc().modify(|_, w| w.set(120 - 1));
    // Setup clock to count down, only trigger interrupts on update
    tim.cr1().modify(|_, w| {
        w.dir()
            .down()
            .opm()
            .disabled()
            .urs()
            .counter_only()
            .arpe()
            .disabled()
    });

    // Enable the update interrupt
    tim.dier().modify(|_, w| w.uie().enabled());
    unsafe {
        stm32h7s::NVIC::unmask(Interrupt::TIM2);
    }

    // The interrupt should not run now as TIM is disabled, but the guard is needed
    cortex_m::interrupt::free(|cs| {
        SEQUENCER_STATE
            .borrow(cs)
            .replace(MaybeUninit::new(SequencerState {
                seqs: DoubleBuffer::new(),
                pllchangei: 0,
                rcc,
                tim,
                is_running: false,
                fracn_rem: 0,
                fracn_i: 0,
            }));
    });

    util::with(&SEQUENCER_STATE, |state| {
        setup_pll(state);
    });

    &SEQUENCER_STATE
}

#[interrupt]
fn TIM2() {
    util::with(&SEQUENCER_STATE, |state| {
        if state.tim.sr().read().uif().bit_is_set() {
            state.tim.sr().modify(|_, w| w.uif().clear_bit());

            if !state.is_running {
                return;
            }

            if state.fracn_rem == 0 {
                step(state);
            } else {
                state.fracn_i += 1;

                // change fracn
                let fracn = state.seqs.get_active().fracn_buffer[state.fracn_i];

                state
                    .rcc
                    .pllcfgr()
                    .modify(|_, w| w.pll2fracen().clear_bit());
                state
                    .rcc
                    .pll2fracr()
                    .modify(|_, w| unsafe { w.fracn().bits(fracn) });
                state.rcc.pllcfgr().modify(|_, w| w.pll2fracen().set_bit());

                state.fracn_rem -= 1;
            }
        }
    })
}
