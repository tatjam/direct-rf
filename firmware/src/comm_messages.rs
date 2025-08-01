use heapless::Vec;

struct ClearFreqSequence;

struct PushFreqSequence {
    values: Vec<u16, 16>,
    times: Vec<u16, 16>,
}
