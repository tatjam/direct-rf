#![no_std]
#![no_main]


use defmt_rtt as _;
use panic_probe as _;
use stm32h7::{stm32h7s};

use cortex_m_rt::entry;

// Uses spread spectrum to "automatically" generate chirps
fn setup_pll_autochirp(rcc: &mut stm32h7s::RCC) {
    // Configure these as desired
    let desired_center_freq: f64 = 435_000_000.0;
    let input_freq: f64 = 64_000_000.0;
    let divided_freq: f64 = 2_000_000.0;
    assert!(divided_freq >= 1_000_000.0 && divided_freq <= 16_000_000.0);

    let div1m = libm::floor(input_freq / divided_freq) as u8;
    assert!(div1m <= 63, "PLL divm too high, lower input freq or increase divided_freq.");

    let plldivn = libm::floor(desired_center_freq / 2_000_000.0) as u16;
    assert!(plldivn >= 8 && plldivn <= 420,
            "PLL divn too high, lower center freq or increase divided_freq.");


    // TODO: Later on we may want to use HSE to reduce jitter
    // Output PLL on MCO1, which can be connected to pll1_q_ck

    // Set predividers to get good reference clock (ex. divide HSI, 64MHz, by 32 to get 2MHz)
    // NOTE: HSI is selected by default on reset as PLLSRC
    // NOTE: This allows us to leave default PLL1RGE value, which assumes input freq is from 1 to 2MHz
    rcc.pllckselr().write(|w| w.divm1().set(div1m));

    if divided_freq <= 2_000_000.0 {
        rcc.pllcfgr().write(|w| w.pll1rge().range1());
    } else if divided_freq <= 4_000_000.0 {
        rcc.pllcfgr().write(|w| w.pll2rge().range2());
    } else if divided_freq <= 8_000_000.0 {
        rcc.pllcfgr().write(|w| w.pll2rge().range4());
    } else if divided_freq <= 16_000_000.0 {
        rcc.pllcfgr().write(|w| w.pll2rge().range8());
    }

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

    rcc.pll1divr1().write(|w| unsafe{ w.divn1().bits(plldivn) });


    /*
    // Setup clock spreading for chirps
    rcc.pllcfgr().write(|w| w.pll1sscgen().set_bit());
    */

    // Enable PLL
    rcc.cr().write(|w| w.pll1on().set_bit());

    // Wait for PLL ready
    while rcc.cr().read().pll1rdy().bit_is_clear() {}

}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    defmt::info!("Hello!");

    setup_pll_autochirp(&mut periph.RCC);

    loop {
    }
}