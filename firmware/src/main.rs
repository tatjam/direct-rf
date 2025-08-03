#![no_std]
#![no_main]

mod comm;
mod comm_messages;
mod sequencer;

use defmt_rtt as _;
use panic_probe as _;
use cortex_m_rt::entry;

use stm32h7::{stm32h7s};

// Assumes we are on a NUCLEO board, which has a 24MHz clock source connected to HSE
fn setup_hse(rcc: &mut stm32h7s::RCC) {
    defmt::info!("Starting HSE");
    rcc.cr().modify(|_, w| w.hseon().set_bit());

    // Wait for HSE ready
    while !rcc.cr().read().hserdy().bit_is_set() {}

    defmt::info!("HSE is ready");

    // Use a PLL for system clock (which also drives APB1 and AHB bus clocks)
    // This PLL is also driven by HSE
    // Critically, note that TIM uses the bus clock, so we want reduced jitter



}

// Launches GPIO peripheral and setups GPIO PC9 for fastest possible operation,
// also connecting it to MCO2 (alternate function)
fn setup_gpio(rcc: &mut stm32h7s::RCC, gpioc: &mut stm32h7s::GPIOC) {
    // Enable the gpioa peripheral
    rcc.ahb4enr().modify(|_, w| w.gpiocen().enabled());

    // Configure PC9 for special function
    gpioc.moder().modify(|_, w| w.mode9().alternate());
    // Set it for highest speed operation
    gpioc.ospeedr().modify(|_, w| w.ospeed9().very_high_speed());
}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    let mut core_periph = cortex_m::Peripherals::take().unwrap();
    defmt::info!("Hello directrf!");


    setup_hse(&mut periph.RCC);
    setup_gpio(&mut periph.RCC, &mut periph.GPIOC);

    let ch0 = periph.GPDMA.ch(0);
    let sequencer_state = sequencer::setup(periph.RCC, periph.TIM2, periph.GPDMA);

    sequencer::with_state(sequencer_state, |state| {
        sequencer::launch(state);
        sequencer::stop(state);
    });

    loop {
    }
}