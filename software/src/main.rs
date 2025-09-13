use chrono::{self, DateTime, TimeZone, Utc};
use common::comm_messages::UplinkMsg::{ClearBuffer, PushFracn, PushPLLChange, StartNow, StopNow};
use common::comm_messages::{MAX_UPLINK_MSG_SIZE, UplinkMsg};
use common::sequence::Sequence;
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};
use std::fmt::Write;
use std::fs;
use std::io::{ErrorKind, Read};
use std::time::Duration;

mod orders;
mod sequence;

// Pseudorandom sequence (PRSeq) generation:
// A file is used to read the "order frequencies" (used to fine-tune the system),
// which specifies a series of intervals in time and frequency where a
// PRSeq in frequency is generated.
// The pseudorandomness of the sequence is further made time-dependent by the time seed.
// Each sequenced is seeded by the hash of the UTC time of the biggest multiple of
// TIME_SEED_ROUND_S seconds that's before the start of the sequence. This rounding
// reduces dependency on very precise clock.

fn find_port() -> Result<String, &'static str> {
    let ports = serialport::available_ports().unwrap();
    for port in ports {
        if let SerialPortType::UsbPort(info) = port.port_type {
            if let Some(m) = info.manufacturer {
                if m == "STMicroelectronics" {
                    println!("Chosen port {}", port.port_name);
                    return Ok(port.port_name);
                }
            }
        }
    }

    Err("ST-Link VCOM port not found")
}

fn frequencies_to_str(freqs: &Vec<(f64, f64)>) -> String {
    let mut out = String::new();

    for freq in freqs {
        writeln!(&mut out, "{:.6},{:.6}", freq.0, freq.1).unwrap();
    }

    out
}

fn send_seq(port: &mut Box<dyn SerialPort>, seq: &Sequence) -> Result<(), &'static str> {
    for slice in seq.fracn_buffer.chunks(32) {
        let mut fixedslice: [u16; 32] = [0; 32];
        // The rest of elements may be left zeroed, as we pass the len separately
        fixedslice[..slice.len()].copy_from_slice(slice);
        let cmd = PushFracn(slice.len() as u8, fixedslice);
        send(port, &cmd).unwrap();
    }

    for pll in &seq.pllchange_buffer {
        send(port, &PushPLLChange(*pll)).unwrap();
    }

    Ok(())
}

// Tries to send data, waiting for acknowledge and retrying
fn send(port: &mut Box<dyn SerialPort>, msg: &UplinkMsg) -> Result<(), &'static str> {
    let mut databuf: [u8; MAX_UPLINK_MSG_SIZE] = [0; MAX_UPLINK_MSG_SIZE];
    let try_encoded = postcard::to_slice_cobs(msg, &mut databuf);
    let data = if let Ok(data) = try_encoded {
        data
    } else {
        return Err("Error decoding");
    };

    const RETRIES: usize = 4;

    port.clear(serialport::ClearBuffer::Input).unwrap();

    let mut numtry = 0;

    while numtry < RETRIES {
        port.write_all(data).unwrap();
        port.flush().unwrap();
        println!("Sent try {}, waiting for reply...", numtry + 1);

        let mut read_buffer: [u8; 1] = [0];
        let try_read = port.read(&mut read_buffer);
        if let Err(e) = try_read {
            if e.kind() == ErrorKind::TimedOut {
                println!("Timed out");
                break;
            } else {
                return Err("I/O error");
            }
        }

        if read_buffer[0] == 0 {
            println!("NoAck received, trying again!");
            // no ack, try again...
        } else {
            return Ok(());
        }
        numtry += 1;
    }

    Err("Too many tries without reply")
}

fn sleep_until_precise(start_date: DateTime<Utc>, until_off_us: i64) {
        loop {
        let now_exact = Utc::now();
        let offset_us = now_exact
            .signed_duration_since(start_date)
            .num_microseconds()
            .unwrap();

        const BUSY_LOOP_MARGIN_US = 50_000;
        let remain = until_off_us - offset_us;

        if remain <= 0 {
            // Ready to start 
            break;
        } else if remain > BUSY_LOOP_MARGIN_US {
            std::thread::sleep(Duration::from_micros((remain - BUSY_LOOP_MARGIN_US) as u64));
        } else {
            // Busy loop
        }
    }
}

fn main() {
    let mut pargs = pico_args::Arguments::from_env();

    // Only generate the time-freq CSV, don't do anything with the serial ports
    let dry = pargs.contains(["-", "--dry"]);
    if dry {
        println!("Running in dry mode");
    }

    let orders_path: String = pargs
        .opt_free_from_str()
        .unwrap()
        .unwrap_or(String::from("orders.csv"));

    let out_path: String = pargs
        .opt_value_from_str("--out")
        .unwrap()
        .unwrap_or(String::from("freqs.csv"));

    let orders = orders::parse_orders(fs::read_to_string(orders_path).unwrap()).unwrap();
    println!("Read {} orders", orders.len());

    let date_str: Option<String> = pargs.opt_value_from_str("--date").unwrap();
    let date = match date_str {
        None => chrono::Utc::now(),
        Some(str) => chrono::DateTime::parse_from_rfc2822(str.as_str())
            .unwrap()
            .to_utc(),
    };
    let start_epoch = sequence::find_start_epoch(date);
    println!(
        "Sequence will start at epoch {}, which is {}s from now",
        start_epoch,
        start_epoch - chrono::Utc::now().timestamp()
    );

    // Note that this seeding is good enough as rand does some "entropy increasing" on the seed
    let plan = sequence::build_upload_plan(orders, start_epoch);
    println!("Built upload plan with {} uploads", plan.len(),);

    let freqs = sequence::build_frequencies(&plan, start_epoch);
    fs::write(&out_path, frequencies_to_str(&freqs)).unwrap();
    println!("Written frequencies to file {}", out_path);

    if !dry {
        let port_name = find_port().unwrap();
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_secs_f64(0.5))
            .flow_control(FlowControl::None)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .data_bits(DataBits::Eight)
            .open()
            .expect("Failed to open STM32 port");

        let start_date = Utc.timestamp_opt(start_epoch, 0).unwrap();
        let mut ctr = 0;
        
        for (&upload_off_us, seq) in &plan {
            println!("Waiting to upload sequence number {}", ctr);
            sleep_until_precise(start_date, upload_off_us);

            send(&mut port, &ClearBuffer());
            send_seq(&mut port, seq);
            if ctr == 0 {
                println!("Waiting to start first sequence");
                sleep_until_precise(start_date, 0);
                send(&mut port, &StartNow());
            }
            ctr += 1;
        }

        println!("Sequence finished");
    }
}
