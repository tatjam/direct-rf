#![no_std]
#![no_main]

mod autochirp;

use defmt_rtt as _;
use panic_probe as _;
use cortex_m_rt::entry;

use stm32h7::{stm32h7s};

struct PowerSupplyGuarantee {}

// Returns true if power supply is below 2V, which gurantees fast GPIO is safe
fn powersupply_okay(pwr: &mut stm32h7s::PWR) -> Option<PowerSupplyGuarantee> {

    // Setup threshold for PVD (Programmable Voltage Detector)
    // (Level 0 is around 2V)
    pwr.cr1().modify(|_, w| unsafe{ w.pls().bits(0) });

    // Launch the PVD
    pwr.cr1().modify(|_, w| w.pvde().set_bit());

    // Wait a bit, just in case supply is stabilizing
    for i in 0..100000 {
        core::hint::black_box(i);
    }

    // Check if the supply voltage is okay (less than maximum)
    // (PVD gets true if voltage is below trigger!)
    let okay = pwr.sr1().read().pvdo().bit_is_set();

    pwr.cr1().modify(|_, w| w.pvde().clear_bit());

    if okay {
        return Some(PowerSupplyGuarantee {});
    }

    return None;
}

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
fn setup_gpio(periph: &mut stm32h7s::Peripherals, guarantee: PowerSupplyGuarantee) {
    let rcc = &mut periph.RCC;
    let gpioa = &mut periph.GPIOA;

    // Enable the gpioa peripheral
    rcc.ahb4enr().modify(|_, w| w.gpioaen().enabled());

    // Configure PA8 for special function
    gpioa.moder().modify(|_, w| w.mode8().alternate());
    // Set it for highest speed operation
    //gpioa.ospeedr().modify(|_, w| w.ospeed8().very_high_speed());

}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    defmt::info!("Hello directrf!");

    let powersupply_guarantee = powersupply_okay(&mut periph.PWR);
    if powersupply_guarantee.is_none() {
        defmt::error!("Power supply voltage too high. Make sure you use 1.8V.");
        loop {}
    }

    setup_hse(&mut periph.RCC);
    setup_gpio(&mut periph, powersupply_guarantee.unwrap());
    autochirp::setup_pll(&mut periph.RCC);

    loop {
    }
}