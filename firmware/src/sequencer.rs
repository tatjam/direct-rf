use core::hint::black_box;

use common::{comm_messages::UplinkMsg, sequence::PLLChange};
use embassy_futures::join;
use embassy_stm32::{
    Peri, bind_interrupts, pac, peripherals,
    rcc::{PllDiv, PllMul, PllPreDiv},
    usart::{self, Uart},
};
use embassy_sync::{
    blocking_mutex::{CriticalSectionMutex, raw::CriticalSectionRawMutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::Timer;
use heapless::Vec;
use postcard::accumulator::CobsAccumulator;

enum FreqCommand {
    Fracn(u16),
    Change(),
}

// This signal is used to send commands to the PLL
static LIVE_COMMAND: Signal<CriticalSectionRawMutex, FreqCommand> = Signal::new();
// This acts as a buffer between incoming data and data being sent to the PLL, it can either suppose a fracn change
// or a notification of an incoming PLL change
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, FreqCommand, 4096> = Channel::new();
// This acts as a buffer for PLL changes, as they are relatively uncommon but heavyweight, to prevent
// the commands from having to carry them
static PLL_CHANGE_CHANNEL: Channel<CriticalSectionRawMutex, PLLChange, 8> = Channel::new();

fn setup_pll2() {
    let rcc = pac::RCC;

    // Output PLL on MCO2 dividing the PLL VCO freq as convenient
    rcc.cfgr()
        .modify(|w| w.set_mco1pre(embassy_stm32::rcc::McoPrescaler::DIV1));
    rcc.pllcfgr().modify(|w| w.set_divpen(2, true));

    // Input clock is HSE, which is 24MHz, and we drive the PLL
    // with 12MHz, because it's outside the band of interest and
    // is overall a pretty nice number (its divisible by 1, 2, 3, 4, 6 and 12)
    // which allows us to obtain neat round frequencies without the sigma-delta modulator.
    rcc.pllckselr().modify(|w| {
        w.set_divm(2, PllPreDiv::DIV2);
        w.set_pllsrc(embassy_stm32::rcc::PllSource::HSE);
    });

    // We need to tell the PLL that its input is 12MHz (range8)
    rcc.pllcfgr()
        .modify(|w| w.set_pllrge(2, pac::rcc::vals::Pllrge::RANGE8));

    // Use the 150 to 420MHz VCO
    rcc.pllcfgr()
        .modify(|w| w.set_pllvcosel(2, pac::rcc::vals::Pllvcosel::MEDIUM_VCO));

    // Set a sane default state (output 8MHz)
    rcc.plldivr(2).modify(|w| {
        w.set_plln(PllMul::from(19));
        w.set_pllp(PllDiv::from(29));
    });
}

fn handle_pllchange(change: PLLChange) {
    let rcc = pac::RCC;

    // Disable the output, to prevent spurious signals
    rcc.pllcfgr().modify(|w| w.set_divpen(2, false));

    // Disable the PLL
    rcc.cr().modify(|w| w.set_pllon(2, false));

    // Set the dividers
    // TODO: This is most likely wrong
    rcc.plldivr(2).modify(|w| {
        w.set_plln(PllMul::from(change.divn));
        w.set_pllp(PllDiv::from(change.divp));
    });

    // Re-enable the PLL
    rcc.cr().modify(|w| w.set_pllon(2, true));

    // Busy-wait for PLL ready and locked
    while !rcc.cr().read().pllrdy(2) {}

    // Re-enable the output
    rcc.pllcfgr().modify(|w| w.set_divpen(2, true));
}

fn handle_fracn(fracn: u16) {
    let rcc = pac::RCC;

    // Disable fractional synthesizer
    rcc.pllcfgr().modify(|w| w.set_pllfracen(2, false));

    // Set the new fracn
    rcc.pllfracr(2).modify(|w| w.set_fracn(fracn));

    // Re-enable fractional synthesizer
    rcc.pllcfgr().modify(|w| w.set_pllfracen(2, true));
}

#[embassy_executor::task]
pub async fn pll_controller_task() {
    setup_pll2();

    loop {
        let cmd = LIVE_COMMAND.wait().await;

        match cmd {
            FreqCommand::Fracn(fracn) => handle_fracn(fracn),
            FreqCommand::Change() => {
                handle_pllchange(PLL_CHANGE_CHANNEL.receive().await);
            }
        }
    }
}

#[embassy_executor::task]
pub async fn sequencer_task() {
    let sleep_us: u64 = 100;

    loop {
        // Wait for timer (or command if none are available just yet)
        let (cmd, _) = join::join(COMMAND_CHANNEL.receive(), Timer::after_micros(sleep_us)).await;
        LIVE_COMMAND.signal(cmd);
    }
}

bind_interrupts!(struct Irqs {
    USART3 => usart::InterruptHandler<peripherals::USART3>;
});

async fn handle_comm_msg(msg: UplinkMsg) {
    match msg {
        UplinkMsg::PushPLLChange(pllchange) => {
            PLL_CHANGE_CHANNEL.send(pllchange).await;
            COMMAND_CHANNEL.send(FreqCommand::Change()).await;
        }
        UplinkMsg::PushFracn(num, buf) => {
            for i in 0..num {
                COMMAND_CHANNEL
                    .send(FreqCommand::Fracn(buf[i as usize]))
                    .await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn comm_task(
    uart: Peri<'static, peripherals::USART3>,
    tx_pin: Peri<'static, peripherals::PB10>,
    rx_pin: Peri<'static, peripherals::PB11>,
    tx_dma: Peri<'static, peripherals::GPDMA1_CH0>,
    rx_dma: Peri<'static, peripherals::GPDMA1_CH1>,
) {
    let mut config = usart::Config::default();
    config.baudrate = 1_000_000;

    let mut uart = Uart::new(
        uart,
        rx_pin,
        tx_pin,
        Irqs,
        tx_dma,
        rx_dma,
        usart::Config::default(),
    )
    .unwrap();

    let mut rx_buffer: [u8; 512] = [0; 512];
    let mut accumulator: CobsAccumulator<512> = CobsAccumulator::new();

    let ok_bytes: [u8; 1] = [1; 1];

    loop {
        uart.read(rx_buffer.as_mut_slice()).await.unwrap();
        // Send an acknowledge so PC knows it can send more data
        uart.write(&ok_bytes).await.unwrap();

        let mut window = &rx_buffer[..];

        while !window.is_empty() {
            window = match accumulator.feed::<UplinkMsg>(&rx_buffer) {
                postcard::accumulator::FeedResult::Consumed => break,
                postcard::accumulator::FeedResult::OverFull(new_wind) => new_wind,
                postcard::accumulator::FeedResult::DeserError(new_wind) => new_wind,
                postcard::accumulator::FeedResult::Success { data, remaining } => {
                    handle_comm_msg(data).await;
                    remaining
                }
            }
        }
    }
}
