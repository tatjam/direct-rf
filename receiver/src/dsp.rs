use crate::correlator::SpectrogramCorrelator;
use crate::stream::{Scalar, StreamedSamplesFreqs};
use anyhow::Result;
use ndarray::Array1;
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
        let start = self.freqs.get_first_epoch() - 1.0;
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

        let nread = self.baseband.get_samples(buffer.as_slice_mut().unwrap())?;

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

        // Seek back to the start, and offset the result of correlation
        self.baseband.seek_to_timestamp((start * 1000.0) as u64)?;
        self.baseband
            .seek(std::io::SeekFrom::Current(delay_in_samples))?;

        Ok(())
    }
}
