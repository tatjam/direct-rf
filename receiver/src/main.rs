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
        window_size: 1024,
        window_step: 512,
        output_decimate,
        min_psr: min_psr as Scalar,
    };

    let mut dsp = Dsp::new(baseband, freqs, dsp_settings);
    for i in 0..19 {
        dsp.build_spectrogram(50);
    }
    let (spectrogram, nwindows) = dsp.build_spectrogram(850);

    let file = File::create("dump.csv").unwrap();
    let mut writer = WriterBuilder::new().has_headers(false).from_writer(file);
    writer.serialize_array2(&spectrogram).unwrap();


}
