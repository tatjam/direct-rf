#![no_std]
#![no_main]

use panic_halt as _;
use stm32h7::stm32h7s;
use cortex_m_rt::entry;

// This marks the entrypoint of our application. The cortex_m_rt creates some
// startup code before this, but we don't need to worry about this
#[entry]
fn main() -> ! {
    let mut periph = stm32h7s::Peripherals::take().unwrap();
    loop {

    }
}