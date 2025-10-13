use crate::stream::{FreqOnTimes, Sample, Scalar, StreamedSamplesFreqs};
use anyhow::{Result, anyhow};
use csv::WriterBuilder;
use ndarray::{Array1, Array2, ArrayViewMut1, azip, s};
use ndarray_npy::write_npy;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use rustfft::num_complex::ComplexFloat;
use rustfft::{Fft, FftPlanner};
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;

pub struct DspSettings {
    // Window size in samples.
    pub window_size: usize,
    // How much each window is offset from the previous one, in samples
    pub window_step: usize,

    // How many windows to use during the search phase
    pub spectrogram_size_search: usize,

    // How many windows to use during the adjust phase
    pub spectrogram_size_adjust: usize,

    // How many samples to slide the spectrogram around its supposed start time to search for
    // correlation, during the adjusting phases. (Maximum slide forward, and backwards!)
    // (During search mode, FFT correlation is done instead)
    pub spectrogram_adjust_slide: usize,

    // Decimation for the output "mixed" signal
    pub output_decimate: usize,
    // Minimum PSR (peak-to-sidelobe ratio) for a correlation to be considered successful
    pub min_psr: Scalar,
}

struct Spectrogram {
    data: Array2<Scalar>,
    start_sample: usize,
    start_sample_confidence: Scalar,
    // Set during first run, afterward corrections use other mechanism
    sample0_epoch: f64,
}

struct ReferenceSpectrogramLine {
    data: Array1<Scalar>,
    num_entries: usize,
}

struct ReferenceSpectrogram {
    lines: HashMap<usize, ReferenceSpectrogramLine>,
    window_size: f64,
    samp_rate: f64,
    cols: usize,
}

impl ReferenceSpectrogram {
    pub fn new(window_size: f64, samp_rate: f64, cols: usize) -> Self {
        Self {
            lines: HashMap::new(),
            window_size,
            samp_rate,
            cols,
        }
    }

    fn get_line_ref(&mut self, bin: usize) -> &mut ReferenceSpectrogramLine {
        self.lines
            .entry(bin)
            .or_insert_with(|| ReferenceSpectrogramLine {
                num_entries: 0,
                data: Array1::zeros(self.cols),
            })
    }

    // Searches the line with the most entries, gets it and removes it
    // Returns the (bin, line) pair.
    fn pull_biggest_line_ref(&mut self) -> Option<(usize, &mut Array1<Scalar>)> {
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
            &mut self.lines.get_mut(&max_entries_line?)?.data,
        ))
    }

    fn add_entry_to_line(&mut self, bin: usize, min_index: usize, max_index: usize, fac: f32) {
        let line = self.get_line_ref(bin);
        line.data
            .slice_mut(s![min_index..max_index])
            .mapv_inplace(|v| v + fac);
        line.num_entries += 1;
    }

    fn add_freq(&mut self, freq: &FreqOnTimes) {
        let Some(bins) = self.hz_to_bin(freq.freq) else {
            return;
        };

        let min_index = 0;
        let max_index = 50;

        self.add_entry_to_line(bins.0.0, min_index, max_index, bins.0.1);
        self.add_entry_to_line(bins.1.0, min_index, max_index, bins.1.1);
    }

    // Given index of bin in spectrogram FFT, returns the center frequency of said bin
    fn bin_to_hz(&self, bin: usize) -> f64 {
        let binf = bin as f64;
        debug_assert!(binf < self.window_size);
        let hz_per_bin = self.samp_rate / self.window_size;

        if binf > self.window_size / 2.0 {
            // Bin represents negative frequency, we map
            // bin N/2 -> -SampRate / 2 (technically not included)
            // bin N-1 -> -SampRate / N
            (binf - self.window_size) * hz_per_bin
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

        let hz_per_bin = self.samp_rate / self.window_size;
        let fract_bin = equivf / hz_per_bin;

        if equivf >= self.window_size {
            return None;
        }

        let upper = fract_bin.ceil();
        let upperfac = (upper - fract_bin) / hz_per_bin;
        let lower = fract_bin.floor();
        let lowerfac = (fract_bin - lower) / hz_per_bin;

        debug_assert!(upperfac + lowerfac >= 0.999);

        Some((
            (lower as usize, lowerfac as Scalar),
            (upper as usize, upperfac as Scalar),
        ))
    }
}

// The algorithm is as follows:
// First run: find the signal time alignment
//  - Assume the RTL-SDR start date has a uncertainty of 1.5s, so consider the
//    start time of the baseband to be the indicated date -1.5s
//  - Seek until the expected start of transmission
//  - Read spectrogram_size_search windows from baseband
//  - (Store a copy for later retrieval once aligned)?
//  - Correlate the resulting spectrogram against the expected one
//  - (Delay previously stored baseband by the needed samples)?
//  - Any further read from baseband is delayed by this number of samples
// Continuous adjustment:
//  - Read from baseband to perform the despreading
//  - Also build a spectrogram continuously with these samples
//  - Everytime the spectrogram is fully built, perform a search for
//    correlation maximum by sliding slightly (not using FFT, just manual convolution)
//  - Apply the correction afterwards
pub struct Dsp {
    baseband: sdriq::Source<File>,
    freqs: StreamedSamplesFreqs,
    settings: DspSettings,

