use std::fs::File;

use crate::dsp::{Dsp, DspSettings};
use crate::stream::{Scalar, StreamedSamplesFreqs};
use log::info;
use sdriq::Source;

mod dsp;
mod stream;

fn main() {
    env_logger::init();
    let mut pargs = pico_args::Arguments::from_env();

    let baseband_path: String = pargs.free_from_str().unwrap();
    info!("Loading baseband for {}", baseband_path);
    let baseband_file = File::open(baseband_path).unwrap();
    let baseband = Source::new(baseband_file).unwrap();

    let freqs_path: String = pargs
        .opt_value_from_str(["-f", "--freqs"])
        .unwrap()
        .unwrap_or(String::from("freqs.csv"));
    info!("Loading transmitted frequencies from {}", freqs_path);
    let freqs = StreamedSamplesFreqs::new(
        freqs_path,
        baseband.get_header().center_freq as f64,
        baseband.get_header().samp_rate,
    );

    let min_psr: f64 = pargs
        .opt_value_from_str(["-m", "--minpsr"])
        .unwrap()
        .unwrap_or(20.0);

    let output_decimate: usize = pargs
        .opt_value_from_str(["-d", "--decimate"])
        .unwrap()
        .unwrap_or(20);

    let dsp_settings = DspSettings {
        window_size: 512,
        window_step: 512,
        spectrogram_size_search: 20000,
        spectrogram_size_adjust: 5000,
        spectrogram_adjust_slide: 2_400,
        output_decimate,
        min_psr: min_psr as Scalar,
    };

    let mut dsp = Dsp::new(baseband, freqs.unwrap(), dsp_settings);
    dsp.first_run().unwrap();
}
