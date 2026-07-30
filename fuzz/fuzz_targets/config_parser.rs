#![no_main]

use libfuzzer_sys::fuzz_target;
use philo_config::ProviderConfigDocument;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    if let Ok(document) = ProviderConfigDocument::from_json(&input) {
        let current = document
            .to_current_json()
            .expect("a parsed document must serialize");
        ProviderConfigDocument::from_json(&current)
            .expect("the current writer must produce readable configuration");
    }
});
