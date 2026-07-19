//! Token accounting details used by phase-two usage and cost semantics.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names
)]

/// Three-state token count distinguishing absence from a reported zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TokenCount {
    /// The provider did not report this field.
    #[default]
    Unknown,
    /// The provider reported an explicit token count, including zero.
    Known(u64),
}

impl TokenCount {
    /// Returns the known value when present.
    pub fn known(self) -> Option<u64> {
        match self {
            Self::Unknown => None,
            Self::Known(value) => Some(value),
        }
    }

    /// Reports whether the count is known.
    pub fn is_known(self) -> bool {
        matches!(self, Self::Known(_))
    }
}

/// Detailed token accounting that preserves Unknown versus Known(0).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsageDetails {
    input_tokens: TokenCount,
    output_tokens: TokenCount,
    total_tokens: TokenCount,
    cached_input_tokens: TokenCount,
    cache_write_tokens: TokenCount,
    reasoning_tokens: TokenCount,
}

impl UsageDetails {
    /// Creates usage details with all fields set explicitly.
    pub fn new(
        input_tokens: TokenCount,
        output_tokens: TokenCount,
        total_tokens: TokenCount,
        cached_input_tokens: TokenCount,
        cache_write_tokens: TokenCount,
        reasoning_tokens: TokenCount,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens,
            cache_write_tokens,
            reasoning_tokens,
        }
    }

    /// Returns prompt/input tokens.
    pub fn input_tokens(self) -> TokenCount {
        self.input_tokens
    }

    /// Returns completion/output tokens.
    pub fn output_tokens(self) -> TokenCount {
        self.output_tokens
    }

    /// Returns total tokens when reported.
    pub fn total_tokens(self) -> TokenCount {
        self.total_tokens
    }

    /// Returns cached input tokens when reported.
    pub fn cached_input_tokens(self) -> TokenCount {
        self.cached_input_tokens
    }

    /// Returns cache write tokens when reported.
    pub fn cache_write_tokens(self) -> TokenCount {
        self.cache_write_tokens
    }

    /// Returns reasoning tokens when reported. This is an output subset.
    pub fn reasoning_tokens(self) -> TokenCount {
        self.reasoning_tokens
    }

    /// Returns true when any field is Known.
    pub fn has_any_known(self) -> bool {
        self.input_tokens.is_known()
            || self.output_tokens.is_known()
            || self.total_tokens.is_known()
            || self.cached_input_tokens.is_known()
            || self.cache_write_tokens.is_known()
            || self.reasoning_tokens.is_known()
    }
}
