use std::fmt::Write;
use std::{fs};
use std::cmp::min;
use std::io::{ErrorKind, Read};
use std::time::{Duration, Instant};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};
use regex::Regex;
use common::sequence::{PLLChange, Sequence};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use pico_args;
use chrono;
use chrono::{DateTime, Utc};
use common::comm_messages::{UplinkMsg, MAX_UPLINK_MSG_SIZE};
use common::comm_messages::UplinkMsg::{ClearBuffers, Ping, PushFracn, PushPLLChange, StartNow, StopNow};
// Pseudorandom sequence (PRSeq) generation:
// A file is used to read the "order frequencies" (used to fine-tune the system),
// which specifies a series of intervals in time and frequency where a
// PRSeq in frequency is generated.
// The pseudorandomness of the sequence is further made time-dependent by the time seed.
// Each sequenced is seeded by the hash of the UTC time of the biggest multiple of
// TIME_SEED_ROUND_S seconds that's before the start of the sequence. This rounding
// reduces dependency on very precise clock.

const TIME_SEED_ROUND_S: i64 = 60;
const FREF_HZ: f64 = 12_000_000.0;

fn find_port() -> Result<String, &'static str> {
    let ports = serialport::available_ports().unwrap();
    for port in ports {
        if let SerialPortType::UsbPort(info) = port.port_type {
            if let Some(m) = info.manufacturer { if m == "STMicroelectronics" {
                println!("Chosen port {}", port.port_name);
                return Ok(port.port_name);
            }}
        }
    }

    Err("ST-Link VCOM port not found")
}

// The sequencer will generate a pseudo-random sequence that spends t_us
// on band of width bandwidth_Hz centered around freq_Hz, with n frequency
// changes.
struct FrequencyOrder {
    t_us: u32,
    freq_hz: u32,
    bandwidth_hz: u32,
    n: usize,
}

fn parse_orders(file: String) -> Result<Vec<FrequencyOrder>, &'static str> {
    let r = Regex::new(r"(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)").unwrap();
    let mut out: Vec<FrequencyOrder> = Vec::new();
    let lines = file.lines();

    for line in lines {
        let captures = r.captures(line).unwrap();
        out.push(FrequencyOrder {
            t_us: captures.get(1).unwrap().as_str().parse().unwrap(),
            freq_hz: captures.get(2).unwrap().as_str().parse().unwrap(),
            bandwidth_hz: captures.get(3).unwrap().as_str().parse().unwrap(),
            n: captures.get(4).unwrap().as_str().parse().unwrap()
        })
    }

    Ok(out)
}

struct SubSequence {
    change: PLLChange,
    fracn: Vec<u16>,
}


