use crate::sequence::PLLChange;
use serde::{Deserialize, Serialize};

pub const MAX_UPLINK_MSG_SIZE: usize = 256;

// We use COBS to "encode" the messages. Each message is simply encoded by postcard in COBS
// mode, and a zero is sent after each message.

#[derive(Serialize, Deserialize)]
pub enum UplinkMsg {
    Ping(),
    PushPLLChange(PLLChange),
    PushFracn(u8, [u16; 32]),
    ClearBuffer(),
    StartNow(),
    StopNow(),
}
