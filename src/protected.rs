//! Single owner of the protection tables shared by every extension axis.
//!
//! The SDK admits caller-declared provider differences on exactly four axes —
//! **body**, **header**, **endpoint**, and **auth**. Each axis has one bounded
//! entry point, and every entry point refuses to write the items listed here.
//!
//! These tables are defined once. Adding a protected item is a one-place edit,
//! and no axis is allowed to keep a private copy of the same rule.
//!
//! | Axis | Entry point | Table |
//! |---|---|---|
//! | body | [`crate::protocol_options`] raw extensions | [`ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS`], [`OPENAI_CHAT_PROTECTED_BODY_FIELDS`], [`PROTECTED_BODY_KEY_SHAPES`] |
//! | header | `GenerationOptions::with_header`, provider headers, dynamic header policy | [`PROTECTED_HEADERS`] |
//! | endpoint | `EndpointConfig` / `EndpointTemplate` | [`REQUIRED_ENDPOINT_SCHEME`] |
//! | auth | `AuthScheme` / `CredentialBinding` | [`AUTH_INELIGIBLE_HEADERS`] |
//!
//! The auth axis has one further rule with no table to centralize: a credential
//! binds to an **exact normalized origin** — scheme, host, and effective port must
//! all match. Suffix, wildcard, and path-scoped matching are offered on no axis.
//! `CredentialBinding::validate` enforces it structurally through `Origin` equality.

use http::HeaderName;

/// Headers no ordinary layer may write.
///
/// Transport, protocol, provider, and caller layers are all bound by this table.
/// A header listed here is writable only by the owner named in
/// `HeaderPolicy::allows`, and never by a request-level override.
pub const PROTECTED_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "host",
    "content-length",
    "content-type",
    "accept",
    "transfer-encoding",
    "connection",
    "cookie",
    "set-cookie",
    "user-agent",
];

/// Headers an authentication scheme may not claim as its credential carrier.
///
/// This is [`PROTECTED_HEADERS`] minus the two authorization names, which are
/// precisely the headers auth *does* own.
pub const AUTH_INELIGIBLE_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "content-type",
    "accept",
    "transfer-encoding",
    "connection",
    "cookie",
    "set-cookie",
    "user-agent",
];

/// Body keys refused on every protocol because they name a non-body owner.
///
/// A raw body extension admits unknown top-level fields. It never admits a key
/// shaped like a header, a credential, or a protocol version, regardless of
/// whether the selected protocol happens to use that name in its body.
pub const PROTECTED_BODY_KEY_SHAPES: &[&str] = &[
    "accept",
    "anthropic-beta",
    "anthropic-version",
    "api_key",
    "auth",
    "authorization",
    "beta",
    "content-length",
    "content-type",
    "header",
    "headers",
    "host",
    "version",
    "x-api-key",
];

/// Anthropic Messages body fields owned by the SDK.
pub const ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS: &[&str] = &[
    "max_tokens",
    "messages",
    "model",
    "output_config",
    "stream",
    "system",
    "temperature",
    "thinking",
    "tool_choice",
    "tools",
];

/// `OpenAI` Chat Completions body fields owned by the SDK.
///
/// `provider` is deliberately absent: it is an aggregation-gateway product
/// parameter, not an SDK-owned field, and is declared through the body axis.
pub const OPENAI_CHAT_PROTECTED_BODY_FIELDS: &[&str] = &[
    "max_completion_tokens",
    "max_tokens",
    "messages",
    "model",
    "n",
    "parallel_tool_calls",
    "reasoning_effort",
    "response_format",
    "stream",
    "stream_options",
    "temperature",
    "tool_choice",
    "tools",
];

/// The only endpoint scheme a production deployment may resolve to.
///
/// Both production gates read this constant: `EndpointNetworkPolicy::validate`
/// before an endpoint may resolve, and `CredentialBinding::exact_https_origin`
/// before a credential may bind to one.
pub const REQUIRED_ENDPOINT_SCHEME: &str = "https";

/// Returns whether an ordinary layer is forbidden from writing this header.
#[must_use]
pub fn is_protected_header(name: &HeaderName) -> bool {
    contains(PROTECTED_HEADERS, name.as_str())
}

/// Returns whether an authentication scheme is forbidden from claiming this header.
#[must_use]
pub fn is_auth_ineligible_header(name: &HeaderName) -> bool {
    contains(AUTH_INELIGIBLE_HEADERS, name.as_str())
}

/// Returns whether a raw body extension is forbidden from writing this key.
///
/// `protocol_fields` is the selected protocol's table, for example
/// [`OPENAI_CHAT_PROTECTED_BODY_FIELDS`]. The shared
/// [`PROTECTED_BODY_KEY_SHAPES`] table always applies on top of it.
#[must_use]
pub fn is_protected_body_field(protocol_fields: &[&str], key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    contains(protocol_fields, &key) || contains(PROTECTED_BODY_KEY_SHAPES, &key)
}

fn contains(table: &[&str], value: &str) -> bool {
    table.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_ineligible_is_exactly_protected_minus_the_authorization_owners() {
        let expected = PROTECTED_HEADERS
            .iter()
            .filter(|name| !matches!(**name, "authorization" | "proxy-authorization"))
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(AUTH_INELIGIBLE_HEADERS, expected.as_slice());
    }

    #[test]
    fn every_table_is_lowercase_sorted_and_duplicate_free() {
        for table in [
            PROTECTED_BODY_KEY_SHAPES,
            ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS,
            OPENAI_CHAT_PROTECTED_BODY_FIELDS,
        ] {
            let mut sorted = table.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(table, sorted.as_slice());
        }
        for table in [PROTECTED_HEADERS, AUTH_INELIGIBLE_HEADERS] {
            let mut deduped = table.to_vec();
            deduped.sort_unstable();
            deduped.dedup();
            assert_eq!(table.len(), deduped.len());
        }
        for table in [
            PROTECTED_HEADERS,
            AUTH_INELIGIBLE_HEADERS,
            PROTECTED_BODY_KEY_SHAPES,
            ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS,
            OPENAI_CHAT_PROTECTED_BODY_FIELDS,
        ] {
            assert!(
                table
                    .iter()
                    .all(|entry| *entry == entry.to_ascii_lowercase())
            );
        }
    }

    #[test]
    fn body_protection_is_case_insensitive_and_spans_both_tables() {
        assert!(is_protected_body_field(
            OPENAI_CHAT_PROTECTED_BODY_FIELDS,
            "MESSAGES"
        ));
        assert!(is_protected_body_field(
            OPENAI_CHAT_PROTECTED_BODY_FIELDS,
            "X-Api-Key"
        ));
        assert!(!is_protected_body_field(
            OPENAI_CHAT_PROTECTED_BODY_FIELDS,
            "provider"
        ));
        assert!(is_protected_body_field(
            ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS,
            "thinking"
        ));
        assert!(!is_protected_body_field(
            ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS,
            "reasoning_effort"
        ));
    }

    #[test]
    fn header_predicates_agree_with_their_tables() {
        assert!(is_protected_header(&HeaderName::from_static(
            "authorization"
        )));
        assert!(is_protected_header(&HeaderName::from_static("set-cookie")));
        assert!(!is_protected_header(&HeaderName::from_static(
            "x-request-id"
        )));
        assert!(!is_auth_ineligible_header(&HeaderName::from_static(
            "authorization"
        )));
        assert!(is_auth_ineligible_header(&HeaderName::from_static(
            "user-agent"
        )));
    }
}
