use common::{comm_messages::UplinkMsg, sequence::PLLChange};
use embassy_futures::join;
use embassy_stm32::{
    Peri, bind_interrupts,
    mode::Async,
    peripherals,
    usart::{self, Uart},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::Timer;
use postcard::accumulator::CobsAccumulator;

enum FreqCommand {
    Fracn(u16),
    Change(u8),
}

// This signal is used to send commands to the PLL
static LIVE_COMMAND: Signal<CriticalSectionRawMutex, FreqCommand> = Signal::new();

// The channel is used to "double-buffer" incoming and outgoing commands, to relax requirements
// on the communication link
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, FreqCommand, 4096> = Channel::new();

#[embassy_executor::task]
pub async fn pll_controller_task() {
    loop {
        let cmd = LIVE_COMMAND.wait().await;

        match cmd {
            FreqCommand::Fracn(_fracn) => {}
            FreqCommand::Change(_change_idx) => {}
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
            todo!("Setup the pll change in the buffer");
            COMMAND_CHANNEL.send(FreqCommand::Change(0)).await;
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

    let mut rx_buffer: [u8; 64] = [0; 64];
    let mut accumulator: CobsAccumulator<128> = CobsAccumulator::new();

    loop {
        uart.read(rx_buffer.as_mut_slice()).await.unwrap();

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
