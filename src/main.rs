#![no_std]
#![no_main]


use defmt_rtt as _;
use panic_probe as _;
use cortex_m_rt::entry;

use stm32h7::{stm32h7s};


// Returns true if power supply is below 2V, which gurantees fast GPIO is safe
fn powersupply_okay(pwr: &mut stm32h7s::PWR) -> bool {

    // Setup threshold for PVD (Programmable Voltage Detector)
    // (Level 0 is around 2V)
    pwr.cr1().write(|w| unsafe{ w.pls().bits(0) });

    // Launch the PVD
    pwr.cr1().write(|w| w.pvde().set_bit());

    // Wait a bit, just in case supply is stabilizing
    for i in 0..100000 {
        core::hint::black_box(i);
    }

    // Check if the supply voltage is okay (less than maximum)
    // (PVD gets true if voltage is below trigger!)
    let okay = pwr.sr1().read().pvdo().bit_is_set();

    pwr.cr1().write(|w| w.pvde().clear_bit());

    return okay;
}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    defmt::info!("Hello directrf!");

    if !powersupply_okay(&mut periph.PWR) {
        defmt::error!("Power supply voltage too high. Make sure you use 1.8V.");
       loop {}
    }
    else {
        defmt::info!("Power supply voltage check okay");
    }

    autochirp::setup_pll(&mut periph.RCC);

    loop {
    }
}