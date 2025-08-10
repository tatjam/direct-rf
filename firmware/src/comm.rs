use core::mem::MaybeUninit;
use core::{ptr};
use heapless::Vec;
use stm32h7::{stm32h7s};
use stm32h7::stm32h7s::{interrupt, Interrupt};
use crate::util::{with, InterruptAccessible, RingBuffer};
use common::comm_messages::{UplinkMsg, MAX_UPLINK_MSG_SIZE};

static COMM_STATE: InterruptAccessible<CommState> = InterruptAccessible::new();

type TXRingBuffer = RingBuffer<u8, 512>;
type RXRingBuffer = RingBuffer<u8, 512>;

// Raw pointers to the rx and tx buffers, which handle "thread" safety on their own.
static mut RX_BUFFER: *const RXRingBuffer = ptr::null();

pub struct CommState {
    pub usart: stm32h7s::USART3,
    rx_buffer: RXRingBuffer,
    msg_buffer: [u8; MAX_UPLINK_MSG_SIZE], msg_buffer_ptr: usize,
}

fn ack(state: &CommState) {
    while state.usart.isr().read().txfnf().bit_is_clear() {}
    state.usart.tdr().write(|w| unsafe{ w.bits(1) });
}

fn nack(state: &CommState) {
    while state.usart.isr().read().txfnf().bit_is_clear() {}
    state.usart.tdr().write(|w| unsafe{ w.bits(0) });
}

// We use USART3, as it's connected to the VCOM port thanks to ST-Link
pub fn setup(rcc: &mut stm32h7s::RCC, gpiod: &stm32h7s::GPIOD, usart: stm32h7s::USART3)
             -> &'static InterruptAccessible<CommState> {

    // Enable the GPIOs used, set them to alternate function and point them to USART3
    rcc.apb1lenr().modify(|_, w| w.usart3en().enabled());

    rcc.ahb4enr().modify(|_, w| w.gpioden().enabled());
    gpiod.moder().modify(|_, w| w
        .mode8().alternate()
        .mode9().alternate());
    gpiod.afrh().modify(|_, w| w
        .afrel8().af7()
        .afrel9().af7());


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
    rcc.pllcfgr().modify(|_, w| w.pll3vcosel().wide_vco());
    // This makes the VCO oscillate at 1152MHz, and output a signal at exactly 115.2MHz
    rcc.pll3divr1().modify(|_, w| unsafe {w.divn3().bits(96 - 1).divq().bits(10 - 1)});
    // Enable DIVQ output
    rcc.pllcfgr().modify(|_, w| w.divq3en().enabled());

    rcc.cr().modify(|_, w| w.pll3on().set_bit());

    // Wait for PLL ready
    while rcc.cr().read().pll3rdy().bit_is_clear() {}

    defmt::info!("PLL3 is ready!");

    // Clock USART from PLL3
    rcc.ccipr2().modify(|_, w| w.uart234578sel().pll3_q());

    // Clocking is exact, so brr = 1000 (we want 115.2kbps, not Mbps!)
    usart.brr().modify(|_, w| w.brr().set(1000));

    // We use FIFO mode
    usart.cr1().modify(|_, w| w.fifoen().enabled());

    cortex_m::interrupt::free( |cs| {
        COMM_STATE.borrow(cs).replace(MaybeUninit::new(CommState{
            usart,
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
        }
        // Enable RX interrupt
        // state.usart.cr3().modify(|_, w| w.rxftie().enabled());
        state.usart.cr1().modify(|_, w| w.rxneie().enabled());
        unsafe {
            stm32h7s::NVIC::unmask(Interrupt::USART3);
        }

        // Finally, enable USART, receiver and transmitter
        state.usart.cr1().modify(|_, w| w
            .re().enabled()
            .te().enabled()
            .ue().enabled());

    });

    defmt::info!("USART3 is ready!");

    &COMM_STATE
}

#[inline]
fn usart3_rx(rx_buffer: &RXRingBuffer) {
    // We use a buffer instead of directly writing to rx_buffer to prevent excessive locking
    // If we are unable to extract all data from RXFIFO, next interrupt will finish the job!
    let mut buffer: Vec<u8, 16> = Vec::new();
    let read = with(&COMM_STATE, |state| {
        while state.usart.isr().read().rxfne().bit_is_set() && buffer.len() <= buffer.capacity() {
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
    let result = postcard::from_bytes_cobs::<UplinkMsg>(buff);
    match result {
        Ok(msg) => {
            Some(msg)
        },
        Err(err) => {
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
        defmt::info!("{}", advance);
        for byte in state.msg_buffer[0..state.msg_buffer_ptr].iter() {
            defmt::info!("{:x}", byte);
        }
        let msg = try_decode_message(&mut state.msg_buffer[0..state.msg_buffer_ptr]);
        defmt::info!("Received msg");
        match msg {
            Some(_) => { ack(state); defmt::info!("properly decoded"); },
            None => { nack(state); defmt::info!("failed decode"); }
        };
        state.msg_buffer_ptr = 0;
        msg
    } else {
        None
    }

}

// This interrupt is designed to block as little as possible, in order to allow nearly
// parallel receiving of information and processing.
#[interrupt]
fn USART3() {

    let rx_buffer = unsafe {
        // This is safe, as the ring buffers handle "thread safety" and are static, const variables
        // (They do have interior mutability!)
        &RX_BUFFER.as_ref().unwrap()
    };

    usart3_rx(rx_buffer);
    // Note that the interrupt flags are cleared upon emptying of RX FIFO
}