use stm32h7::{stm32h7s};

// Uses spread spectrum to "automatically" generate chirps
pub fn setup_pll(rcc: &mut stm32h7s::RCC) {
    // Configure these as desired
    let desired_center_freq: f64 = 430_000_000.0;
    // (HSE on NUCLEO board)
    let input_freq: f64 = 24_000_000.0;
    let divided_freq: f64 = 4_000_000.0;
    assert!(divided_freq >= 1_000_000.0 && divided_freq <= 16_000_000.0);

    // DIV1M has to divide input_freq to get divided_freq, so...
    let div1m = libm::floor(input_freq / divided_freq) as u8;
    assert!(div1m <= 63, "PLL divm too high, lower input freq or increase divided_freq.");

    // DIVN has to MULTIPLY divided_freq to get desired_freq / 2 (an aditional 2 divider is
    // at the output of the fast VCO)
    let plldivn = libm::floor(desired_center_freq / divided_freq - 1.0) as u16;
    assert!(plldivn >= 8 && plldivn <= 420,
            "PLL divn too high, lower center freq or increase divided_freq.");

    defmt::info!("Using div1m = {}, plldivn = {}, actual center freq {}", div1m, plldivn,
        input_freq / (div1m as f64) * (plldivn as f64 + 1.0));

    rcc.pllckselr().modify(|_, w| w.pllsrc().hse());
    // Set predividers to get good reference clock
    // NOTE: HSI is selected by default on reset as PLLSRC
    // NOTE: This allows us to leave default PLL1RGE value, which assumes input freq is from 1 to 2MHz
    rcc.pllckselr().modify(|_, w| w.divm1().set(div1m));

    // Make sure the sigma-delta modulator (SDM) is loaded with 0.
    // Procedure taken from manual, may not be needed as this seems to be the default value?
    rcc.pllcfgr().modify(|_, w| w.pll1fracen().clear_bit());
    rcc.pll1fracr().modify(|_, w| unsafe{ w.fracn().bits(0)});
    rcc.pllcfgr().modify(|_, w| w.pll1fracen().set_bit());

    // Wait for a bit (atleast 5μs)
    for i in 0..100000 {
        core::hint::black_box(i);
    }

    // TODO: Is this next call required? Doesn't appear to be
    //rcc.pllcfgr().modify(|_, w| w.pll1fracen().clear_bit());

    rcc.pllcfgr().modify(|_, w| w.pll1sscgen().clear_bit());

    rcc.pllcfgr().modify(|_, w| w.pll1vcosel().clear_bit());
    // TODO: Later on we may want to use HSE to reduce jitter
    // Output PLL on MCO1, which can be connected to pll1_q_ck
    // (MCO1 is the alternate function 0 for PA8)
    // TODO: Maybe set prescaler to 1 instead of disabled?
    rcc.cfgr().modify(|_, w| w.mco1().pll1_q().mco1pre().set(1));



    if divided_freq <= 2_000_000.0 {
        rcc.pllcfgr().modify(|_, w| w.pll1rge().range1());
    } else if divided_freq <= 4_000_000.0 {
        rcc.pllcfgr().modify(|_, w| w.pll2rge().range2());
    } else if divided_freq <= 8_000_000.0 {
        rcc.pllcfgr().modify(|_, w| w.pll2rge().range4());
    } else if divided_freq <= 16_000_000.0 {
        rcc.pllcfgr().modify(|_, w| w.pll2rge().range8());
    }


    rcc.pll1divr1().modify(|_, w| unsafe{ w.divn1().bits(plldivn) });


    /*
    // Setup clock spreading for chirps
    rcc.pllcfgr().modify(|_, w| w.pll1sscgen().set_bit());
    */

    // Enable PLL
    rcc.cr().modify(|_, w| w.pll1on().set_bit());

    // Wait for PLL ready
    while rcc.cr().read().pll1rdy().bit_is_clear() {}

    // Enable outputs
    rcc.pllcfgr().modify(|_, w| w.divq1en().enabled());

    defmt::info!("PLL is ready!");

}
