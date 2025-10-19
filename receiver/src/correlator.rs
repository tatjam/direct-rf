use std::{collections::HashMap, sync::Arc};

use ndarray::{Array1, Array2, ArrayView1, ArrayViewMut1, azip, s};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::{Fft, FftPlanner, num_complex::ComplexFloat};
use sdriq::Complex;

use crate::stream::{FreqChange, FreqOnTimes, Sample, Scalar, get_freqs_for_interval};

struct CorrelationBuffers {
    /// Accumulation buffer where the correlation results are summed
    accum_corr: Array1<Scalar>,
    /// Buffer for baseband data
    buff_rx: Array1<Scalar>,
    /// Buffer for reference baseband data
    buff_ref: Array1<Scalar>,
    /// FFT of baseband data
    fft_rx: Array1<Sample>,
    /// FFT of reference baseband data
    fft_ref: Array1<Sample>,

    /// Number of correlations performed
    accum_i: usize,
    /// Histogram of each bin to the accumulated PSR (scaled up to keep decimals)
    max_index_histogram: HashMap<usize, u64>,
}

impl CorrelationBuffers {
    pub fn new(num_windows: usize) -> Self {
        // Here we will accumulate the correlation
        let accum_corr: Array1<Scalar> = Array1::zeros(num_windows * 2);

        // Real buffers to perform the FFTs in. Note the double size as we have zero padding
        // to prevent circular correlation artefacts.
        let buff_rx: Array1<Scalar> = Array1::zeros(num_windows * 2);
        let buff_ref: Array1<Scalar> = Array1::zeros(num_windows * 2);
        // Imaginary buffers to perform the correlation. Note the +1 needed for the DC component.
        let fft_rx: Array1<Sample> = Array1::zeros(num_windows + 1);
        let fft_ref: Array1<Sample> = Array1::zeros(num_windows + 1);

        // Maps each max_index to the sum of its PSR (*10000) over the accumulation period,
        // such that picking the maximum is reasonable
        let max_index_histogram: HashMap<usize, u64> = HashMap::new();

        Self {
            accum_corr,
            buff_rx,
            buff_ref,
            fft_rx,
            fft_ref,
            accum_i: 0,
            max_index_histogram,
        }
    }
}

pub struct SpectrogramCorrelator {
    /// The number of samples on each spectrogram window
    window_size: usize,
    /// Distance between the start of two consecutive windows
    window_step: usize,

    /// Max number of windows that can be processed. Any less are tolerated as zero-padding.
    max_spectrogram_size: usize,

    /// The FFT used for spectrogram building
    window_fft: Arc<dyn Fft<Scalar>>,
    /// Scatch buffer for spectrogram building
    window_fft_scratch: Vec<Sample>,
    /// Windowing function used. Each value is duplicated to allow fast SSE multiplication
    window_function: Array1<Scalar>,

    /// The FFT used for correlation
    correlate_fft: Arc<dyn RealToComplex<Scalar>>,
    /// The IFFT used for correlation
    correlate_ifft: Arc<dyn ComplexToReal<Scalar>>,
    /// The scratch buffer used for both FFT and IFFT
    correlate_fft_scratch: Vec<Sample>,
}

impl SpectrogramCorrelator {
    pub fn get_max_length_samples(&self) -> usize {
        (self.max_spectrogram_size - 1) * self.window_step + self.window_size
    }

    fn build_window_fft(window_size: usize) -> (Arc<dyn Fft<Scalar>>, Vec<Sample>) {
        let mut fft_planner = FftPlanner::new();
        let window_fft = fft_planner.plan_fft_forward(window_size);
        let mut window_fft_scratch = Vec::new();
        window_fft_scratch.resize(window_fft.get_inplace_scratch_len(), Sample::new(0.0, 0.0));

        (window_fft, window_fft_scratch)
    }
    fn build_window_function(window_size: usize) -> Array1<Scalar> {
        let mut window_function = Array1::zeros(window_size * 2);
        let n = window_size - 1;
        for i in 0..window_size {
            // TODO: This is a Hann window, change to other type more appropiate
            let val = (std::f64::consts::PI * (i as f64) / (n as f64)).sin() as Scalar;
            window_function[i * 2] = val * val;
            window_function[i * 2 + 1] = window_function[i * 2];
        }

        window_function
    }

