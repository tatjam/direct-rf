use crate::stream::{Sample, Scalar, StreamedBaseband, StreamedSamplesFreqs};
use log::info;
use rustfft::num_complex::ComplexFloat;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct DspSettings {
    // How much time do we use to correlate the two signals (rounded up to number of samples) to // find the alignment between the two
    pub correlate_samps: usize,

    // Same as correlate_samps, but for every run that's not the first one
    pub adjust_samps: usize,

    // How much time per run (rounded up to number of samples). This allows adjusting how
    // often correlation and alignment happens
    pub run_samps: usize,

    // Decimation for the output "mixed" signal
    pub output_decimate: usize,

    // Minimum PSR (peak-to-sidelobe ratio) for a correlation to be considered successful
    pub min_psr: Scalar,
}

pub struct Dsp {
    baseband: StreamedBaseband,
    freqs: StreamedSamplesFreqs,
    settings: DspSettings,

    adjust_fft: Arc<dyn Fft<Scalar>>,
    adjust_ifft: Arc<dyn Fft<Scalar>>,
    correlate_fft: Arc<dyn Fft<Scalar>>,
    correlate_ifft: Arc<dyn Fft<Scalar>>,
    adjust_scratch: Vec<Sample>,
    correlate_scratch: Vec<Sample>,
    abuf: Vec<Sample>,
    bbuf: Vec<Sample>,

    first_run: bool,
}

impl Dsp {
    fn correlate(
        self: &mut Self,
        baseband: &mut Vec<Sample>,
        reference: &mut Vec<Sample>,
        skip_adjust: bool,
    ) -> (i32, Scalar) {
        debug_assert!(baseband.len() >= self.settings.correlate_samps);
        debug_assert!(baseband.len() >= self.settings.adjust_samps);
        debug_assert!(reference.len() >= self.settings.correlate_samps);
        debug_assert!(reference.len() >= self.settings.adjust_samps);

        if !skip_adjust {
            let adjust_res = correlate(
                &self.adjust_fft,
                &self.adjust_ifft,
                &mut self.adjust_scratch,
                &baseband[0..self.settings.adjust_samps],
                &reference[0..self.settings.adjust_samps],
                &mut self.abuf[0..(self.settings.adjust_samps * 2 - 1)],
                &mut self.bbuf[0..(self.settings.adjust_samps * 2- 1)],
            );

            if adjust_res.1 >= self.settings.min_psr {
                return adjust_res;
            }
        }

        correlate(
            &self.correlate_fft,
            &self.correlate_ifft,
            &mut self.correlate_scratch,
            &mut baseband[0..self.settings.correlate_samps],
            &mut reference[0..self.settings.correlate_samps],
            &mut self.abuf,
            &mut self.bbuf
        )
    }

    // Each run does:
    // If first run, an start correlation, otherwise, an adjust correlation
    // If no significant correlation peak is found, run a full correlation
    // Offset signal with the correlation
    // Find the product of the two signals, with offset and store that as the result
    // Decimate down to the output sample rate
    pub fn run_once(self: &mut Self) -> Vec<Sample> {
        if self.first_run {
            // Advance baseband so it's half a second before the start of freq
            let first_epoch = self.freqs.get_first_epoch().floor();
            self.baseband.seek_epoch(first_epoch - 0.2);
        }

        // Load the samples from the sources
        let mut baseband = self.baseband.get_next(self.settings.run_samps);
        let mut reference = self.freqs.get_next(self.settings.run_samps);

        zeropad_shortest(
            &mut baseband,
            &mut reference,
            self.settings.correlate_samps as usize,
        );
        debug_assert_eq!(baseband.len(), reference.len());

        info!("Correlating...");
        let (offset, psr) = self.correlate(&mut baseband, &mut reference, self.first_run);
        info!(
            "Correlation peak offset {} samples, with PSR = {}dB",
            offset,
            10.0 * psr.log10()
        );
        self.first_run = false;

        if psr < self.settings.min_psr {
            panic!("Unable to correlate the two sequences");
        }

        if offset > 0 {
            // We need to delay reference, i.e. advance baseband (dropping samples)
            let new_samples = self.baseband.get_next(offset.abs() as usize);
            rotate_insert(&mut baseband, &new_samples);
        } else if offset < 0 {
            // We need to delay baseband, i.e. advance reference (dropping samples)
            let new_samples = self.freqs.get_next(offset.abs() as usize);
            rotate_insert(&mut reference, &new_samples);
        }

        debug_assert_eq!(baseband.len(), reference.len());

        // TODO: Do mixing, low pass filter and then decimate to improve SNR
        // This does mixing (multiply the two signals) and decimation (take every nth sample)
        baseband
            .iter()
            .zip(reference)
            .map(|(a, b)| a * b)
            .step_by(self.settings.output_decimate)
            .collect()
    }

