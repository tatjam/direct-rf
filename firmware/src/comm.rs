use core::mem::MaybeUninit;
use core::{ptr};
use heapless::Vec;
use stm32h7::{stm32h7s};
use stm32h7::stm32h7s::interrupt;
use crate::util::{with, InterruptAccessible, RingBuffer};
use common::comm_messages::{DownlinkMsg, UplinkMsg, MAX_UPLINK_MSG_SIZE};

static COMM_STATE: InterruptAccessible<CommState> = InterruptAccessible::new();

type TXRingBuffer = RingBuffer<u8, 512>;
type RXRingBuffer = RingBuffer<u8, 512>;

// Raw pointers to the rx and tx buffers, which handle "thread" safety on their own.
static mut RX_BUFFER: *const RXRingBuffer = ptr::null();
static mut TX_BUFFER: *const RXRingBuffer = ptr::null();

pub struct CommState {
    usart: stm32h7s::USART3,
    tx_buffer: TXRingBuffer,
    rx_buffer: RXRingBuffer,
    msg_buffer: [u8; MAX_UPLINK_MSG_SIZE],
    msg_buffer_ptr: usize,
}

// Initiates a transfer of bytes, possibly blocking if the tx buffer gets full
fn tx(state: &mut CommState, bytes: &[u8]) {
}

// We use USART3, as it's connected to the VCOM port thanks to ST-Link
pub fn setup(rcc: &mut stm32h7s::RCC, usart: stm32h7s::USART3)
             -> &'static InterruptAccessible<CommState> {

    rcc.apb1lenr().modify(|_, w| w.usart3en().enabled());

    // 115200bps, 8bit-data, no parity, one stop bit, no flow control, per the
    // NUCLEO board docs.
    // Reset values are good for 8bit-data, 1 start bit, 1 stop bit,
    // parity disabled, flow control disabled. We just need the bitrate...
    // By default, usart3 is clocked from rcc_pclk1, which is a bit inconvenient
    // as it also is used as the kernel clock for TIM2...
    // We instead clock UART from the remaining PLL we have, thus using all 3 of them :)

    // Clock PLL3 input at 12MHz
    rcc.pllckselr().modify(|_, w| w.divm3().set(2).pllsrc().hse());
    rcc.pllcfgr().modify(|_, w| w.pll3rge().range8());

    // Use the 384 to 1672MHz VCO
    rcc.pllcfgr().modify(|_, w| w.pll3vcosel().clear_bit());
    // This makes the VCO oscillate at 1152MHz, and output a signal at exactly 115.2MHz
    rcc.pll3divr1().modify(|_, w| unsafe {w.divn3().bits(96).divq().bits(5 - 1)});
    // Enable DIVQ output
    rcc.pllcfgr().modify(|_, w| w.divq3en().enabled());

    rcc.cr().modify(|_, w| w.pll3on().set_bit());

    // Wait for PLL ready
    while rcc.cr().read().pll3rdy().bit_is_clear() {}

    defmt::info!("PLL3 is ready!");

    // Clocking is exact, so brr = 1
    usart.brr().modify(|_, w| w.brr().set(1));

    // We use FIFO mode
    usart.cr1().modify(|_, w| w.fifoen().enabled());

    // Trigger receive interrupt when FIFO is 3/4 full, to leave some margin
    usart.cr3().modify(|_, w| w.rxftcfg().depth_3_4());

    cortex_m::interrupt::free( |cs| {
        COMM_STATE.borrow(cs).replace(MaybeUninit::new(CommState{
            usart,
            tx_buffer: RingBuffer::new(),
            rx_buffer: RingBuffer::new(),
            msg_buffer: [0; MAX_UPLINK_MSG_SIZE],
            msg_buffer_ptr: 0,
        }));
    });

    with(&COMM_STATE, |state| {
        unsafe {
            // It's safe to take these pointers as they point to static memory and the RingBuffer
            // handles "thread safety" for us.
            RX_BUFFER = &state.rx_buffer;
            TX_BUFFER = &state.tx_buffer;
        }
        // Enable RX interrupt
        state.usart.cr3().modify(|_, w| w.rxftie().enabled());

        // Finally, enable USART, receiver and transmitter
        state.usart.cr1().modify(|_, w| w
            .ue().enabled()
            .re().enabled()
            .te().enabled());
    });



    &COMM_STATE
}

#[inline]
fn usart3_rxft(rx_buffer: &RXRingBuffer) {
    // We use a buffer instead of directly writing to rx_buffer to prevent excessive locking
    // If we are unable to extract all data from RXFIFO, next interrupt will finish the job!
    let mut buffer: Vec<u8, 16> = Vec::new();
    let read = with(&COMM_STATE, |state| {
        while state.usart.isr().read().rxft().bit_is_set() && buffer.len() <= buffer.capacity() {
            let byte = state.usart.rdr().read().rdr().bits();
            buffer.push((byte & 0xFF) as u8).unwrap();
        }
    });

    let num_written = rx_buffer.write(buffer.as_slice());
    if num_written != buffer.len() {
        // This would mean losing data, the programmer must increase the buffer size
        panic!("RX buffer is full to writes");
    }

}

fn try_decode_message(buff: &mut [u8]) -> Option<UplinkMsg> {
    let result = postcard::from_bytes_cobs(buff);
    match result {
        Ok(msg) => {
            // Send Ack
            msg
        },
        Err(err) => {
            // Send Unack, this will likely trigger a resend of the message
            None
        }
    }
}

// If a full message is available in the receive ring buffer, it's returned
pub fn get_message(state: &mut CommState) -> Option<UplinkMsg> {
    let advance = state.rx_buffer.read(&mut state.msg_buffer[state.msg_buffer_ptr..], Some(0));
    state.msg_buffer_ptr += advance;

    if state.msg_buffer_ptr == 0 {
        return None
    }

    if state.msg_buffer[state.msg_buffer_ptr - 1] == 0 {
        // Note this does not include the final 0
        try_decode_message(&mut state.msg_buffer[0..state.msg_buffer_ptr])
    } else {
        None
    }

}

#[inline]
fn usart3_txfe(tx_buffer: &TXRingBuffer) {

}

// This interrupt is designed to block as little as possible, in order to allow nearly
// parallel receiving of information and processing.
#[interrupt]
fn USART3() {

    // rxff = RX FIFO Full
    // txfe = TX FIFO Empty
    let (rxft, txfe) = with(&COMM_STATE, |state| {(
        state.usart.isr().read().rxft().bit_is_set(),
        state.usart.isr().read().txfe().bit_is_set()
    )});

    assert!(rxft || txfe);

    let (rx_buffer, tx_buffer) = unsafe {(
        // This is safe, as the ring buffers handle "thread safety" and are static, const variables
        // (They do have interior mutability!)
        &RX_BUFFER.read(),
        &TX_BUFFER.read()
    )};

    if rxft {
        usart3_rxft(rx_buffer);
    }

    if txfe {
        usart3_txfe(rx_buffer);
    }

    // Note that the interrupt flags are cleared upon emptying of RX FIFO, or filling of TX FIFO!
}