use heapless::Vec;

struct ClearFreqSequence;

struct PushFreqSequence {
    times: Vec<u16, 16>,
    values: Vec<u16, 16>,
}


