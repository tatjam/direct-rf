use std::{env, fs};
use std::time::{Instant};
use serialport::{SerialPort, SerialPortType};
use regex::Regex;
use common::sequence::{PLLChange, Sequence};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use log::warn;
// Pseudorandom sequence (PRSeq) generation:
// A file is used to read the "order frequencies" (used to fine-tune the system),
// which specifies a series of intervals in time and frequency where a
// PRSeq in frequency is generated.
// The pseudorandomness of the sequence is further made time-dependent by the time seed.
// Each sequence is seeded by the Galileo epoch at the time of its start, flooring said start
// time to intervals of TIME_SEED_ROUND_S seconds.
// This guarantees that receivers and emitters don't need an excessively well synchronized
// clock. Sequences are started as closely to the start of the floored galileo epoch as possible
// (by the local clock time)

const TIME_SEED_ROUND_S: f64 = 10.0;
const FREF_HZ: f64 = 12_000_000.0;

fn find_port() -> Result<String, &'static str> {
    let ports = serialport::available_ports().unwrap();
    for port in ports {
        match port.port_type {
            SerialPortType::UsbPort(info) => {

                return Result::Ok(port.port_name);
            }
            _ => {
                continue;
            }
        }
    }

    Result::Err("ST-Link VCOM port not found")
}

// The sequencer will generate a pseudo-random sequence that spends t_us
// on band of width bandwidth_Hz centered around freq_Hz, with n frequency
// changes.
struct FrequencyOrder {
    t_us: u32,
    freq_Hz: u32,
    bandwidth_Hz: u32,
    n: usize,
}

fn parse_freqs(file: String) -> Result<Vec<FrequencyOrder>, &'static str> {
    let r = Regex::new(r"(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)").unwrap();
    let mut out: Vec<FrequencyOrder> = Vec::new();
    let mut lines = file.lines();

    for line in lines {
        let captures = r.captures(line).unwrap();
        out.push(FrequencyOrder {
            t_us: captures.get(1).unwrap().as_str().parse().unwrap(),
            freq_Hz: captures.get(2).unwrap().as_str().parse().unwrap(),
            bandwidth_Hz: captures.get(3).unwrap().as_str().parse().unwrap(),
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
    let fhigh = order.freq_Hz as f64 + 0.5 * order.bandwidth_Hz as f64;
    let flow = order.freq_Hz as f64 - 0.5 * order.bandwidth_Hz as f64;
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

    for i in 0..order.n {
        // fac is uniformly distributed on [-0.5, 0.5), and represents
        // our desired position in the bandwidth
        let fac = rng.random::<f64>() - 0.5;
        let freq = order.freq_Hz as f64 + fac * (order.bandwidth_Hz as f64);
        // Actual fout = fref * (divn + 1 + fracn/2^13) / divp, so we find
        let fracnf = 8192.0 * ((freq + divpf * freq) / FREF_HZ - 1.0 - divnf);
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
            tim_us: (order.n * order.t_us as usize) as u32,
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
        let mut subseq = build_subsequence(order, seed).unwrap();
        // Offset PLLChange index!
        subseq.change.start_tick += out.fracn_buffer.len();
        out.pllchange_buffer.push(subseq.change);
        for fracni in subseq.fracn {
            out.fracn_buffer.push(fracni).unwrap();
        }
    }

    Ok(out)
}

fn get_seed(time: Instant) -> u32 {
    0
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let freqs_path = if(args.len() < 2) {
        String::from("freqs.csv")
    } else {
        args[1].clone()
    };

    let freqs = parse_freqs(fs::read_to_string(freqs_path).unwrap()).unwrap();
    let port = find_port().unwrap();


}
