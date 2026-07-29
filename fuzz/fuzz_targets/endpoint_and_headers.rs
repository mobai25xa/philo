#![no_main]

use http::{HeaderName, HeaderValue};
use libfuzzer_sys::fuzz_target;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::headers::HeaderOperation;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = EndpointConfig::absolute(&text);
    let split = data.len() / 2;
    if let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(&data[..split]),
        HeaderValue::from_bytes(&data[split..]),
    ) {
        let _ = HeaderOperation::set(name, value);
    }
});
