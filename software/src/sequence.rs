use crate::orders::FrequencyOrder;
use common::sequence::{PLLChange, Sequence};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

const FREF_HZ: f64 = 12_000_000.0;

pub struct SubSequence {
    change: PLLChange,
    fracn: Vec<u16>,
}

pub fn build_subsequence(order: FrequencyOrder, seed: u64) -> Result<SubSequence, &'static str> {
    let mut fracn_buf = Vec::with_capacity(order.n);

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
    const VCOSEL0_MAX_FREQ: f64 = 1_672_000_000.0;
    const VCOSEL1_MIN_FREQ: f64 = 150_000_000.0;
    const VCOSEL1_MAX_FREQ: f64 = 420_000_000.0;

    let vcosel = if min_vcofreq_1 >= VCOSEL1_MIN_FREQ && max_vcofreq_1 <= VCOSEL1_MAX_FREQ {
        true
    } else if min_vcofreq_0 >= VCOSEL0_MIN_FREQ && max_vcofreq_0 <= VCOSEL0_MAX_FREQ {
        false
    } else {
        println!("divnf = {} divpf = {}", divnf, divpf);
        println!(
            "min_vcofreq_0 = {} max_vcofreq_0 = {}",
            min_vcofreq_0, max_vcofreq_0
        );
        println!(
            "min_vcofreq_1 = {} max_vcofreq_1 = {}",
            min_vcofreq_1, max_vcofreq_1
        );
        return Err("No VCO configuration satisfies desired frequency range");
    };

    let divn = divnf as u16;
    if !(7..=419).contains(&divn) {
        return Err("No DIVN configuration satisfies desired frequency range");
    }
    let divp = divpf as u8;
    if divp > 127 {
        return Err("No DIVP configuration satisfies desired frequency range");
    }

    for _ in 0..order.n {
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

pub fn build_sequence(orders: Vec<FrequencyOrder>, seed: u64) -> Result<Sequence, &'static str> {
    let mut out = Sequence {
        fracn_buffer: heapless::Vec::new(),
        pllchange_buffer: heapless::Vec::new(),
    };

    for order in orders {
        let mut subseq = build_subsequence(order, seed)?;
        // Offset PLLChange index!
        subseq.change.start_tick += out.fracn_buffer.len();
        out.pllchange_buffer
            .push(subseq.change)
            .unwrap_or_else(|_| panic!());
        for fracni in subseq.fracn {
            out.fracn_buffer.push(fracni).unwrap();
        }
    }

    Ok(out)
}

pub fn find_start_epoch(date: chrono::DateTime<chrono::Utc>) -> i64 {
    // We add a bit of margin, to prevent the hypothetical case of starting a few milliseconds
    // before the next epoch and not having enough time to send the stuff to the transmitter

    const TIME_SEED_ROUND_S: i64 = 60;
    const MARGIN_S: i64 = 1;
    ((date.timestamp() + MARGIN_S) / TIME_SEED_ROUND_S) * TIME_SEED_ROUND_S + TIME_SEED_ROUND_S
}

// Returns unix epoch (in f64 seconds) - frequency pairs (in Hz)
// This function has some fine-tuning parameters to match the timing of the actual transmitter!
pub fn build_frequencies(seq: &Sequence, start_timestamp: i64) -> Vec<(f64, f64)> {
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
