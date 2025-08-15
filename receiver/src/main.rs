use log::info;
use pico_args;
use regex::Regex;
use rustfft::{FftPlanner, num_complex::Complex};
use std::f64::consts::PI;
use crate::dsp::{DspSettings, Dsp};
use crate::stream::{StreamedBaseband, StreamedSamplesFreqs, Scalar};

mod stream;
mod dsp;

fn correlate_and_run(baseband: &mut StreamedBaseband, freqs: &mut StreamedSamplesFreqs, comp_samps: u64, run_samps: u64) {

}

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

    let correlate_time: f64 = pargs.opt_value_from_str(["-c", "--correlate"]).unwrap().unwrap_or(2.0);
    let correlate_samps = (correlate_time * baseband.get_sample_rate() as f64).ceil() as usize;

    let adjust_time: f64 = pargs.opt_value_from_str(["-a", "--adjust"]).unwrap().unwrap_or(0.25);
    let adjust_samps = (adjust_time * baseband.get_sample_rate() as f64).ceil() as usize;

    let run_time: f64 = pargs.opt_value_from_str(["-r", "--run"]).unwrap().unwrap_or(10.0);
    let run_samps = (run_time * baseband.get_sample_rate() as f64).ceil() as usize;

    let min_psr: f64 = pargs.opt_value_from_str(["-m", "--minpsr"]).unwrap().unwrap_or(20.0);

    let output_decimate: usize = pargs.opt_value_from_str(["-d", "--decimate"]).unwrap().unwrap_or(20);

    let dsp_settings = DspSettings {
        correlate_samps,
        adjust_samps,
        run_samps,
        output_decimate,
        min_psr: min_psr as Scalar,
    };

    let dsp = Dsp::new(baseband, freqs, dsp_settings);


}