fn build_subsequence(order: FrequencyOrder, seed: u64) -> Result<SubSequence, &'static str> {
    let mut fracn_buf = Vec::new();
    fracn_buf.reserve(order.n);

    let mut rng = ChaCha20Rng::seed_from_u64(seed);

    // Divn and divp are set to maximize the resolution. It turns out
    // the following (found by Mathematica) settings work:
    let fhigh = order.freq_hz as f64 + 0.5 * order.bandwidth_hz as f64;
    let flow = order.freq_hz as f64 - 0.5 * order.bandwidth_hz as f64;
    if fhigh <= flow || flow <= 0.0 {
       return Err("Invalid configuration");
    }
    let divnf = fhigh / (fhigh - flow) - 2.0;
    let divpf = FREF_HZ * (2.0 + divnf) / fhigh;

    // To determine if vcosel = 0 or 1, we compute the vco frequency in both
    // cases, and select the most appropiate
    let min_vcofreq_1 = 2.0 * FREF_HZ * (divnf + 1.0);
    let max_vcofreq_1 = 2.0 * FREF_HZ * (divnf + 2.0);
    let min_vcofreq_0 = min_vcofreq_1 / 2.0;
    let max_vcofreq_0 = max_vcofreq_1 / 2.0;

    const VCOSEL0_MIN_FREQ: f64 = 384_000_000.0;
    const VCOSEL0_MAX_FREQ: f64 = 1672_000_000.0;
    const VCOSEL1_MIN_FREQ: f64 = 150_000_000.0;
    const VCOSEL1_MAX_FREQ: f64 = 420_000_000.0;

    let vcosel = if min_vcofreq_1 >= VCOSEL1_MIN_FREQ && max_vcofreq_1 <= VCOSEL1_MAX_FREQ {
        true
    } else if min_vcofreq_0 >= VCOSEL0_MIN_FREQ && max_vcofreq_0 <= VCOSEL0_MAX_FREQ {
        false
    } else {
        println!("divnf = {} divpf = {}", divnf, divpf);
        println!("min_vcofreq_0 = {} max_vcofreq_0 = {}", min_vcofreq_0, max_vcofreq_0);
        println!("min_vcofreq_1 = {} max_vcofreq_1 = {}", min_vcofreq_1, max_vcofreq_1);
        return Err("No VCO configuration satisfies desired frequency range");
    };

    let divn = divnf as u16;
    if divn < 7 || divn > 419 {
        return Err("No DIVN configuration satisfies desired frequency range");
    }
    let divp = divpf as u8;
    if divp > 127 {
        return Err("No DIVP configuration satisfies desired frequency range");
    }

    let mut t = 0.0;
    for i in 0..order.n {
        // fac is uniformly distributed on [-0.5, 0.5), and represents
        // our desired position in the bandwidth
        let fac = rng.random::<f64>() - 0.5;
        let freq = order.freq_hz as f64 + fac * (order.bandwidth_hz as f64);
        // Actual fout = fref * (divn + 1 + fracn/2^13) / divp, so we find
        let fracnf = 8192.0 * (divpf * freq - FREF_HZ - divnf * FREF_HZ) / FREF_HZ;
        let fracn = if fracnf < 0.0 {
            log::warn!("fracn went below 0, clamping");
            0
        } else if fracnf >= 8192.0 {
            log::warn!("fracn went above 8191, clamping");
            8191
        } else {
            fracnf as u16
        };
        fracn_buf.push(fracn);
    }

    Ok(SubSequence {
        change: PLLChange {
            for_ticks: order.n,
            start_tick: 0,
            divn,
            vcosel,
            divp,
            tim_us: ((order.t_us as f64 / order.n as f64) as usize) as u32,
        },

        fracn: fracn_buf,
    })
}

fn build_sequence(orders: Vec<FrequencyOrder>, seed: u64) -> Result<Sequence, &'static str> {
    let mut out = Sequence {
        fracn_buffer: heapless::Vec::new(),
        pllchange_buffer: heapless::Vec::new(),
    };

    for order in orders {
        let mut subseq = build_subsequence(order, seed)?;
        // Offset PLLChange index!
        subseq.change.start_tick += out.fracn_buffer.len();
        out.pllchange_buffer.push(subseq.change).unwrap_or_else(|_| panic!());
        for fracni in subseq.fracn {
            out.fracn_buffer.push(fracni).unwrap();
        }
    }

    Ok(out)
}

fn find_start_epoch(date: chrono::DateTime<chrono::Utc>) -> i64 {
    // We add a bit of margin, to prevent the hypothetical case of starting a few milliseconds
    // before the next epoch and not having enough time to send the stuff to the transmitter
    const MARGIN_S: i64 = 1;
    ((date.timestamp() + MARGIN_S) / TIME_SEED_ROUND_S) * TIME_SEED_ROUND_S + TIME_SEED_ROUND_S
}

fn encode(msg: &UplinkMsg) -> Vec<u8> {
    let mut buffer = Vec::new();
    buffer.resize(common::comm_messages::MAX_UPLINK_MSG_SIZE, 0);
    let slice = postcard::to_slice_cobs(msg, buffer.as_mut_slice()).unwrap();
    slice.to_vec()
}

// Returns unix epoch (in f64 seconds) - frequency pairs (in Hz)
// This function has some fine-tuning parameters to match the timing of the actual transmitter!
fn build_frequencies(seq: &Sequence, start_timestamp: i64) -> Vec<(f64, f64)> {
    const PLLCHANGE_S: f64 = 5e-6;

    let mut out = Vec::new();
    let mut t = start_timestamp as f64;

    for change in seq.pllchange_buffer.iter() {
        t += PLLCHANGE_S;
        for i in 0..change.for_ticks {
            let fracn = seq.fracn_buffer[change.start_tick + i] as f64;
            let divn = change.divn as f64;
            let divp = change.divp as f64;
            let freq = FREF_HZ * (divn + 1.0 + fracn / 8192.0) / (divp + 1.0);
            out.push((t, freq));
            t += change.tim_us as f64 * 1e-6;
        }
    }

    out
}

