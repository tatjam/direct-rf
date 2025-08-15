use log::info;
use pico_args;
use regex::Regex;
use rustfft::{FftPlanner, num_complex::Complex};
use std::f64::consts::PI;
use crate::stream::{StreamedBaseband, StreamedSamplesFreqs};

mod stream;

fn main() {
    env_logger::init();
    let mut pargs = pico_args::Arguments::from_env();

    let baseband_path: String = pargs.free_from_str().unwrap();
    info!("Loading baseband for {}", baseband_path);
    let mut baseband = StreamedBaseband::new(baseband_path);

    let freqs_path: String = pargs.opt_value_from_str(["-f", "--freqs"])
        .unwrap().unwrap_or(String::from("freqs.csv"));
    info!("Loading transmitted frequencies from {}", freqs_path);
    let mut freqs = StreamedSamplesFreqs::new(freqs_path,
                                          baseband.get_center_freq(),
                                          baseband.get_sample_rate());

}
