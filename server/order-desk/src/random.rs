//! Random values for ids and secrets, straight from the operating system.

use alloy::hex;

/// 32 random bytes as lowercase `0x` hex: the form of `bytes32` in the database.
pub fn bytes32_hex() -> String {
    format!("0x{}", hex::encode(bytes32()))
}

pub fn bytes32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system has no random source");
    bytes
}
