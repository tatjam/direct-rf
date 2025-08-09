use core::ops::Deref;
use heapless::Vec;
use serde::{Serialize, Deserialize};
use crate::sequence::PLLChange;

// We use COBS to "encode" the messages. Each message is simply encoded by postcard in COBS
// mode, and a zero is sent before and after each message (yes, duplicated zeroes are sent
// but this allows the receiver to connect "in the fly", which could save us a headache)

#[derive(Serialize, Deserialize)]
pub enum UplinkMsg {
    Ping(),
    PushPLLChange(PLLChange),
    PushFracn(u8, [u16; 32]),
    ClearBuffers(),
    StartNow(),
    StopNow(),
    SetLooping(bool),
    EpochNow(i64),
    StartAtEpoch(i64),
}

#[derive(Serialize, Deserialize)]
pub enum DownlinkMsg {
    Pong(),
}