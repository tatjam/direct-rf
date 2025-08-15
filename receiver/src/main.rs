use log::info;
use pico_args;
use regex::Regex;
use rustfft::{FftPlanner, num_complex::Complex};
use std::f64::consts::PI;

mod stream;

fn main() {
    env_logger::init();
    let mut pargs = pico_args::Arguments::from_env();

    let baseband_path: String = pargs.free_from_str().unwrap();
    let baseband = load_baseband(baseband_path).unwrap();

    let freqs_path: String = pargs.opt_value_from_str(["-f", "--freqs"])
        .unwrap().unwrap_or(String::from("freqs.csv"));
    log::info!("Loading transmitted frequencies from {}", freqs_path);
    let freqs = load_freqs(freqs_path).unwrap();

}
