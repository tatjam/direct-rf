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
                &mut baseband[0..self.settings.adjust_samps],
                &mut reference[0..self.settings.adjust_samps],
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
            // Advance baseband so it's one second before the start of freq
            let first_epoch = self.freqs.get_first_epoch().floor();
            self.baseband.seek_epoch(first_epoch - 1.0);
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

        let adjust_fft = fft_planner.plan_fft_forward(settings.adjust_samps);
        let adjust_ifft = fft_planner.plan_fft_inverse(settings.adjust_samps);
        let mut adjust_scratch = Vec::<Sample>::new();
        adjust_scratch.resize(adjust_fft.get_inplace_scratch_len(), Sample::new(0.0, 0.0));
        assert_eq!(adjust_scratch.len(), adjust_ifft.get_inplace_scratch_len());

        let correlate_fft = fft_planner.plan_fft_forward(settings.correlate_samps);
        let correlate_ifft = fft_planner.plan_fft_inverse(settings.correlate_samps);
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
// Mutates both buffers, leaving a with the result of the correlation
fn correlate(
    fft: &Arc<dyn Fft<Scalar>>,
    ifft: &Arc<dyn Fft<Scalar>>,
    scratch: &mut Vec<Sample>,
    a: &mut [Sample],
    b: &mut [Sample],
) -> (i32, Scalar) {
    fft.process_with_scratch(a, scratch);
    fft.process_with_scratch(b, scratch);

    // Multiply a by b's conjugate, storing in a
    for pair in a.iter_mut().zip(b) {
        *pair.0 = *pair.0 * (*pair.1).conj();
    }

    ifft.process_with_scratch(a, scratch);

    // We collect the maximum and second maximum absolute value, to get the PSR
    // (The max value is max[0], the second max is max[1]
    let mut max = [Scalar::NEG_INFINITY, Scalar::NEG_INFINITY];
    let mut maxidx: usize = 0;
    for (idx, v) in a.iter().enumerate() {
        if v.abs() > max[0] {
            max[1] = max[0];
            max[0] = v.abs();
            maxidx = idx;
        }
    }

    let offset = {
        if maxidx > a.len() / 2 - 1 {
            // a must be delayed to match b
            -((a.len() - maxidx) as i32)
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
