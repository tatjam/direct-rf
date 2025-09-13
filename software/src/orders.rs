use regex::Regex;

// The sequencer will generate a pseudo-random sequence that spends t_us
// on band of width bandwidth_Hz centered around freq_Hz, with n frequency
// changes.
pub struct FrequencyOrder {
    pub t_us: u32,
    pub freq_hz: u32,
    pub bandwidth_hz: u32,
    pub n: usize,
}

pub fn parse_orders(file: String) -> Result<Vec<FrequencyOrder>, &'static str> {
    let r = Regex::new(r"(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)").unwrap();
    let mut out: Vec<FrequencyOrder> = Vec::new();
    let lines = file.lines();

    for line in lines {
        let captures = r.captures(line).unwrap();
        out.push(FrequencyOrder {
            t_us: captures.get(1).unwrap().as_str().parse().unwrap(),
            freq_hz: captures.get(2).unwrap().as_str().parse().unwrap(),
            bandwidth_hz: captures.get(3).unwrap().as_str().parse().unwrap(),
            n: captures.get(4).unwrap().as_str().parse().unwrap(),
        })
    }

    Ok(out)
}
