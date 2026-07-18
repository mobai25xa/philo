//! Prints the SDK and frozen behavior-contract metadata.

use philo::{PHASE_ONE_CONTRACT_ID, PHASE_ONE_CONTRACT_VERSION, SDK_NAME, SDK_VERSION};

fn main() {
    println!("{SDK_NAME} {SDK_VERSION}");
    println!("contract: {PHASE_ONE_CONTRACT_ID} {PHASE_ONE_CONTRACT_VERSION}");
}
