use core::cell::{Cell, Ref, RefCell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr;
use cortex_m::interrupt::Mutex;
use heapless::Vec;
use stm32h7::{stm32h7s};
use stm32h7::stm32h7s::interrupt;
use crate::util;
use crate::util::{with, InterruptAccessible, RingBuffer, SingleThreadUnsafeCell};

static COMM_STATE: InterruptAccessible<CommState> = InterruptAccessible::new();

type TXRingBuffer = RingBuffer<u8, 512>;
type RXRingBuffer = RingBuffer<u8, 512>;

// Raw pointers to the rx and tx buffers, which handle "thread" safety on their own.
static mut RX_BUFFER: *const RXRingBuffer = ptr::null();
static mut TX_BUFFER: *const RXRingBuffer = ptr::null();

struct CommState {
    usart: stm32h7s::USART3,
    tx_buffer: TXRingBuffer,
    rx_buffer: RXRingBuffer,
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

    cortex_m::interrupt::free( |cs| {
        COMM_STATE.borrow(cs).replace(MaybeUninit::new(CommState{
            usart,
            tx_buffer: RingBuffer::new(),
            rx_buffer: RingBuffer::new()
        }));
    });

    util::with(&COMM_STATE, |state| {
        unsafe {
            // It's safe to take these pointers as they point to static memory and the RingBuffer
            // handles "thread safety" for us.
            RX_BUFFER = &state.rx_buffer;
            TX_BUFFER = &state.tx_buffer;
        }
        // Finally, enable USART, receiver and transmitter, and receiver interrupt
        // Transmitter interrupt is enabled as needed
        state.usart.cr1().modify(|_, w| w
            .rxffie().enabled()
            .ue().enabled()
            .re().enabled()
            .te().enabled());
    });



    &COMM_STATE
}

#[inline]
fn usart3_rxff(rx_buffer: &RXRingBuffer) {

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
    let (rxff, txfe) = util::with(&COMM_STATE, |state| {(
        state.usart.isr().read().rxff().bit_is_set(),
        state.usart.isr().read().txfe().bit_is_set()
    )});

    assert!(rxff || txfe);

    let (rx_buffer, tx_buffer) = unsafe {(
        // This is safe, as the ring buffers handle "thread safety" and are static, const variables
        // (They do have interior mutability!)
        &RX_BUFFER.read(),
        &TX_BUFFER.read()
    )};

    if rxff {
        usart3_rxff(rx_buffer);
    }

    if txfe {
        usart3_txfe(rx_buffer);
    }

    // Note that the interrupt flags are cleared upon emptying of RX FIFO, or filling of TX FIFO!
}