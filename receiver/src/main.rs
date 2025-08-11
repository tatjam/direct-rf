use chrono::{Date, TimeZone};
use log::info;
use pico_args;
use regex::Regex;
use wavers::{Wav, read};
use wavers::chunks::fmt::CbSize::Base;
use rustfft::{FftPlanner, num_complex::Complex};

type Scalar = f32;

struct Baseband {
    samples: Vec<Complex<Scalar>>,
    sample_rate: i32,
    center_freq: Scalar,
    start_date: chrono::DateTime<chrono::Utc>,
}

struct Frequencies {
    t: f64,
    freq: f64,
}

fn load_freqs(path: String) -> Result<Vec<Frequencies>, &'static str> {
    Err("Guamedo")
}

fn load_baseband(path: String) -> Result<Baseband, &'static str> {
    log::info!("Loading baseband from {}", path);
    // TODO: For now we assume time is in local timezone, but I've made a SDR++ feature request that
    // TODO: would allow timezones to be specified in the filename (or simply the filenames being UTC)
    let date_regex = Regex::new(r".*/baseband_(\d+)Hz_(\d+)-(\d+)-(\d+)_(\d+)-(\d+)-(\d+).*\.wav").expect("Good Regex");
    let (freq, date) = match date_regex.captures(path.as_str()) {
        None => return Err("Unable to parse baseband filename, use $t_$f_$h-$m-$s_$d-$M-$y"),
        Some(captures) => {
            let freq = captures.get(1).unwrap().as_str().parse().unwrap();
            let hour = captures.get(2).unwrap().as_str().parse().unwrap();
            let min = captures.get(3).unwrap().as_str().parse().unwrap();
            let sec = captures.get(4).unwrap().as_str().parse().unwrap();
            let day = captures.get(5).unwrap().as_str().parse().unwrap();
            let month = captures.get(6).unwrap().as_str().parse().unwrap();
            let year = captures.get(7).unwrap().as_str().parse().unwrap();

            // TODO: Add timezone detection / UTC once SDR++ supports it
            let date = chrono::Local.with_ymd_and_hms(year, month, day, hour, min, sec)
                .unwrap().to_utc();

            (freq, date)
        },
    };

    info!("Understood file as starting in {}", date);
    info!("Understood file as centered in frequency {}Hz", freq);


    let mut baseband_wav: Wav<f32> = Wav::from_path(path).unwrap();
    if baseband_wav.n_channels() != 2 {
        return Err("Baseband must contain I/Q data in stereo");
    }

    let interleaved_samples: &[Scalar] = &baseband_wav.read().unwrap();
    assert_eq!(interleaved_samples.len() % 2, 0);

    let mut samples = Vec::new();
    samples.reserve(interleaved_samples.len() / 2);
    for samps in interleaved_samples.chunks(2) {
        let complex = Complex {
            re: samps[0],
            im: samps[1],
        };
        samples.push(complex);
    }

    log::info!("Loaded {} I/Q samples", interleaved_samples.len() / 2);

    Ok(Baseband {
        samples,
        sample_rate: baseband_wav.sample_rate(),
        center_freq: freq,
        start_date: date,
    })
}

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