fn frequencies_to_str(freqs: &Vec<(f64, f64)>) -> String {
    let mut out = String::new();

    for freq in freqs {
        writeln!(&mut out, "{:.6},{:.6}", freq.0, freq.1).unwrap();
    }

    out
}

fn send_seq(port: &mut Box<dyn SerialPort>, seq: Sequence) -> Result<(), &'static str> {
    for slice in seq.fracn_buffer.chunks(32) {
        let mut fixedslice: [u16; 32] = [0; 32];
        // The rest of elements may be left zeroed, as we pass the len separately
        fixedslice[..slice.len()].copy_from_slice(slice);
        let mut cmd = PushFracn(slice.len() as u8, fixedslice);
        send(port, &cmd).unwrap();
    }

    for pll in seq.pllchange_buffer {
        send(port, &PushPLLChange(pll)).unwrap();
    }

    Ok(())
}

// Tries to send data, waiting for acknowledge and retrying
fn send(port: &mut Box<dyn SerialPort>, msg: &UplinkMsg) -> Result<(), &'static str> {
    let mut databuf: [u8; MAX_UPLINK_MSG_SIZE] = [0; MAX_UPLINK_MSG_SIZE];
    let try_encoded = postcard::to_slice_cobs(msg, &mut databuf);
    let data = if try_encoded.is_err() {
        return Err("Unable to encode message");
    } else {
        try_encoded.unwrap()
    };

    const RETRIES: usize = 4;
    const TIMEOUT_S: f64 = 0.5;

    port.clear(ClearBuffer::Input).unwrap();

    let mut numtry = 0;
    let start_time = Instant::now();

    while numtry < RETRIES {
        port.write(data).unwrap();
        port.flush();
        println!("Sent try {}, waiting for reply...", numtry + 1);

        let mut read_buffer: [u8; 1] = [0];
        loop {
            let try_read = port.read(&mut read_buffer);
            if let Err(e) = try_read {
                if e.kind() == ErrorKind::TimedOut {
                    println!("Timed out");
                    break;
                } else {
                    return Err("I/O error");
                }
            } else {
                break;
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

fn main() {
    let mut pargs = pico_args::Arguments::from_env();

    // Only generate the time-freq CSV, don't do anything with the serial ports
    let dry = pargs.contains(["-", "--dry"]);
    if dry {
        println!("Running in dry mode");
    }


    let orders_path: String = pargs.opt_free_from_str()
        .unwrap().unwrap_or(String::from("orders.csv"));

    let out_path: String = pargs.opt_value_from_str("--out")
        .unwrap().unwrap_or(String::from("freqs.csv"));

    let orders = parse_orders(fs::read_to_string(orders_path).unwrap()).unwrap();
    println!("Read {} orders", orders.len());

    let date_str: Option<String> = pargs.opt_value_from_str("--date").unwrap();
    let date = match date_str {
        None => chrono::Utc::now(),
        Some(str) =>
            chrono::DateTime::parse_from_rfc2822(str.as_str()).unwrap().to_utc(),
    };
    let start_epoch = find_start_epoch(date);
    println!("Sequence will start at epoch {}, which is {}s from now", start_epoch,
             start_epoch - chrono::Utc::now().timestamp());

    // Note that this seeding is good enough as rand does some "entropy increasing" on the seed
    let seq = build_sequence(orders, start_epoch as u64).unwrap();
    println!("Built sequence with {} pll changes and {} fracn values",
             seq.pllchange_buffer.len(), seq.fracn_buffer.len());
    let freqs = build_frequencies(&seq, start_epoch);
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
            .open().expect("Failed to open STM32 port");

        // Send the sequence
        send(&mut port, &StopNow());
        send(&mut port, &ClearBuffers());
        send_seq(&mut port, seq);

        println!("Waiting to start sequence");
        // Trigger the start at a relatively precise time
        while Utc::now().timestamp() < start_epoch {
            std::thread::sleep(Duration::from_millis(100));
        }

        send(&mut port, &StartNow());
        println!("Sequence started");

    }


}
