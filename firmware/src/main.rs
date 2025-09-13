#![no_std]
#![no_main]

mod comm;
mod sequencer;
mod util;

use crate::sequencer::SequencerState;
use crate::util::InterruptAccessible;
use common::comm_messages::UplinkMsg;
use core::hint::black_box;
use cortex_m_rt::entry;
use defmt::export::panic;
use defmt_rtt as _;
use panic_probe as _;
use stm32h7::stm32h7s;

// Assumes we are on a NUCLEO board, which has a 24MHz clock source connected to HSE
fn setup_hse(rcc: &mut stm32h7s::RCC, flash: &mut stm32h7s::FLASH) {
    // Enable monitor output, dividing PLL1 clock frequency by 100
    // (Note this is not exactly the system frequency! It differs by divp)
    rcc.cfgr()
        .modify(|_, w| unsafe { w.mco1().pll1_q().mco1pre().bits(10) });
    rcc.pll1divr1()
        .modify(|_, w| unsafe { w.divq().bits(10 - 1) });
    rcc.pllcfgr().modify(|_, w| w.divq1en().enabled());

    defmt::info!("Starting HSE");
    rcc.cr().modify(|_, w| w.hseon().set_bit());

    // Wait for HSE ready
    while !rcc.cr().read().hserdy().bit_is_set() {}

    defmt::info!("HSE is ready");

    // Use a PLL for system clock (which also drives APB1 and AHB bus clocks)
    // This PLL is also driven by HSE
    // Critically, note that TIM uses the bus clock, so we want reduced jitter
    // (We use a 24MHz clock and divide by 2 to get 12Mhz reference in PLL1)
    rcc.pllckselr()
        .modify(|_, w| w.divm1().set(2).pllsrc().hse());
    rcc.pllcfgr().modify(|_, w| w.pll1rge().range8());

    // Use the 384 to 1672MHz VCO
    rcc.pllcfgr().modify(|_, w| w.pll1vcosel().clear_bit());
    // This makes the VCO oscillate at 480MHz, and output a signal at 120MHz
    rcc.pll1divr1()
        .modify(|_, w| unsafe { w.divn1().bits(19).divp().bits(2 - 1) });
    // Enable DIVP output
    rcc.pllcfgr().modify(|_, w| w.divp1en().enabled());

    rcc.cr().modify(|_, w| w.pll1on().set_bit());

    // Wait for PLL ready
    while rcc.cr().read().pll1rdy().bit_is_clear() {}

    defmt::info!("PLL1 is ready!");

    // Set FLASH to increase delay states. We over-estimate a bit
    flash.acr().modify(|_, w| unsafe { w.latency().bits(3) });

    // Set system clock to use PLL1
    rcc.cfgr().modify(|_, w| w.sw().pll1());

    if !rcc.cfgr().read().sw().is_pll1() {
        defmt::error!("Could not clock system using PLl1");
        panic();
    }
    defmt::info!("System is clocked using PLL!");
}

// Launches GPIO peripheral and setups GPIO PC9 for fastest possible operation,
// also connecting it to MCO2 (alternate function)
fn setup_gpio(rcc: &mut stm32h7s::RCC, gpioc: &mut stm32h7s::GPIOC) {
    // Enable the gpioa peripheral
    rcc.ahb4enr().modify(|_, w| w.gpiocen().enabled());

    // Configure PC9 for special function
    gpioc.moder().modify(|_, w| w.mode9().alternate());
    // Set it for highest speed operation
    // gpioc.ospeedr().modify(|_, w| w.ospeed9().very_high_speed());
}

fn handle_msg(msg: UplinkMsg, sequencer_state: &InterruptAccessible<SequencerState>) {
    match msg {
        UplinkMsg::Ping() => {
            defmt::info!("Pong :)")
        }
        UplinkMsg::PushPLLChange(ch) => {
            util::with(sequencer_state, |state| {
                defmt::info!("Pushing PLLChange");
                sequencer::push_pllchange(state, ch);
            });
        }
        UplinkMsg::PushFracn(num, arr) => {
            util::with(sequencer_state, |state| {
                defmt::info!("Pushing fracn");
                sequencer::push_fracn(state, &arr[0..(num as usize)]);
                defmt::info!("Done :)");
            });
        }
        UplinkMsg::ClearBuffers() => {
            util::with(sequencer_state, |state| {
                defmt::info!("Cleaning buffers");
                sequencer::clear_buffers(state);
            });
        }
        UplinkMsg::StartNow() => {
            util::with(sequencer_state, |state| {
                defmt::info!("Launching");
                sequencer::launch(state);
            });
        }
        UplinkMsg::StopNow() => {
            util::with(sequencer_state, |state| {
                defmt::info!("Stopping");
                sequencer::stop(state);
            });
        }
        UplinkMsg::SetLooping(_) => {}
        UplinkMsg::EpochNow(_) => {}
        UplinkMsg::StartAtEpoch(_) => {}
    }
}

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this. otably, it
// enables the FPU
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    defmt::info!("Hello directrf!");

    setup_hse(&mut periph.RCC, &mut periph.FLASH);
    setup_gpio(&mut periph.RCC, &mut periph.GPIOC);

    periph.RCC.ahb4enr().modify(|_, w| w.gpioaen().enabled());
    periph.GPIOA.moder().modify(|_, w| w.mode8().alternate());

    let comm_state = comm::setup(&mut periph.RCC, &periph.GPIOD, periph.USART3);
    let sequencer_state = sequencer::setup(periph.RCC, periph.TIM2);

    defmt::info!("Sequencer is setup!");

    loop {
        let msg = util::with(comm_state, comm::get_message);

        if let Some(v) = msg {
            handle_msg(v, sequencer_state);
        } else {
            for i in 0..1000000 {
                black_box(i);
            }
        }
    }
}
