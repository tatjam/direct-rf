use crate::correlator::SpectrogramCorrelator;
use crate::stream::{Sample, Scalar, StreamedSamplesFreqs};
use anyhow::Result;
use ndarray::{Array1, azip};
use std::fs::File;

pub struct DspSettings {
    // Window size in samples.
    pub window_size: usize,
    // How much each window is offset from the previous one, in samples
    pub window_step: usize,

    // How many windows to use during the search phase
    pub spectrogram_size_search: usize,

    // How many windows to use during the adjust phase
    pub spectrogram_size_adjust: usize,

    // Decimation for the output "mixed" signal
    pub output_decimate: usize,
    // Minimum PSR (peak-to-sidelobe ratio) for a correlation to be considered successful
    pub min_psr: Scalar,
}

pub struct Dsp {
    baseband: sdriq::Source<File>,
    freqs: StreamedSamplesFreqs,
    settings: DspSettings,
    correlator: SpectrogramCorrelator,

    first_run: bool,
}

impl Dsp {
    pub fn new(
        baseband: sdriq::Source<File>,
        freqs: StreamedSamplesFreqs,
        settings: DspSettings,
    ) -> Self {
        let correlator = SpectrogramCorrelator::new(
            settings.window_size,
            settings.window_step,
            settings.spectrogram_size_search,
        );

        Self {
            baseband,
            freqs,
            settings,
            correlator,
            first_run: true,
        }
    }

    pub fn first_run(&mut self) -> Result<()> {
        let start0 = self.freqs.get_first_epoch();
        // Give a bit of margin to not lose information
        let start = start0 - 1.0;
        log::info!(
            "Baseband starts at {}, freqs start at {}",
            self.baseband.get_header().start_timestamp / 1000,
            self.freqs.get_first_epoch()
        );

        log::info!("Seeking to {}", start);
        self.baseband.seek_to_timestamp((start * 1000.0) as u64)?;

        let nsamples = self.correlator.get_max_length_samples();

        // Collect enough samples to run the correlation
        let mut buffer = Array1::zeros(nsamples);

        let nread = self
            .baseband
            .get_samples_norm(buffer.as_slice_mut().unwrap())?;

        log::info!("Able to read {} samples out of {} optimal", nread, nsamples);

        let samp_rate = self.baseband.get_header().samp_rate as u64;
        let center_freq = self.baseband.get_header().center_freq as f64;

        let delay_in_samples = self.correlator.correlate_against(
            &buffer,
            start,
            samp_rate,
            center_freq,
            self.freqs.get_freqs(),
        );

        let delay_in_time = delay_in_samples as f64 / self.baseband.get_header().samp_rate as f64;

        log::info!(
            "Delay  in samples = {}, in time = {}ms",
            delay_in_samples,
            delay_in_time * 1000.0,
        );

        // Seek back to the start, and offset the result of correlation + the extra offset we applied
        // (Note delay is with respect to reference also starting at t0, so expected to be small unless clocks
        // are tremendously off!)
        self.baseband.seek_to_timestamp((start0 * 1000.0) as u64)?;
        self.baseband
            .seek(std::io::SeekFrom::Current(delay_in_samples))?;

        // Now baseband is more or less exactly in line with reference samples

        self.first_run = false;

        Ok(())
    }

    pub fn run(&mut self, samples: usize) -> Result<Array1<Sample>> {
        if self.first_run {
            self.first_run()?;
        }

        let mut out = Array1::zeros(samples / self.settings.output_decimate);

        // Get samples from both rx baseband and reference (with an offset) and mix them together
        let mut rx_samples: Array1<Sample> = Array1::zeros(samples);
        self.baseband
            .get_samples_norm(rx_samples.as_slice_mut().unwrap())?;

        let ref_samples = self.freqs.get_next(samples, 10000.0).0;

        // Modify rx_samples so it contains the mixed result
        azip!((a in &mut rx_samples, &b in &ref_samples) *a *= b.conj());

        // TODO: Perform some kind of interpolation?
        for i in (0..samples).step_by(self.settings.output_decimate) {
            out[i / self.settings.output_decimate] = rx_samples[i];
            //out[i / self.settings.output_decimate] = ref_samples[i];
        }

        Ok(out)
    }
}
