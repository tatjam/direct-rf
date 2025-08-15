//! Simple classes from streaming (non random access!) of samples, so that we can work with
//! big files without hogging memory and having long load times.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use log::info;
use regex::Regex;
use chrono::{TimeZone, Utc, DateTime};
use rustfft::num_complex::Complex;
use wave_stream::open_wav::OpenWav;
use wave_stream::wave_reader::{OpenWavReader, StreamOpenWavReader, StreamWavReader};

pub type Scalar = f32;

// Allows streaming samples from a Wav file, without fully loading them in memory
struct StreamedBaseband {
    center_freq: f64,
    start_date: DateTime<Utc>,
    sample_rate: u32,
    wav: wave_stream::wave_reader::StreamWavReaderIterator<f32>,
}

impl StreamedBaseband {
    pub fn new(path: String) -> Result<Self, &'static str> {
        info!("Loading baseband from {}", path);
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

        let open_wav = wave_stream::read_wav_from_file_path(Path::new(&path)).unwrap();
        if open_wav.num_channels() != 2 {
            return Err("Baseband must contain I/Q data in stereo");
        }

        let sample_rate = open_wav.sample_rate();

        let iter = open_wav.get_stream_f32_reader().unwrap().into_iter();

        Ok(StreamedBaseband{
            center_freq: freq,
            start_date: date,
            sample_rate,
            wav: iter,
        })
    }

    pub fn get_next(self: &mut Self, seconds: f64) -> Vec<Complex<Scalar>> {
        let mut out = Vec::new();


        out
    }
}


// Allows streaming samples from a frequencies file, without fully loading them in memory
struct StreamedSamplesFreqs {
}

impl StreamedSamplesFreqs {

}