    window_fft: Arc<dyn Fft<Scalar>>,
    window_fft_scratch: Vec<Sample>,
    // Each value is duplicated to allow fast SSE operation
    hann_window: Array1<Scalar>,
    first_run: bool,

    overlap_data: Array1<Sample>,
    window_buffer: Array1<Sample>,

    spectrogram: Spectrogram,
    spectrogram_buffer: Vec<Sample>,
    correlate_fft: Arc<dyn RealToComplex<Scalar>>,
    correlate_ifft: Arc<dyn ComplexToReal<Scalar>>,
    correlate_fft_scratch: Vec<Sample>,
}

impl Dsp {
    pub fn new(
        baseband: sdriq::Source<File>,
        freqs: StreamedSamplesFreqs,
        settings: DspSettings,
    ) -> Self {
        let mut fft_planner = FftPlanner::new();

        let window_fft = fft_planner.plan_fft_forward(settings.window_size);
        let mut window_fft_scratch = Vec::new();
        window_fft_scratch.resize(window_fft.get_inplace_scratch_len(), Sample::new(0.0, 0.0));

        let mut hann_window = Array1::zeros(settings.window_size * 2);
        let n = settings.window_size - 1;
        for i in 0..settings.window_size {
            let val = (std::f64::consts::PI * (i as f64) / (n as f64)).sin() as Scalar;
            hann_window[i * 2] = val * val;
            hann_window[i * 2 + 1] = hann_window[i * 2];
        }

        let window_buffer = Array1::zeros(settings.window_size);

        let spectrogram = Spectrogram {
            start_sample: 0,
            start_sample_confidence: 0.0,
            data: Default::default(),
            sample0_epoch: 0.0,
        };

        let mut real_planner = RealFftPlanner::<Scalar>::new();
        let correlate_fft = real_planner.plan_fft_forward(settings.spectrogram_size_search);
        let correlate_ifft = real_planner.plan_fft_inverse(settings.spectrogram_size_search);
        let mut correlate_fft_scratch = Vec::new();
        correlate_fft_scratch.resize(correlate_fft.get_scratch_len(), Complex::new(0.0, 0.0));

        assert_eq!(
            correlate_fft_scratch.len(),
            correlate_ifft.get_scratch_len()
        );

        Self {
            baseband,
            freqs,
            settings,
            first_run: true,
            window_fft,
            window_fft_scratch,
            hann_window,
            overlap_data: Default::default(),
            window_buffer,
            spectrogram,
            spectrogram_buffer: Default::default(),
            correlate_fft,
            correlate_ifft,
            correlate_fft_scratch,
        }
    }

    pub fn first_run(&mut self) -> Result<()> {
        let start = self.freqs.get_first_epoch() - 1.5;
        self.baseband.seek_to_timestamp((start * 1000.0) as u64)?;
        self.spectrogram.sample0_epoch = start;

        debug_assert_eq!(self.spectrogram_buffer.len(), 0);

        // First window has no "historic data"!
        let nsamples = self.settings.window_size
            + self.settings.window_step * (self.settings.spectrogram_size_search - 1);

        // Collect enough samples to run the correlation
        self.spectrogram_buffer = Vec::new();
        self.spectrogram_buffer
            .resize(nsamples, Sample::new(0.0, 0.0));

        let nread = self
            .baseband
            .get_samples(self.spectrogram_buffer.as_mut_slice())?;

        //debug_assert_eq!(nread, nsamples);

        self.build_spectrogram();
        self.dump_spectrogram()?;

        let _delay = self.correlate_spectrogram();

        self.first_run = false;

        Ok(())
    }

    fn build_spectrogram(&mut self) {
        // Note that the start sample is the first sample of the first window, and hence if historic
        // data was used, it's already accounted for here.
        self.spectrogram.start_sample += self.spectrogram.data.ncols() * self.settings.window_step;

        let nwindows = if self.overlap_data.is_empty() {
            1 + (self.spectrogram_buffer.len() - self.settings.window_size)
                / self.settings.window_step
        } else {
            self.spectrogram_buffer.len() / self.settings.window_step
        };

        self.spectrogram.data = Array2::zeros((self.settings.window_size, nwindows));

        let mut buffer_ptr = 0;
        let mut window_ptr = 0;
        loop {
            let nsamples = self.read_full_window(buffer_ptr);
            if nsamples == 0 {
                break;
            }

            self.apply_hann();
            self.fft_window();
            // Hopefully this will be done efficiently?
            self.spectrogram
                .data
                .column_mut(window_ptr)
                .assign(&self.window_buffer.mapv(|v| v.abs()));

            buffer_ptr += nsamples;
            window_ptr += 1;
        }
    }