    fn build_correlate_ffts(
        spectrogram_size: usize,
    ) -> (
        Arc<dyn RealToComplex<Scalar>>,
        Arc<dyn ComplexToReal<Scalar>>,
        Vec<Sample>,
    ) {
        let mut real_planner = RealFftPlanner::<Scalar>::new();

        // Double size to prevent circular-convolution messing up results
        let correlate_fft = real_planner.plan_fft_forward(spectrogram_size * 2);
        let correlate_ifft = real_planner.plan_fft_inverse(spectrogram_size * 2);
        let mut correlate_fft_scratch = Vec::new();
        correlate_fft_scratch.resize(correlate_fft.get_scratch_len(), Complex::new(0.0, 0.0));

        assert_eq!(
            correlate_fft_scratch.len(),
            correlate_ifft.get_scratch_len()
        );

        (correlate_fft, correlate_ifft, correlate_fft_scratch)
    }

    /// Build a spectrogram correlator internal buffers
    /// `window_size`: how many samples does each window include
    /// `window_step`: how many samples separate the start of each window, overlap is allowed
    /// `spectrogram_size`: total number of windows to include in the spectrogram
    pub fn new(window_size: usize, window_step: usize, spectrogram_size: usize) -> Self {
        let (window_fft, window_fft_scratch) = Self::build_window_fft(window_size);
        let window_function = Self::build_window_function(window_size);

        let (correlate_fft, correlate_ifft, correlate_fft_scratch) =
            Self::build_correlate_ffts(spectrogram_size);

        Self {
            window_size,
            window_step,
            correlate_fft,
            correlate_ifft,
            correlate_fft_scratch,
            window_fft,
            window_fft_scratch,
            window_function,
            max_spectrogram_size: spectrogram_size,
        }
    }

    fn apply_hann(&self, array: &mut Array1<Sample>) {
        let mut wbuffer_as_scalars = unsafe {
            // SAFETY: All operations are correct as long as Complex = {Scalar, Scalar} in memory
            ArrayViewMut1::from_shape_ptr(array.len() * 2, array.as_mut_ptr() as *mut Scalar)
        };

        // This will hopefully auto-vectorize / use BLAS
        wbuffer_as_scalars *= &self.window_function;
    }

    fn fft_window(&mut self, array: &mut Array1<Sample>) {
        self.window_fft
            .process_with_scratch(array.as_slice_mut().unwrap(), &mut self.window_fft_scratch);
    }

    fn build_spectrogram(
        &mut self,
        samples: &Array1<Sample>,
        num_windows: usize,
    ) -> Array2<Scalar> {
        let mut out = Array2::zeros((self.window_size, num_windows));

        let mut buffer_ptr = 0;
        for window_ptr in 0..num_windows {
            // We make a copy, to not disturb the source sample array
            let mut samples_window = samples
                .slice(s![buffer_ptr..buffer_ptr + self.window_size])
                .to_owned();

            self.apply_hann(&mut samples_window);
            self.fft_window(&mut samples_window);
            out.column_mut(window_ptr)
                .assign(&samples_window.mapv(|v| v.abs()));

            buffer_ptr += self.window_step;
        }

        out
    }

    fn build_ref_spectrogram(
        &self,
        num_windows: usize,
        t0: f64,
        samp_rate: u64,
        center_freq: f64,
        freqs: &Vec<FreqChange>,
    ) -> ReferenceSpectrogram {
        let end_offset_samples = self.window_step * (num_windows - 1) + self.window_size;
        let end_offset_t = end_offset_samples as f64 / samp_rate as f64;

        let freqs_interval = get_freqs_for_interval(freqs, t0, t0 + end_offset_t);

        let mut ref_spectrogram = ReferenceSpectrogram::new(
            self.window_size,
            self.window_step,
            num_windows,
            t0,
            center_freq,
            samp_rate as f64,
        );

        for freq in &freqs_interval {
            ref_spectrogram.add_freq(freq);
        }

        ref_spectrogram
    }

    fn correlate_line(
        &mut self,
        rx_line: &ArrayView1<Scalar>,
        ref_line: &Array1<Scalar>,
        buffers: &mut CorrelationBuffers,
    ) {
        let n = buffers.buff_rx.len() / 2;

        // Move the measured line into scratch_a and zero the rest
        buffers.buff_rx.slice_mut(s![..n]).assign(rx_line);
        buffers.buff_rx.slice_mut(s![n..]).fill(0.0);

        // Move the reference line into scratch_b and zero the rest
        buffers.buff_ref.slice_mut(s![..n]).assign(ref_line);
        buffers.buff_ref.slice_mut(s![n..]).fill(0.0);

        // FFT both for fast convolution
        self.correlate_fft
            .process_with_scratch(
                buffers.buff_rx.as_slice_mut().unwrap(),
                buffers.fft_rx.as_slice_mut().unwrap(),
                self.correlate_fft_scratch.as_mut_slice(),
            )
            .unwrap();

        self.correlate_fft
            .process_with_scratch(
                buffers.buff_ref.as_slice_mut().unwrap(),
                buffers.fft_ref.as_slice_mut().unwrap(),
                self.correlate_fft_scratch.as_mut_slice(),
            )
            .unwrap();

        // Multiply together (convolve in time domain)
        azip!((a in &mut buffers.fft_rx, &b in &buffers.fft_ref) *a *= b.conj());

        // Return to time domain by the IFFT. Note results of previous op are in fft_rx
        self.correlate_ifft
            .process_with_scratch(
                buffers.fft_rx.as_slice_mut().unwrap(),
                buffers.buff_rx.as_slice_mut().unwrap(),
                self.correlate_fft_scratch.as_mut_slice(),
            )
            .unwrap();

        azip!((a in &mut buffers.accum_corr, &b in &buffers.buff_rx) *a += b);

        let (max_idx, psr) = get_max_index_and_psr(&buffers.accum_corr);
        log::info!("Max index = {} PSR = {}", max_idx, psr);

        // PSR weighted histogram
        let entry = buffers.max_index_histogram.entry(max_idx).or_insert(0);
        *entry += (psr * 10000.0) as u64;

        buffers.accum_i += 1;
    }

