use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::ops::Add;
use crate::stream::{Sample, Scalar, StreamedBaseband, StreamedSamplesFreqs};
use log::info;
use rustfft::num_complex::ComplexFloat;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use ndarray::{Array1, Array2, s, ArrayViewMut1};
use chrono::{DateTime, TimeDelta, Utc};
use csv::WriterBuilder;
use ndarray_csv::Array2Writer;

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
    baseband: StreamedBaseband,
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

}


impl Dsp {
    pub fn new(
        baseband: StreamedBaseband,
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
            hann_window[i*2] = (std::f64::consts::PI * (i as f64) / (n as f64)).sin() as Scalar;
            hann_window[i*2+1] = hann_window[i];
        }

        let window_buffer = Array1::zeros(settings.window_size);

        let spectrogram = Spectrogram {
            start_sample: 0,
            start_sample_confidence: 0.0,
            data: Default::default(),
            sample0_epoch: 0.0,
        };

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
        }
    }

    pub fn first_run(&mut self) {
        self.baseband.seek_epoch(self.freqs.get_first_epoch() - 1.5);
        self.spectrogram.sample0_epoch = self.freqs.get_first_epoch() - 1.5;

        debug_assert_eq!(self.spectrogram_buffer.len(), 0);

        // First window has no "historic data"!
        let nsamples = self.settings.window_size
            + self.settings.window_step * (self.settings.spectrogram_size_search - 1);

        // Collect enough samples to run the correlation
        self.spectrogram_buffer = Vec::new();
        self.spectrogram_buffer.resize(nsamples, Sample::new(0.0, 0.0));

        let nread = self.baseband.read_into(self.spectrogram_buffer.as_mut_slice());

        debug_assert_eq!(nread, nsamples);

        self.build_spectrogram();
        self.dump_spectrogram();

        let delay = self.correlate_spectrogram();

        self.first_run = false;

    }

    fn build_spectrogram(&mut self) {
        // Note that the start sample is the first sample of the first window, and hence if historic
        // data was used, it's already accounted for here.
        self.spectrogram.start_sample += self.spectrogram.data.ncols() * self.settings.window_step;

        let nwindows = if self.overlap_data.is_empty() {
            1 + (self.spectrogram_buffer.len() - self.settings.window_size) / self.settings.window_step
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
            self.spectrogram.data.column_mut(window_ptr).assign(
                &self.window_buffer.mapv(|v| v.abs())
            );

            buffer_ptr += nsamples;
            window_ptr += 1;
        }

    }

    fn dump_spectrogram(&self) {
        let file = File::create("dump.csv").unwrap();
        let mut writer = WriterBuilder::new().has_headers(false).from_writer(file);
        writer.serialize_array2(&self.spectrogram.data).unwrap();
    }

    // Reads a full window from buffer into window_buffer, using overlap_data
    // if any is there, and updating overlap_data. Returns how many samples we consumed from the buffer.
    fn read_full_window(&mut self, ptr: usize) -> usize {
        let as_slice = self.window_buffer.as_slice_mut().unwrap();
        let data = &self.spectrogram_buffer[ptr..];

        let nsamp = if self.overlap_data.is_empty() {
            self.overlap_data = Array1::zeros(self.settings.window_size - self.settings.window_step);
            // Full copy
            self.settings.window_size
        } else {
            debug_assert_eq!(self.overlap_data.len(), self.settings.window_size - self.settings.window_step);
            // Partial copy
            self.settings.window_step
        };

        if data.len() < nsamp {
            // We only operate on full windows, skip
            return 0;
        }

        as_slice[self.settings.window_size-nsamp..].copy_from_slice(&data[0..nsamp]);

        // The rest is loaded from the overlap data
        if nsamp != self.settings.window_size {
            as_slice[0..self.settings.window_size - nsamp].copy_from_slice(
                &self.overlap_data.as_slice().unwrap());
        }

        // The next overlap data is copied over from the new slice
        self.overlap_data.as_slice_mut().unwrap().copy_from_slice(
            &as_slice[self.settings.window_step..]
        );

        nsamp
    }

    // Returns how many samples the spectrogram has to be delayed to match the reference
    fn correlate_spectrogram(&mut self) -> usize  {
        let start_offset = self.spectrogram.start_sample as f64 / self.baseband.get_sample_rate() as f64;
        let start_t = self.spectrogram.sample0_epoch + start_offset;
        let end_offset = self.spectrogram.data.ncols() as f64 / self.baseband.get_sample_rate() as f64;
        let freqs = self.freqs.get_frequencies_for_interval(start_t, start_t + end_offset);

        for intfreq in freqs.keys() {
            let relfreq = *intfreq as f64 - self.baseband.get_center_freq();
        }

        0
    }

    fn apply_hann(&mut self) {
        let mut wbuffer_as_scalars = unsafe {
            // SAFETY: All operations are correct as long as Complex = {Scalar, Scalar} in memory
            ArrayViewMut1::from_shape_ptr(
                self.window_buffer.len() * 2,
                self.window_buffer.as_mut_ptr() as *mut Scalar
            )
        };

        // This will hopefully auto-vectorize / use BLAS
        wbuffer_as_scalars *= &self.hann_window;
    }

    fn fft_window(&mut self) {
        self.window_fft.process_with_scratch(
            self.window_buffer.as_slice_mut().unwrap(),
            &mut self.window_fft_scratch);
    }


}
