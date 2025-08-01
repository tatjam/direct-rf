#![no_std]
#![no_main]

mod autochirp;
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
}

// Launches GPIO peripheral and setups GPIO PA8 for fastest possible operation,
// also connecting it to MCO1 (alternate function)
fn setup_gpio(periph: &mut stm32h7s::Peripherals) {
    let rcc = &mut periph.RCC;
    let gpioa = &mut periph.GPIOA;

    // Enable the gpioa peripheral
    rcc.ahb4enr().modify(|_, w| w.gpioaen().enabled());

    // Configure PA8 for special function
    gpioa.moder().modify(|_, w| w.mode8().alternate());
    // Set it for highest speed operation
    gpioa.ospeedr().modify(|_, w| w.ospeed8().very_high_speed());
}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    defmt::info!("Hello directrf!");

    setup_hse(&mut periph.RCC);
    setup_gpio(&mut periph);
    let sequencer_state = sequencer::setup(&mut periph);
    sequencer::launch(sequencer_state);
    sequencer::stop(sequencer_state);

    let rcc = &mut periph.RCC;

    let mut i: u16 = 0;
    loop {
    }
}