    /// Correlate the spectrogram build from the given `samples`, assumed to start at `t0`, and to be sampled
    /// at a rate of `samp_rate` samples per second, against the
    /// reference frequencies `freqs`, returning the number of samples that `samples` has to be delayed
    /// (negative if it has to be advanced) to match the reference as good as possible
    pub fn correlate_against(
        &mut self,
        samples: &Array1<Sample>,
        t0: f64,
        samp_rate: u64,
        center_freq: f64,
        freqs: &Vec<FreqChange>,
    ) -> i64 {
        let num_windows = (samples.len() - self.window_size) / self.window_step + 1;
        assert!(num_windows > 1);

        let spectrogram = self.build_spectrogram(samples, num_windows);

        let mut ref_spectrogram =
            self.build_ref_spectrogram(num_windows, t0, samp_rate, center_freq, freqs);

        let mut buffers = CorrelationBuffers::new(num_windows);

        // Correlate lines with the most entries until a good result is achieved (good side-lobe ratio)
        while let Some((bin, line)) = ref_spectrogram.pull_biggest_line_ref() {
            log::info!("Correlating bin {}", bin);

            let rx_line = spectrogram.slice(s![bin, ..]);
            self.correlate_line(&rx_line, &line, &mut buffers);

            // TODO: Early exit condition?
            if buffers.accum_i > 50 {
                break;
            }
        }

        // Pick the most popular entry
        let max_entry = buffers
            .max_index_histogram
            .iter()
            .max_by_key(|(_, v)| *v)
            .unwrap()
            .0;
        log::info!("Max entry computed to be: {}", max_entry);

        // TODO: Check that this is correct!
        let max_entry = if *max_entry as i64 > self.max_spectrogram_size as i64 {
            // It's actually delayed
            *max_entry as i64 - self.max_spectrogram_size as i64 * 2
        } else {
            *max_entry as i64
        };

        max_entry * self.window_step as i64 + self.window_size as i64 / 2
    }
}

/// An individual "line" (single frequency bin over the duration of the spectrogram)
#[derive(Debug)]
struct ReferenceSpectrogramLine {
    data: Array1<Scalar>,
    num_entries: usize,
}

/// A complete reference spectrogram
struct ReferenceSpectrogram {
    lines: HashMap<usize, ReferenceSpectrogramLine>,
    window_size: usize,
    window_step: usize,
    num_windows: usize,
    start_epoch: f64,
    center_freq: f64,
    samp_rate: f64,
}

impl ReferenceSpectrogram {
    /// Create a new ReferenceSpectrogram from needed data:
    /// `window_size`: How many samples go into each window?
    /// `window_step`: How many samples separate the start of each window
    /// `num_windows`: Total number of windows in the spectrogram
    /// `start_epoch`: Epoch of first sample in the spectrogram
    /// `center_freq`: Frequency of the central bin, to map frequencies to bins
    /// `samp_rate`: What's the sampling rate used to relate frequency to bin?
    pub fn new(
        window_size: usize,
        window_step: usize,
        num_windows: usize,
        start_epoch: f64,
        center_freq: f64,
        samp_rate: f64,
    ) -> Self {
        Self {
            lines: HashMap::new(),
            window_size,
            window_step,
            samp_rate,
            num_windows,
            start_epoch,
            center_freq,
        }
    }

    fn get_line_ref(&mut self, bin: usize) -> &mut ReferenceSpectrogramLine {
        self.lines
            .entry(bin)
            .or_insert_with(|| ReferenceSpectrogramLine {
                num_entries: 0,
                data: Array1::zeros(self.num_windows),
            })
    }

