#![no_std]
#![no_main]

use core::ops::BitOr;
use panic_halt as _;
use stm32h7::{stm32h7s, Periph};
use cortex_m_rt::entry;

// Uses spread spectrum to "automatically" generate chirps
fn setup_pll_autochirp(rcc: &mut stm32h7s::RCC) {
    let desired_center_freq: f64 = 435_000_000.0;
    let input_freq: f64 = 64_000_000.0;
    let div1m_float: f64 = input_freq / 2_000_000.0;
    // Make sure the input frequency is not too high, as otherwise we couldn't divide it down
    assert!(div1m_float < 64.0);
    let div1m = libm::floor(div1m_float) as u8;
    assert!(libm::fabs(libm::floor(div1m_float) - div1m_float) < 0.001);



    // TODO: Later on we may want to use HSE to reduce jitter
    // Output PLL on MCO1, which can be connected to pll1_q_ck

    // Set predividers to get good reference clock (ex. divide HSI, 64MHz, by 32 to get 2MHz)
    // NOTE: HSI is selected by default on reset as PLLSRC
    // NOTE: This allows us to leave default PLL1RGE value, which assumes input freq is from 1 to 2MHz
    rcc.pllckselr().write(|w| w.divm1().set(32));

    // Make sure the sigma-delta modulator (SDM) is loaded with 0.
    // Procedure taken from manual, may not be needed as this seems to be the default value?
    rcc.pllcfgr().write(|w| w.pll1fracen().clear_bit());
    rcc.pll1fracr().write(|w| unsafe{ w.fracn().bits(0)});
    rcc.pllcfgr().write(|w| w.pll1fracen().set_bit());

    // Wait for a bit (atleast 5μs)
    for i in 0..100000 {
        core::hint::black_box(i);
    }

    // TODO: Is this next call required? Doesn't appear to be
    rcc.pllcfgr().write(|w| w.pll1fracen().clear_bit());

    // Setup nominal PLL frequency on the 435MHz band, so we have a "safe" bandwidth of 10MHz
    // To do so, we multiply the input by 435, i.e. divide the input by that


    /*
    // Setup clock spreading for chirps
    rcc.pllcfgr().write(|w| w.pll1sscgen().set_bit());

    */

    // Enable PLL
    rcc.cr().write(|w| w.pll1on().set_bit());

    // Wait for PLL ready
    while(rcc.cr().read().pll1rdy().bit_is_clear()) {}

}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this, notably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();

    // Enable the PLL clock and output it on MCO
    setup_pll_autochirp(&mut periph.RCC);

    loop {
        
    }
}