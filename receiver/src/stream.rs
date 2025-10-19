//! Simple classes from streaming (non random access!) of samples, so that we can work with
//! big files without hogging memory and having long load times.

use anyhow::{Result, anyhow};
use regex::Regex;
use rustfft::num_complex::Complex;
use std::fs::File;
use std::io::{BufRead, BufReader};

use ndarray::prelude::*;

pub type Scalar = f32;
pub type Sample = Complex<Scalar>;

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
    // FREQUENCIES ARE RELATIVE TO CENTER FREQUENCY!
    pub fn get_frequencies_for_interval(&self, start: f64, dur: f64) -> Vec<FreqOnTimes> {
        let mut out = Vec::new();

        for pair in self.freqs.windows(2) {
            if pair[0].t < start || pair[0].t > start + dur {
                continue;
            }

            out.push(FreqOnTimes {
                freq: pair[0].freq - self.center_freq,
                start: pair[0].t,
                end: pair[1].t,
            });
        }

        out
    }

    pub fn get_center_freq(&self) -> f64 {
        self.center_freq
    }

    // Returns current, and next freq change for given time
    fn find_freq_change_for(&self, t: f64) -> Option<(FreqChange, FreqChange)> {
        self.freqs
            .windows(2)
            .find(|pair| pair[0].t <= t && pair[1].t > t)
            .map(|v| (v[0], v[1]))
    }

    // If we run out of data, the vector will be zero-padded
    // We return number of samples read alongside them.
    pub fn get_next(&mut self, num_samples: usize) -> (Array1<Sample>, usize) {
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

    fn load_freqs(freqs_path: String) -> Result<Vec<FreqChange>> {
        let mut out = Vec::new();
        let lines = BufReader::new(File::open(freqs_path)?).lines();
        let re = Regex::new(r"\s*([0-9.]+)\s*,([0-9.]+)")?;

        for maybe_line in lines {
            let line = maybe_line?;
            let regex_match = re.captures(line.as_str()).ok_or(anyhow!("Wrong regex"))?;
            let t = regex_match.get(1).expect("Regex").as_str().parse()?;
            let freq = regex_match.get(2).expect("Regex").as_str().parse()?;

            out.push(FreqChange { t, freq });
        }

        Ok(out)
    }

    pub fn new(freqs_path: String, center_freq: f64, srate: u32) -> Result<Self> {
        let freqs = Self::load_freqs(freqs_path)?;
        Ok(Self {
            t: freqs[0].t,
            center_freq,
            tstep: 1.0 / (srate as f64),
            freqs,
            phase: 0.0,
        })
    }

    pub fn get_first_epoch(&self) -> f64 {
        self.freqs[0].t
    }

    pub fn dump_to_sdriq(&mut self, path: String) -> Result<()> {
        let header = sdriq::Header {
            samp_rate: (1.0 / self.tstep) as u32,
            center_freq: self.center_freq as u64,
            start_timestamp: 0,
            samp_size: 24,
        };

        let file = File::create(path)?;
        let mut sink = sdriq::Sink::new(file, header)?;
        loop {
            let (samples, num_read) = self.get_next(10000);
            if num_read == 0 {
                break;
            }
            sink.write_all_samples_denorm(samples.as_slice().expect("Flat memory"));
        }

        Ok(())
    }
}
