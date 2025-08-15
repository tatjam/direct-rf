//! Simple classes from streaming (non random access!) of samples, so that we can work with
//! big files without hogging memory and having long load times.

use std::fs::File;
use std::hint::unreachable_unchecked;
use std::io::{BufRead, BufReader};
use std::path::Path;
use log::info;
use regex::Regex;
use chrono::{TimeZone, Utc, DateTime};
use rustfft::num_complex::Complex;
use wave_stream::open_wav::OpenWav;
use wave_stream::wave_reader::{OpenWavReader, StreamOpenWavReader, StreamWavReader};

pub type Scalar = f32;

// Allows streaming samples from a Wav file, without fully loading them in memory
pub struct StreamedBaseband {
    center_freq: f64,
    start_date: DateTime<Utc>,
    sample_rate: u32,
    wav: wave_stream::wave_reader::StreamWavReaderIterator<f32>,
}

impl StreamedBaseband {
    pub fn new(path: String) -> Self {
        info!("Loading baseband from {}", path);
        // TODO: For now we assume time is in local timezone, but I've made a SDR++ feature request that
        // TODO: would allow timezones to be specified in the filename (or simply the filenames being UTC)
        let date_regex = Regex::new(r".*/baseband_(\d+)Hz_(\d+)-(\d+)-(\d+)_(\d+)-(\d+)-(\d+).*\.wav").expect("Good Regex");
        let (freq, date) = match date_regex.captures(path.as_str()) {
            None => panic!("Unable to parse baseband filename, use $t_$f_$h-$m-$s_$d-$M-$y"),
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
        assert_eq!(open_wav.num_channels(), 2);
        assert!(open_wav.channels().front_left);
        assert!(open_wav.channels().front_right);

        let sample_rate = open_wav.sample_rate();

        let iter = open_wav.get_stream_f32_reader().unwrap().into_iter();

        Self {
            center_freq: freq,
            start_date: date,
            sample_rate,
            wav: iter,
        }
    }

    // If we run out of data, the vector will not have the same size as num_samples
    pub fn get_next(self: &mut Self, num_samples: u64) -> Vec<Complex<Scalar>> {
        let mut out = Vec::new();

        for i in 0..num_samples {
            let samps = match self.wav.next() {
                None => break,
                Some(v) => match(v) {
                    Ok(samp) => {samp}
                    Err(e) => break,
                }
            };
            // SAFETY: Safe because we checked on object creation, we need this to run fast
            let left = samps.front_left.unwrap_or_else(|| unsafe{ unreachable_unchecked(); });
            let right = samps.front_right.unwrap_or_else(|| unsafe{ unreachable_unchecked(); });

            let iqsamp = Complex::new(left, right);
            out.push(iqsamp);
        }

        out
    }

    pub fn get_sample_rate(self: &Self) -> u32 {
        self.sample_rate
    }

    pub fn get_center_freq(self: &Self) -> f64 {
        self.center_freq
    }
}

#[derive(Copy, Clone)]
struct FreqChange {
    t: f64,
    freq: f64
}

// Allows streaming samples from a frequencies file, without fully loading them in memory
pub struct StreamedSamplesFreqs {
    t: f64,
    tstep: f64,
    freqs: Vec<FreqChange>,
    center_freq: f64,
}

impl StreamedSamplesFreqs {

    // Returns current, and next freq change for given time
    fn find_freq_change_for(self: &Self, t: f64) -> Option<(FreqChange, FreqChange)> {
        match self.freqs.windows(2).find(|pair| pair[0].t >= t && pair[1].t > t) {
            None => None,
            Some(v) => Some((v[0], v[1])),
        }
    }

    // If we run out of data, the vector will not have the same size as num_samples
    pub fn get_next(self: &mut Self, num_samples: u64) -> Vec<Complex<Scalar>> {
        let mut out = Vec::new();
        let mut num_written = 0;

        while let Some(pair) = self.find_freq_change_for(self.t) {
            let t_remains = pair.1.t - self.t;
            let samps_remain = (t_remains / self.tstep).ceil() as u64;

            for _ in 0..samps_remain {
                debug_assert!(num_written <= num_samples);
                if num_written == num_samples {
                    return out;
                }

                let rf = pair.0.freq - self.center_freq;
                let w = 2.0 * std::f64::consts::PI * rf;
                out.push(Complex::new(w.sin() as Scalar, w.cos() as Scalar));

                num_written += 1;
                self.t += self.tstep;
            }
        }

        out
    }

    fn load_freqs(freqs_path: String) -> Vec<FreqChange> {
        let mut out = Vec::new();
        let lines = BufReader::new(File::open(freqs_path).unwrap()).lines();
        let re = Regex::new(r"\s*([0-9.]+)\s*,([0-9.]+)").unwrap();

        for maybe_line in lines {
            let line = maybe_line.unwrap();
            let regex_match = re.captures(line.as_str()).unwrap();
            let t = regex_match.get(1).unwrap().as_str().parse().unwrap();
            let freq = regex_match.get(2).unwrap().as_str().parse().unwrap();

            out.push(FreqChange{
                t,
                freq
            });
        }

        out
    }

    pub fn new(freqs_path: String, center_freq: f64, srate: u32) -> Self {
        let freqs = Self::load_freqs(freqs_path);
        Self {
            t: freqs[0].t,
            center_freq,
            tstep: 1.0 / (srate as f64),
            freqs,
        }
    }
}