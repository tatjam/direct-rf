use std::{env, fs};
use serialport::{SerialPort, SerialPortType};
use regex::Regex;
use common::sequence::Sequence;

fn find_port() -> Result<String, &'static str> {
    let ports = serialport::available_ports().unwrap();
    for port in ports {
        match port.port_type {
            SerialPortType::UsbPort(info) => {

                return Result::Ok(port.port_name);
            }
            _ => {
                continue;
            }
        }
    }

    Result::Err("ST-Link VCOM port not found")
}

struct FrequencyOrder {
    t: u32,
    freq: u32,
}

fn parse_freqs(file: String) -> Result<Vec<FrequencyOrder>, &'static str> {
    let r = Regex::new(r"([0-9]+),([0-9]+)").unwrap();
    let mut out: Vec<FrequencyOrder> = Vec::new();

    for line in file.lines() {
        let captures = r.captures(line).unwrap();
        out.push(FrequencyOrder {
            t: captures.get(1).unwrap().as_str().parse().unwrap(),
            freq: captures.get(2).unwrap().as_str().parse().unwrap()
        })
    }

    Ok(out)
}

fn build_sequence(orders: Vec<FrequencyOrder>) -> Result<Sequence, &'static str> {
    // We achieve the output frequency by the formula:
    // FOUT = FREF * (DIVN + 1 + FRACN / 2^13) / (DIVP + 1)
    // where FREF = 12MHz, and furthermore we have the following limitations:
    // - fracn ∈ [0, 2^13 - 1]
    // - divn ∈ [7, 419]
    // - divp ∈ [0, 127]
    // - vcosel must be 1 if FVCO is between 150 and 420MHz
    //   then FVCO = FREF *  (DIVN + 1 + FRACN / 2^13)
    //   the valid range of FOUT is then [1.181, 420]MHz
    // - vcosel must be 0 if FVCO is between 384 and 1672MHz
    //   then FVCO = 2 * FREF * (DIVN + 1 + FRACN / 2^13)
    //   the valid range of FOUT is then [1.51, 836]MHz
    // (Other frequencies are invalid for FVCO)
    Err("Guamedo")
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let freqs_path = if(args.len() < 2) {
        String::from("freqs.csv")
    } else {
        args[1].clone()
    };

    let freqs = parse_freqs(fs::read_to_string(freqs_path).unwrap()).unwrap();
    let seq = build_sequence(freqs).unwrap();


    let port = find_port().unwrap();
}
