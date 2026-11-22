pub mod analytics;
pub mod bots;
pub mod dca;
pub mod executor;
pub mod notifier;
pub mod protections;
pub mod rebalance;
pub mod rotation;
pub mod routes;
pub mod scheduler;
pub mod semi;
pub mod webhook;

use sha2::{Digest, Sha256};

pub fn sha256_hex32(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(32);
    for b in &digest[..16] {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}
