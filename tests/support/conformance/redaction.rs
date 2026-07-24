use serde::Serialize;

/// Bounded failure observation that never stores body, prompt, output, or request-id values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedFailure {
    pub category: &'static str,
    pub http_status: Option<u16>,
    pub provider_code: Option<&'static str>,
    pub body_length: usize,
    pub body_digest: String,
}

impl RedactedFailure {
    pub fn observe(
        category: &'static str,
        http_status: Option<u16>,
        provider_code: Option<&str>,
        body: &[u8],
    ) -> Self {
        let provider_code = match provider_code {
            Some("invalid_api_key") => Some("invalid_api_key"),
            Some("rate_limit_exceeded") => Some("rate_limit_exceeded"),
            _ => None,
        };
        Self {
            category,
            http_status,
            provider_code,
            body_length: body.len(),
            body_digest: format!("fnv1a64:{:016x}", fnv1a(body)),
        }
    }
}

pub fn contains_forbidden_value(text: &str, canaries: &[&str]) -> bool {
    let lowercase = text.to_ascii_lowercase();
    canaries
        .iter()
        .any(|value| !value.is_empty() && lowercase.contains(&value.to_ascii_lowercase()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
