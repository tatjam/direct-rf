use serde::{Serialize, Deserialize};
use crate::sequence::PLLChange;

pub const MAX_UPLINK_MSG_SIZE: usize = 256;

// We use COBS to "encode" the messages. Each message is simply encoded by postcard in COBS
// mode, and a zero is sent after each message.

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