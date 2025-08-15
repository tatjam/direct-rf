use std::cmp::min;
use rustfft::num_complex::Complex;
use crate::stream::{Sample, Scalar, StreamedBaseband, StreamedSamplesFreqs};

pub struct DspSettings {
    // How much time do we use to correlate the two signals (rounded up to number of samples) to
    // find the alignment between the two
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

    first_run: bool,
}

impl Dsp {

    fn correlate(self: &mut Self, baseband: &Vec<Sample>, reference: &Vec<Sample>, skip_adjust: bool)
        -> (i32, Scalar) {
        debug_assert!(baseband.len() >= self.settings.correlate_samps);
        debug_assert!(baseband.len() >= self.settings.adjust_samps);
        debug_assert!(reference.len() >= self.settings.correlate_samps);
        debug_assert!(reference.len() >= self.settings.adjust_samps);

        if !skip_adjust {
            let adjust_res = correlate(&baseband[0..self.settings.adjust_samps],
                                       &reference[0..self.settings.adjust_samps]);

            if adjust_res.1 > self.settings.min_psr {
                return adjust_res;
            }
        }

        correlate(&baseband[0..self.settings.correlate_samps],
                  &reference[0..self.settings.correlate_samps])
    }

    // Each run does:
    // If first run, an start correlation, otherwise, an adjust correlation
    // If no significant correlation peak is found, run a full correlation
    // Offset signal with the correlation
    // Find the product of the two signals, with offset and store that as the result
    // Decimate down to the output sample rate
    pub fn run_once(self: &mut Self) -> Vec<Sample> {
        // Load the samples from the sources
        let mut baseband = self.baseband.get_next(self.settings.run_samps);
        let mut reference = self.freqs.get_next(self.settings.run_samps);

        zeropad_shortest(&mut baseband, &mut reference, self.settings.correlate_samps as usize);
        debug_assert_eq!(baseband.len(), reference.len());

        let (offset, psr)  = self.correlate(&baseband, &reference, self.first_run);
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

        // This does mixing (multiply the two signals) and decimation (take every nth sample)
        baseband.iter().zip(reference).map(|(a, b)| a*b).step_by(self.settings.output_decimate).collect()
    }

    pub fn new(baseband: StreamedBaseband, freqs: StreamedSamplesFreqs, settings: DspSettings) -> Self {
        Self {
            baseband,
            freqs,
            settings,
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
fn correlate(a: &[Sample], b: &[Sample]) -> (i32, Scalar) {
    (0, 0.0)
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