    // Searches the line with the most entries, gets it and removes it
    // Returns the (bin, line) pair.
    fn pull_biggest_line_ref(&mut self) -> Option<(usize, Array1<Scalar>)> {
        let mut max_entries = 0;
        let mut max_entries_line: Option<usize> = None;

        for line in &self.lines {
            if line.1.num_entries > max_entries {
                max_entries = line.1.num_entries;
                max_entries_line = Some(*line.0);
            }
        }

        Some((
            max_entries_line?,
            self.lines.remove(&max_entries_line?)?.data,
        ))
    }

    fn add_entry_to_line(&mut self, bin: usize, start_samp: i64, end_samp: i64, fac: f32) {
        assert!(start_samp < end_samp);

        let min_window =
            (start_samp / self.window_step as i64).clamp(0, self.num_windows as i64 - 1);
        let max_window = (end_samp / self.window_step as i64).clamp(0, self.num_windows as i64 - 1);

        let line = self.get_line_ref(bin);

        // TODO: Rewrite this using a high order method from ndarray for speed
        for window in min_window..=max_window {
            // TODO: Edge effect on partial windows where the frequency starts / ends "in the middle"
            let window_fac = if window == min_window {
                1.0
            } else if window == max_window {
                1.0
            } else {
                1.0
            };

            let total_fac = fac * window_fac;

            let data = &mut line.data[window as usize];
            // Only set if no other data is here, this happens on two subsequent frequencies
            *data = if *data > 0.0 || total_fac == 0.0 {
                *data
            } else {
                total_fac
            };
        }

        line.num_entries += 1;
    }

    fn add_freq(&mut self, freq: &FreqOnTimes) {
        let Some(bins) = self.hz_to_bin(freq.freq - self.center_freq) else {
            return;
        };

        let start_rel_t = freq.start - self.start_epoch;
        let end_rel_t = freq.end - self.start_epoch;

        let start_samp = (start_rel_t * self.samp_rate) as i64;
        let end_samp = (end_rel_t * self.samp_rate) as i64;

        self.add_entry_to_line(bins.0.0, start_samp, end_samp, bins.0.1);
        self.add_entry_to_line(bins.1.0, start_samp, end_samp, bins.1.1);
    }

    // Given index of bin in spectrogram FFT, returns the center frequency of said bin
    fn bin_to_hz(&self, bin: usize) -> f64 {
        debug_assert!(bin < self.window_size);

        let binf = bin as f64;
        let winf = self.window_size as f64;
        let hz_per_bin = self.samp_rate / winf;

        if binf > winf / 2.0 {
            // Bin represents negative frequency, we map
            // bin N/2 -> -SampRate / 2 (technically not included)
            // bin N-1 -> -SampRate / N
            (binf - winf) * hz_per_bin
        } else {
            // Bin represents positive frequency, we map
            // bin 0 -> 0Hz
            // bin N/2 -> SampRate / 2
            binf * hz_per_bin
        }
    }

    // Given frequency, returns the two nearest bins and their linear weight factor
    // or None if out of bounds
    fn hz_to_bin(&self, f: f64) -> Option<((usize, Scalar), (usize, Scalar))> {
        let equivf = if f < 0.0 {
            // Negative frequency is located on the FFT as if it were over Nyquist
            self.samp_rate + f
        } else {
            f
        };

        debug_assert!(equivf >= 0.0);

        let winf = self.window_size as f64;
        let hz_per_bin = self.samp_rate / winf;
        let fract_bin = equivf / hz_per_bin;

        if fract_bin > winf - 1.0 {
            return None;
        }

        let upper = fract_bin.ceil();
        let upperfac = if upper == fract_bin {
            1.0
        } else {
            upper - fract_bin
        };
        let lower = fract_bin.floor();
        let lowerfac = if lower == fract_bin {
            0.0
        } else {
            fract_bin - lower
        };

        debug_assert!(upperfac + lowerfac >= 0.999);

        Some((
            (lower as usize, 1.0 - lowerfac as Scalar),
            (upper as usize, 1.0 - upperfac as Scalar),
        ))
    }
}

// Returns the index of the maximum value in data, and how big it's compared to
// the next 10 biggest values (their average)
fn get_max_index_and_psr(data: &Array1<Scalar>) -> (usize, Scalar) {
    let mut max = Vec::with_capacity(10);
    let mut max_idx = 0;

    for (i, &v) in data.iter().enumerate() {
        let pos = max
            .binary_search_by(|&x| v.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or_else(|e| e);
        if max.len() < 10 {
            max.insert(pos, v);
            if pos == 0 {
                max_idx = i;
            }
        } else if pos < 10 {
            max.insert(pos, v);
            max.pop();
            if pos == 0 {
                max_idx = i;
            }
        }
    }

    let avg: Scalar = max[1..].iter().sum::<Scalar>() / max[1..].len() as Scalar;
    (max_idx, max[0] / avg)
}
