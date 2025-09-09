//! Simple classes from streaming (non random access!) of samples, so that we can work with
//! big files without hogging memory and having long load times.

use chrono::{DateTime, TimeZone, Utc};
use log::{info, warn};
use regex::Regex;
use rustfft::num_complex::Complex;
use std::collections::HashMap;
use std::fs::File;
use std::hint::unreachable_unchecked;
use std::io::{BufRead, BufReader};
use std::path::Path;
use wave_stream::open_wav::OpenWav;
use wave_stream::samples_by_channel::SamplesByChannel;
use wave_stream::wave_header::{Channels, SampleFormat, WavHeader};
use wave_stream::wave_reader::StreamOpenWavReader;
use wave_stream::write_wav_to_file_path;

use ndarray::prelude::*;

pub type Scalar = f32;
pub type Sample = Complex<Scalar>;

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
        let date_regex =
            Regex::new(r".*/baseband_(\d+)Hz_(\d+)-(\d+)-(\d+)_(\d+)-(\d+)-(\d+).*\.wav")
                .expect("Good Regex");
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
                let date = chrono::Local
                    .with_ymd_and_hms(year, month, day, hour, min, sec)
                    .unwrap()
                    .to_utc();

                (freq, date)
            }
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

    // If we run out of data, the vector will not be overwritten!
    // We return number of samples read
    pub fn read_into(self: &mut Self, mut buf: &mut [Sample]) -> usize {
        let mut num_samples_read = 0;
        while let Some(Ok(samps)) = self.wav.next() {
            if num_samples_read == buf.len() {
                break;
            }

            // SAFETY: Safe because we checked on object creation, we need this to run fast
            let left = samps.front_left.unwrap_or_else(|| unsafe {
                unreachable_unchecked();
            });
            let right = samps.front_right.unwrap_or_else(|| unsafe {
                unreachable_unchecked();
            });

            let iqsamp = Complex::new(left, right);
            buf[num_samples_read] = iqsamp;
            num_samples_read += 1;
        }

        num_samples_read
    }

    // Advances the stream, without saving data, until the epoch indicated, assuming the
    // recording starts exactly at start date. Remember to use some margin, as the actual
    // start of recording may be anytime during the second
    // TODO: This will change if SDR++ gets improved date merged
    pub fn seek_epoch(self: &mut Self, epoch: f64) {
        let start_epoch = self.start_date.timestamp() as f64
            + (self.start_date.timestamp_subsec_nanos() as f64) * 1e-9;
        let delta = epoch - start_epoch;
        if delta < 0.0 {
            warn!("Baseband is older than given epoch, not seeking");
            return;
        }
        let num_samples = (delta * (self.sample_rate as f64)).floor() as usize;
        info!(
            "Seeking baseband {} samples to align with epoch {}",
            num_samples, epoch
        );

        for _ in 0..num_samples {
            _ = self.wav.next();
        }

        info!("Done");
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
    freq: f64,
}

// Allows streaming samples from a frequencies file, without fully loading them in memory
pub struct StreamedSamplesFreqs {
    t: f64,
    phase: f64,
    tstep: f64,
    freqs: Vec<FreqChange>,
    center_freq: f64,
}

// Times relative to given start time
pub struct FreqOnTimes {
    pub freq: f64,
    pub start: f64,
    pub end: f64,
}

impl StreamedSamplesFreqs {
    // Gets which frequencies are present on the interval of time starting at epoch
    // start and continuing for samples, and at which times they are on.
    // All samples are assumed to be relative to start epoch.
    pub fn get_frequencies_for_interval(&self, start: f64, dur: f64) -> Vec<FreqOnTimes> {
        let mut out = Vec::new();

        for pair in self.freqs.windows(2) {
            if pair[0].t < start || pair[0].t > start + dur {
                continue;
            }

            out.push(FreqOnTimes {
                freq: pair[0].freq,
                start: pair[0].t - start,
                end: pair[1].t - start,
            });
        }

        out
    }

    // Returns current, and next freq change for given time
    fn find_freq_change_for(self: &Self, t: f64) -> Option<(FreqChange, FreqChange)> {
        match self
            .freqs
            .windows(2)
            .find(|pair| pair[0].t <= t && pair[1].t > t)
        {
            None => None,
            Some(v) => Some((v[0], v[1])),
        }
    }

    // If we run out of data, the vector will be zero-padded
    // We return number of samples read alongside them.
    pub fn get_next(self: &mut Self, num_samples: usize) -> (Array1<Sample>, usize) {
        let mut out = Array1::zeros(num_samples);

        let mut num_written = 0;
        while let Some(pair) = self.find_freq_change_for(self.t) {
            debug_assert!(num_written <= num_samples);
            if num_written == num_samples {
                break;
            }

            let t_remains = pair.1.t - self.t;
            let samps_remain = (t_remains / self.tstep).ceil() as u64;
            let mut this_step_written: usize = 0;

            for _ in 0..samps_remain {
                if num_written == num_samples {
                    break;
                }

                let rf = pair.0.freq - self.center_freq;
                let w = 2.0 * std::f64::consts::PI * rf;
                self.phase += w * self.tstep;
                out[num_written] =
                    Sample::new(self.phase.sin() as Scalar, self.phase.cos() as Scalar);

                num_written += 1;
                this_step_written += 1;
                // DO not do timestepping here, as floating point precision error accumulates
            }

            // Do it here instead
            self.t += self.tstep * this_step_written as f64;
        }

        (out, num_written)
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

            out.push(FreqChange { t, freq });
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
            phase: 0.0,
        }
    }

    pub fn get_first_epoch(self: &Self) -> f64 {
        return self.freqs[0].t;
    }

    pub fn dump_to_wav(self: &mut Self, path: String) {
        let header = WavHeader {
            sample_format: SampleFormat::Float,
            channels: Channels::new().front_left().front_right(),
            sample_rate: (1.0 / self.tstep) as u32,
        };
        let open_wav = write_wav_to_file_path(Path::new(&path), header).unwrap();
        let mut writer = open_wav.get_random_access_f32_writer().unwrap();
        let mut i = 0;
        loop {
            let (samples, num_read) = self.get_next(10000);
            if num_read == 0 {
                break;
            }

            for sample in samples {
                let mut sample_per_channel = SamplesByChannel::new();
                sample_per_channel.front_left = Some(sample.re * 0.1);
                sample_per_channel.front_right = Some(sample.im * 0.1);
                let _ = writer.write_samples(i, sample_per_channel);
                i += 1;
            }
        }
    }
}
