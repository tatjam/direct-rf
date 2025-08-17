use std::fs::File;
use csv::WriterBuilder;
use log::info;
use pico_args;
use crate::dsp::{DspSettings, Dsp};
use crate::stream::{StreamedBaseband, StreamedSamplesFreqs, Scalar};
use ndarray_csv::Array2Writer;

mod stream;
mod dsp;

fn main() {
    env_logger::init();
    let mut pargs = pico_args::Arguments::from_env();

    let baseband_path: String = pargs.free_from_str().unwrap();
    info!("Loading baseband for {}", baseband_path);
    let baseband = StreamedBaseband::new(baseband_path);

    let freqs_path: String = pargs.opt_value_from_str(["-f", "--freqs"])
        .unwrap().unwrap_or(String::from("freqs.csv"));
    info!("Loading transmitted frequencies from {}", freqs_path);
    let freqs = StreamedSamplesFreqs::new(freqs_path,
                                          baseband.get_center_freq(),
                                          baseband.get_sample_rate());

    let min_psr: f64 = pargs.opt_value_from_str(["-m", "--minpsr"]).unwrap().unwrap_or(20.0);

    let output_decimate: usize = pargs.opt_value_from_str(["-d", "--decimate"]).unwrap().unwrap_or(20);

    let dsp_settings = DspSettings {
        window_size: 512,
        window_step: 128,
        spectrogram_size_search: 75000, // 4s at 2.4Msps with these settings
        spectrogram_size_adjust: 5000, // A bit over 0.25s with these settings
        spectrogram_adjust_slide: 2_400, // 1ms on each direction at 2.4Msps
        output_decimate,
        min_psr: min_psr as Scalar,
    };

    let mut dsp = Dsp::new(baseband, freqs, dsp_settings);
    dsp.first_run();
}