    fn dump_spectrogram(&self) -> Result<()> {
        write_npy("dump.npy", &self.spectrogram.data)?;
        Ok(())
    }

    // Reads a full window from buffer into window_buffer, using overlap_data
    // if any is there, and updating overlap_data. Returns how many samples we consumed from the buffer.
    fn read_full_window(&mut self, ptr: usize) -> usize {
        let as_slice = self.window_buffer.as_slice_mut().unwrap();
        let data = &self.spectrogram_buffer[ptr..];

        let nsamp = if self.overlap_data.is_empty() {
            self.overlap_data =
                Array1::zeros(self.settings.window_size - self.settings.window_step);
            // Full copy
            self.settings.window_size
        } else {
            debug_assert_eq!(
                self.overlap_data.len(),
                self.settings.window_size - self.settings.window_step
            );
            // Partial copy
            self.settings.window_step
        };

        if data.len() < nsamp {
            // We only operate on full windows, skip
            return 0;
        }

        as_slice[self.settings.window_size - nsamp..].copy_from_slice(&data[0..nsamp]);

        // The rest is loaded from the overlap data
        if nsamp != self.settings.window_size {
            as_slice[0..self.settings.window_size - nsamp]
                .copy_from_slice(self.overlap_data.as_slice().unwrap());
        }

        // The next overlap data is copied over from the new slice
        self.overlap_data
            .as_slice_mut()
            .unwrap()
            .copy_from_slice(&as_slice[self.settings.window_step..]);

        nsamp
    }

    // Returns how many samples the spectrogram has to be delayed to match the reference
    fn correlate_spectrogram(&mut self) -> usize {
        let start_offset =
            self.spectrogram.start_sample as f64 / self.baseband.get_header().samp_rate as f64;
        let start_t = self.spectrogram.sample0_epoch + start_offset;
        let end_offset =
            self.spectrogram.data.ncols() as f64 / self.baseband.get_header().samp_rate as f64;
        let freqs = self
            .freqs
            .get_frequencies_for_interval(start_t, start_t + end_offset);

        let mut ref_spectrogram = ReferenceSpectrogram::new(
            self.settings.window_size as f64,
            self.baseband.get_header().samp_rate as f64,
            self.settings.spectrogram_size_search,
        );

        for freq in &freqs {
            ref_spectrogram.add_freq(freq);
        }

        let n = self.settings.spectrogram_size_search;
        let mut accum_corr: Array1<Scalar> = Array1::zeros(n);

        // Scratch buffers to perform the FFTs in

        // TODO: Everything here is terribly inneficient as a lot of copies
        // are done. We could be smarter and perform the FFT in place!
        let mut scratch_a: Array1<Scalar> = Array1::zeros(n * 2);
        let mut scratch_b: Array1<Scalar> = Array1::zeros(n * 2);
        let mut fft_a: Array1<Sample> = Array1::zeros(n + 1);
        let mut fft_b: Array1<Sample> = Array1::zeros(n + 1);

        // Correlate lines with the most entries until a good result is achieved
        while let Some((bin, line)) = ref_spectrogram.pull_biggest_line_ref() {
            // Move the measured line into scratch_a and zero the rest
            scratch_a
                .slice_mut(s![..n])
                // TODO: Check that flatten is a no-op in this case
                .assign(&self.spectrogram.data.slice(s![bin, ..]).flatten());
            scratch_a.slice_mut(s![n..]).fill(0.0);
            // Move the reference line into scratch_b and zero the rest
            scratch_b.slice_mut(s![..n]).assign(line);
            scratch_b.slice_mut(s![n..]).fill(0.0);

            // FFT both for fast convolution
            self.correlate_fft
                .process_with_scratch(
                    scratch_a.as_slice_mut().unwrap(),
                    fft_a.as_slice_mut().unwrap(),
                    self.correlate_fft_scratch.as_mut_slice(),
                )
                .unwrap();

            self.correlate_fft
                .process_with_scratch(
                    scratch_b.as_slice_mut().unwrap(),
                    fft_b.as_slice_mut().unwrap(),
                    self.correlate_fft_scratch.as_mut_slice(),
                )
                .unwrap();

            // Multiply together (convolve in time domain)
            azip!((a in &mut fft_a, &b in &fft_b) *a *= b);

            // Return to time domain
        }

        0
    }

    fn apply_hann(&mut self) {
        let mut wbuffer_as_scalars = unsafe {
            // SAFETY: All operations are correct as long as Complex = {Scalar, Scalar} in memory
            //
            ArrayViewMut1::from_shape_ptr(
                self.window_buffer.len() * 2,
                self.window_buffer.as_mut_ptr() as *mut Scalar,
            )
        };

        // This will hopefully auto-vectorize / use BLAS
        wbuffer_as_scalars *= &self.hann_window;
    }

    fn fft_window(&mut self) {
        self.window_fft.process_with_scratch(
            self.window_buffer.as_slice_mut().unwrap(),
            &mut self.window_fft_scratch,
        );
    }
}