    pub fn new(
        baseband: StreamedBaseband,
        freqs: StreamedSamplesFreqs,
        settings: DspSettings,
    ) -> Self {
        let mut fft_planner = FftPlanner::new();

        let fftsize_correlate = settings.correlate_samps * 2 - 1;
        let fftsize_adjust = settings.adjust_samps * 2 - 1;

        let mut abuf = Vec::new();
        let mut bbuf = Vec::new();
        assert!(settings.correlate_samps > settings.adjust_samps);
        abuf.resize(fftsize_correlate, Sample::new(0.0, 0.0));
        bbuf.resize(fftsize_correlate, Sample::new(0.0, 0.0));

        let adjust_fft = fft_planner.plan_fft_forward(fftsize_adjust);
        let adjust_ifft = fft_planner.plan_fft_inverse(fftsize_adjust);
        let mut adjust_scratch = Vec::<Sample>::new();
        adjust_scratch.resize(adjust_fft.get_inplace_scratch_len(), Sample::new(0.0, 0.0));
        assert_eq!(adjust_scratch.len(), adjust_ifft.get_inplace_scratch_len());

        let correlate_fft = fft_planner.plan_fft_forward(fftsize_correlate);
        let correlate_ifft = fft_planner.plan_fft_inverse(fftsize_correlate);
        let mut correlate_scratch = Vec::<Sample>::new();
        correlate_scratch.resize(
            correlate_fft.get_inplace_scratch_len(),
            Sample::new(0.0, 0.0),
        );
        assert_eq!(correlate_scratch.len(), correlate_ifft.get_inplace_scratch_len());

        Self {
            baseband,
            freqs,
            settings,
            adjust_fft,
            adjust_ifft,
            adjust_scratch,
            correlate_fft,
            correlate_ifft,
            correlate_scratch,
            first_run: true,
            abuf,
            bbuf
        }
    }
}

fn zeropad_shortest(a: &mut Vec<Sample>, b: &mut Vec<Sample>, min_size: usize) {
    let zero = Sample::new(0.0, 0.0);
    if a.len() < min_size {
        a.resize(min_size, zero);
    }
    if b.len() < min_size {
        b.resize(min_size, zero);
    }

    if a.len() > b.len() {
        b.resize(a.len(), zero);
    } else if b.len() > a.len() {
        a.resize(b.len(), zero);
    }
}

// Returns largest peak offset in samples and its PSR (peak-to-sidelobe ratio)
// Offset is positive if b must be delayed to match a
// abuf and bbuf must have len of 2*len(a) - 1
// Mutates both buffers, leaving a with the result of the correlation
fn correlate(
    fft: &Arc<dyn Fft<Scalar>>,
    ifft: &Arc<dyn Fft<Scalar>>,
    scratch: &mut Vec<Sample>,
    a: &[Sample],
    b: &[Sample],
    abuf: &mut [Sample],
    bbuf: &mut [Sample],
) -> (i32, Scalar) {
    debug_assert_eq!(a.len(), b.len());
    debug_assert_eq!(abuf.len(), bbuf.len());
    debug_assert!(abuf.len() >= a.len());

    abuf[0..a.len()].copy_from_slice(a);
    bbuf[0..b.len()].copy_from_slice(b);
    abuf[a.len()..].fill(Sample::new(0.0, 0.0));
    bbuf[b.len()..].fill(Sample::new(0.0, 0.0));

    fft.process_with_scratch(abuf, scratch);
    fft.process_with_scratch(bbuf, scratch);

    // Multiply a by b's conjugate, storing in a
    for pair in abuf.iter_mut().zip(bbuf) {
        *pair.0 = *pair.0 * (*pair.1).conj();
    }

    ifft.process_with_scratch(abuf, scratch);

    let csv_content = abuf.iter()
        .map(|c| format!("{},{}", c.re, c.im))
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write("debug.csv", format!("real,imag\n{}", csv_content)).unwrap();


    // We collect the maximum and second maximum absolute value, to get the PSR
    // (The max value is max[0], the second max is max[1]
    let mut max = [Scalar::NEG_INFINITY, Scalar::NEG_INFINITY];
    let mut maxidx: usize = 0;
    for (idx, v) in abuf.iter().enumerate() {
        if v.abs() > max[0] {
            max[1] = max[0];
            max[0] = v.abs();
            maxidx = idx;
        }
    }

    let offset = {
        if maxidx > abuf.len() / 2 - 1 {
            // a must be delayed to match b
            -((abuf.len() - maxidx) as i32)
        } else {
            // b must be delayed to match a
            maxidx as i32
        }
    };

    (offset, max[0] / max[1])
}

// Inserts inserted samples at the end, shifting all the other samples to the left, and
// discarding everything that runs off the array
fn rotate_insert(vec: &mut [Sample], inserted: &[Sample]) {
    debug_assert!(vec.len() > inserted.len());
    vec.rotate_left(inserted.len());
    // Overwrite
    for i in 0..inserted.len() {
        vec[vec.len() - 1 - i] = inserted[inserted.len() - 1 - i];
    }
}
