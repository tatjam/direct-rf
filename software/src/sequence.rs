use std::{collections::BTreeMap, ops::Div};

use crate::orders::FrequencyOrder;
use common::sequence::{MAX_DIVN_CHANGES, MAX_SEQUENCE_LEN, PLLChange, Sequence};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

const FREF_HZ: f64 = 12_000_000.0;

/// Maps UNIX timestamps to the sequence that must be uploaded at that moment
type UploadPlan = BTreeMap<i64, Sequence>;

pub struct SubSequence {
    change: PLLChange,
    fracn: Vec<u16>,
}

pub fn build_subsequence(order: &FrequencyOrder, seed: u64) -> Result<SubSequence, &'static str> {
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

// Appends the sequence to the given one, or creates a new one if it doesn't fit and
// returns the final sequence to be stored in the upload map
pub fn build_sequence(
    order: &FrequencyOrder,
    base: &mut Sequence,
    is_last: bool,
    toff_us: u64,
    t_start: i64,
) -> Option<Sequence> {
    let mut out = None;

    // TODO: seed
    let seed = 0;

    let mut subseq = build_subsequence(order, seed).unwrap();
    if base.fracn_buffer.len() + subseq.fracn.len() > MAX_SEQUENCE_LEN
        || base.pllchange_buffer.len() + 1 > MAX_DIVN_CHANGES
        || is_last
    {
        // We ran out of space in the base seq, create a new one
        out = Some(base.expensive_copy());
        *base = Default::default();
    }

    // Offset PLLChange index!
    subseq.change.start_tick += base.fracn_buffer.len();
    base.pllchange_buffer
        .push(subseq.change)
        .unwrap_or_else(|_| panic!());

    for fracni in subseq.fracn {
        base.fracn_buffer.push(fracni).unwrap();
    }

    out
}

// Returns upload time estimate in us
pub fn estimate_upload_time(_seq: &Sequence) -> u64 {
    1_000_000
}

// start_tstamp is the (approximate) time the sequence will start
pub fn build_upload_plan(orders: Vec<FrequencyOrder>, start_tstamp: i64) -> UploadPlan {
    let mut out = UploadPlan::new();

    let mut work_seq: Sequence = Default::default();
    let mut last_upload_off_us: i64 = 0;
    let mut toff_us: u64 = 0;

    // peekable so we can know we are in the last order to finish the sequence
    let mut it = orders.iter().peekable();

    while let Some(order) = it.next() {
        let maybe_done = build_sequence(
            order,
            &mut work_seq,
            it.peek().is_none(),
            toff_us,
            start_tstamp,
        );

        if let Some(done_seq) = maybe_done {
            let preempt = estimate_upload_time(&done_seq);
            let net_off_us = toff_us as i64 - preempt as i64;
            // Uploads must be well ordered, this could happen if a sequence is too short (<1 second)
            assert!(net_off_us > last_upload_off_us);
            // euclid div to prevent negative net_off_us from being too late
            let net_off_s = net_off_us.div_euclid(1_000_000) as i64;
            let time = start_tstamp + net_off_s;

            out.insert(time, done_seq);
            last_upload_off_us = net_off_us;
        }
        toff_us += order.t_us as u64;
    }

    out
}

pub fn find_start_epoch(date: chrono::DateTime<chrono::Utc>) -> i64 {
    // We add a bit of margin, to prevent the hypothetical case of starting a few milliseconds
    // before the next epoch and not having enough time to send the stuff to the transmitter

    const TIME_SEED_ROUND_S: i64 = 10;
    const MARGIN_S: i64 = 1;
    ((date.timestamp() + MARGIN_S) / TIME_SEED_ROUND_S) * TIME_SEED_ROUND_S + TIME_SEED_ROUND_S
}

// Returns unix epoch (in f64 seconds) - frequency pairs (in Hz)
// This function has some fine-tuning parameters to match the timing of the actual transmitter!
pub fn build_frequencies(plan: &UploadPlan, start_timestamp: i64) -> Vec<(f64, f64)> {
    const PLLCHANGE_S: f64 = 5e-6;

    let mut out = Vec::new();
    let mut t = start_timestamp as f64;

    for (_, seq) in plan {
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
    }

    out
}
