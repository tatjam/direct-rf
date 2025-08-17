use crate::stream::{Sample, Scalar, StreamedBaseband, StreamedSamplesFreqs};
use log::info;
use rustfft::num_complex::ComplexFloat;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use ndarray::{Array1, Array2, s, ArrayViewMut1};

pub struct DspSettings {
    // Window size in samples.
    pub window_size: usize,
    // How much each window is offset from the previous one, in samples
    pub window_step: usize,

    // Decimation for the output "mixed" signal
    pub output_decimate: usize,
    // Minimum PSR (peak-to-sidelobe ratio) for a correlation to be considered successful
    pub min_psr: Scalar,
}

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

        // Example with window size = 5, step size = 2
        // Win1:    XXXXX....
        // Win2:    ..XXXXX..
        // Win3:    ....XXXXX
        // Ovrl12:  ..XXX....
        // New12:   .....XX..
        // Ovrl23:  ....XXX..
        // New23:   .......XX
        let overlap_data = Array1::zeros(settings.window_size - settings.window_step);
        let window_buffer = Array1::zeros(settings.window_size);

        Self {
            baseband,
            freqs,
            settings,
            first_run: true,
            window_fft,
            window_fft_scratch,
            hann_window,
            overlap_data,
            window_buffer,
        }
    }

    // Fetches new data from baseband, concatenates it to old data, builds a new window buffer
    // and updates the old data vector. (Doesn't perform the fft!)
    // Returns the number of samples that were truly read from baseband
    fn update_window_buffer(&mut self, first: bool) -> usize {
        let ndata = if first {
            // All data is new data
            self.baseband.read_into(self.window_buffer.slice_mut(s![0..]))
        } else {
            // Old data is placed on the left
            self.window_buffer.slice_mut(s![0..self.overlap_data.len()]).assign(&self.overlap_data);
            // New data is placed on the right
            self.baseband.read_into(self.window_buffer.slice_mut(s![self.overlap_data.len()..]))
        };

        // Zero-out data that was not read (most of the time this does nothing)
        if !first {
            self.window_buffer.slice_mut(s![self.overlap_data.len() + ndata..]).fill(Sample::new(0.0, 0.0));
        }

        // Update old data with the data on the right of the buffer
        self.overlap_data.view_mut().assign(&self.window_buffer.slice(s![self.settings.window_step..]));

        ndata
    }

    fn apply_window(&mut self) {
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

    // Builds a window proper, i.e. performs window update, windowing and fft
    // Returns the number of real samples read from baseband
    fn build_window(&mut self, first: bool) -> usize {
        let ndata = self.update_window_buffer(first);
        self.apply_window();
        self.fft_window();
        ndata
    }

    // Each index contains all frequency bins for the time given by that window, if no new
    // data is available, zero padding will take place. The number of windows with useful data
    // is returned alongside the spectrogram
    pub fn build_spectrogram(&mut self, num_windows: usize) -> (Array2<Scalar>, usize) {
        let mut out = Array2::zeros((num_windows, self.settings.window_size));
        let mut nwindows = 0;

        loop {
            if nwindows == num_windows {
                break;
            }

            let ndata = self.build_window(self.first_run);
            self.first_run = false;
            if ndata == 0 {
                break;
            }

            let mut target = out.slice_mut(s![nwindows, ..]);
            target.assign(&self.window_buffer.map(|c| c.abs() ));

            nwindows += 1;
        }

        (out, nwindows)
    }
}
