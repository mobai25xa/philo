//! Token accounting and local cost estimation for phase-two semantics.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names
)]

use crate::error::{CostError, CostFailure};

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

    /// Returns true when the three core counters are all Known.
    pub fn has_complete_core(self) -> bool {
        self.input_tokens.is_known()
            && self.output_tokens.is_known()
            && self.total_tokens.is_known()
    }

    /// Validates subset and total relationships for any known fields.
    pub fn validate_relationships(self) -> Result<(), CostError> {
        if let (TokenCount::Known(input), TokenCount::Known(output), TokenCount::Known(total)) =
            (self.input_tokens, self.output_tokens, self.total_tokens)
            && input.checked_add(output) != Some(total)
        {
            return Err(CostError::new(
                "usage.total_tokens",
                CostFailure::InconsistentUsage,
                None,
                "usage total does not equal input + output",
            ));
        }

        if let (TokenCount::Known(input), TokenCount::Known(cached), TokenCount::Known(write)) = (
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_tokens,
        ) {
            if cached.checked_add(write).is_none_or(|sum| sum > input) {
                return Err(CostError::new(
                    "usage.cached_input_tokens",
                    CostFailure::InconsistentUsage,
                    None,
                    "cached input subsets exceed input tokens",
                ));
            }
        } else {
            if let (TokenCount::Known(input), TokenCount::Known(cached)) =
                (self.input_tokens, self.cached_input_tokens)
                && cached > input
            {
                return Err(CostError::new(
                    "usage.cached_input_tokens",
                    CostFailure::InconsistentUsage,
                    None,
                    "cached input exceeds input tokens",
                ));
            }
            if let (TokenCount::Known(input), TokenCount::Known(write)) =
                (self.input_tokens, self.cache_write_tokens)
                && write > input
            {
                return Err(CostError::new(
                    "usage.cache_write_tokens",
                    CostFailure::InconsistentUsage,
                    None,
                    "cache write exceeds input tokens",
                ));
            }
        }

        if let (TokenCount::Known(output), TokenCount::Known(reasoning)) =
            (self.output_tokens, self.reasoning_tokens)
            && reasoning > output
        {
            return Err(CostError::new(
                "usage.reasoning_tokens",
                CostFailure::InconsistentUsage,
                None,
                "reasoning tokens exceed output tokens",
            ));
        }
        Ok(())
    }
}

/// Result of merging a newer usage snapshot into previously observed details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageMergeOutcome {
    /// No public usage event should be emitted.
    Unchanged,
    /// Emit a complete P1 usage event for the first time the core counters are known.
    EmitP1 {
        /// Full detailed usage snapshot containing complete core counters.
        details: UsageDetails,
    },
    /// Emit only detailed usage because at least one core counter is still unknown
    /// or because previously unknown optional fields became known.
    EmitDetailed {
        /// Latest detailed usage snapshot.
        details: UsageDetails,
    },
}

/// Merges usage according to the frozen P2 Official `OpenAI` rules.
///
/// `Unknown → Known` fills missing values and may emit. Identical known values
/// are ignored. Conflicting known values fail closed. Numeric accumulation is
/// never performed.
pub fn merge_usage_details(
    previous: Option<UsageDetails>,
    next: UsageDetails,
) -> Result<(UsageDetails, UsageMergeOutcome), CostError> {
    next.validate_relationships()?;
    let Some(previous) = previous else {
        let outcome = if next.has_complete_core() {
            UsageMergeOutcome::EmitP1 { details: next }
        } else if next.has_any_known() {
            UsageMergeOutcome::EmitDetailed { details: next }
        } else {
            UsageMergeOutcome::Unchanged
        };
        return Ok((next, outcome));
    };

    let merged = UsageDetails::new(
        merge_token_count(
            previous.input_tokens,
            next.input_tokens,
            "usage.input_tokens",
        )?,
        merge_token_count(
            previous.output_tokens,
            next.output_tokens,
            "usage.output_tokens",
        )?,
        merge_token_count(
            previous.total_tokens,
            next.total_tokens,
            "usage.total_tokens",
        )?,
        merge_token_count(
            previous.cached_input_tokens,
            next.cached_input_tokens,
            "usage.cached_input_tokens",
        )?,
        merge_token_count(
            previous.cache_write_tokens,
            next.cache_write_tokens,
            "usage.cache_write_tokens",
        )?,
        merge_token_count(
            previous.reasoning_tokens,
            next.reasoning_tokens,
            "usage.reasoning_tokens",
        )?,
    );
    merged.validate_relationships()?;

    if merged == previous {
        return Ok((merged, UsageMergeOutcome::Unchanged));
    }

    let previously_complete = previous.has_complete_core();
    let outcome = if !previously_complete && merged.has_complete_core() {
        UsageMergeOutcome::EmitP1 { details: merged }
    } else {
        UsageMergeOutcome::EmitDetailed { details: merged }
    };
    Ok((merged, outcome))
}

fn merge_token_count(
    previous: TokenCount,
    next: TokenCount,
    field: &'static str,
) -> Result<TokenCount, CostError> {
    match (previous, next) {
        (TokenCount::Unknown, next) => Ok(next),
        (TokenCount::Known(value), TokenCount::Unknown) => Ok(TokenCount::Known(value)),
        (TokenCount::Known(left), TokenCount::Known(right)) if left == right => {
            Ok(TokenCount::Known(left))
        }
        (TokenCount::Known(_), TokenCount::Known(_)) => Err(CostError::new(
            field,
            CostFailure::InconsistentUsage,
            None,
            "usage field changed after becoming known",
        )),
    }
}

/// Uppercase ISO-4217 currency code used by local price profiles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    /// Creates a currency code. Phase-two price fixtures use `USD`.
    pub fn new(value: impl Into<String>) -> Result<Self, CostError> {
        let value = value.into();
        if value.len() != 3
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() && byte.is_ascii_alphabetic())
        {
            return Err(CostError::new(
                "price.currency",
                CostFailure::InvalidCurrency,
                None,
                "currency must be an uppercase ISO-4217 code",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the currency code.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit local pricing used to estimate provider costs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceProfile {
    version: String,
    source: String,
    currency: CurrencyCode,
    input_per_million_micros: u64,
    output_per_million_micros: u64,
    cached_input_per_million_micros: u64,
    cache_write_per_million_micros: u64,
}

impl PriceProfile {
    /// Creates a price profile. Prices must be supplied by the caller; model names
    /// never imply a price table.
    pub fn new(
        version: impl Into<String>,
        source: impl Into<String>,
        currency: CurrencyCode,
        input_per_million_micros: u64,
        output_per_million_micros: u64,
        cached_input_per_million_micros: u64,
        cache_write_per_million_micros: u64,
    ) -> Result<Self, CostError> {
        let version = version.into();
        let source = source.into();
        if version.is_empty() || source.is_empty() {
            return Err(CostError::new(
                "price",
                CostFailure::InvalidPriceProfile,
                None,
                "price version and source must be non-empty",
            ));
        }
        Ok(Self {
            version,
            source,
            currency,
            input_per_million_micros,
            output_per_million_micros,
            cached_input_per_million_micros,
            cache_write_per_million_micros,
        })
    }

    /// Returns the price version label.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the price source label.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the currency.
    pub fn currency(&self) -> &CurrencyCode {
        &self.currency
    }
}

/// A monetary amount in micro-currency units, or unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneyAmount {
    /// No price or insufficient usage to compute this line item.
    Unknown,
    /// Micro-currency units using checked `i128` arithmetic.
    Micros(i128),
}

/// Local cost estimate produced from usage and an optional price profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CostEstimate {
    currency: Option<CurrencyCode>,
    input: MoneyAmount,
    output: MoneyAmount,
    cached_input: MoneyAmount,
    cache_write: MoneyAmount,
    total: MoneyAmount,
    price_version: Option<String>,
    price_source: Option<String>,
}

impl CostEstimate {
    /// Returns the currency when a price profile was supplied.
    pub fn currency(&self) -> Option<&CurrencyCode> {
        self.currency.as_ref()
    }

    /// Returns the uncached input cost.
    pub fn input(&self) -> MoneyAmount {
        self.input
    }

    /// Returns the output cost. Reasoning tokens are included here only.
    pub fn output(&self) -> MoneyAmount {
        self.output
    }

    /// Returns the cached input cost.
    pub fn cached_input(&self) -> MoneyAmount {
        self.cached_input
    }

    /// Returns the cache write cost.
    pub fn cache_write(&self) -> MoneyAmount {
        self.cache_write
    }

    /// Returns the total when every line item is known.
    pub fn total(&self) -> MoneyAmount {
        self.total
    }

    /// Returns the price version when a profile was supplied.
    pub fn price_version(&self) -> Option<&str> {
        self.price_version.as_deref()
    }

    /// Returns the price source when a profile was supplied.
    pub fn price_source(&self) -> Option<&str> {
        self.price_source.as_deref()
    }
}

/// Estimates local cost from usage and an optional explicit price profile.
///
/// Missing prices never become zero: all money amounts stay `Unknown`.
pub fn estimate_cost(
    usage: &UsageDetails,
    price: Option<&PriceProfile>,
) -> Result<CostEstimate, CostError> {
    usage.validate_relationships()?;
    let Some(price) = price else {
        return Ok(CostEstimate {
            currency: None,
            input: MoneyAmount::Unknown,
            output: MoneyAmount::Unknown,
            cached_input: MoneyAmount::Unknown,
            cache_write: MoneyAmount::Unknown,
            total: MoneyAmount::Unknown,
            price_version: None,
            price_source: None,
        });
    };

    let cached_input = priced_tokens(
        usage.cached_input_tokens,
        price.cached_input_per_million_micros,
    )?;
    let cache_write = priced_tokens(
        usage.cache_write_tokens,
        price.cache_write_per_million_micros,
    )?;
    let output = priced_tokens(usage.output_tokens, price.output_per_million_micros)?;
    let input = match (
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_tokens,
    ) {
        (TokenCount::Known(input), TokenCount::Known(cached), TokenCount::Known(write)) => {
            let uncached = input
                .checked_sub(cached)
                .and_then(|value| value.checked_sub(write))
                .ok_or_else(|| {
                    CostError::new(
                        "usage.input_tokens",
                        CostFailure::InconsistentUsage,
                        None,
                        "cached input subsets exceed input tokens",
                    )
                })?;
            MoneyAmount::Micros(round_micros(uncached, price.input_per_million_micros)?)
        }
        (TokenCount::Known(input), TokenCount::Unknown, TokenCount::Unknown) => {
            MoneyAmount::Micros(round_micros(input, price.input_per_million_micros)?)
        }
        _ => MoneyAmount::Unknown,
    };

    let total = match (input, output, cached_input, cache_write) {
        (
            MoneyAmount::Micros(a),
            MoneyAmount::Micros(b),
            MoneyAmount::Micros(c),
            MoneyAmount::Micros(d),
        ) => {
            let sum = a
                .checked_add(b)
                .and_then(|value| value.checked_add(c))
                .and_then(|value| value.checked_add(d))
                .ok_or_else(|| {
                    CostError::new(
                        "cost.total",
                        CostFailure::Overflow,
                        None,
                        "cost total overflowed checked i128 arithmetic",
                    )
                })?;
            MoneyAmount::Micros(sum)
        }
        _ => MoneyAmount::Unknown,
    };

    Ok(CostEstimate {
        currency: Some(price.currency.clone()),
        input,
        output,
        cached_input,
        cache_write,
        total,
        price_version: Some(price.version.clone()),
        price_source: Some(price.source.clone()),
    })
}

fn priced_tokens(
    tokens: TokenCount,
    rate_per_million_micros: u64,
) -> Result<MoneyAmount, CostError> {
    match tokens {
        TokenCount::Unknown => Ok(MoneyAmount::Unknown),
        TokenCount::Known(value) => Ok(MoneyAmount::Micros(round_micros(
            value,
            rate_per_million_micros,
        )?)),
    }
}

fn round_micros(tokens: u64, rate_per_million_micros: u64) -> Result<i128, CostError> {
    let tokens = i128::from(tokens);
    let rate = i128::from(rate_per_million_micros);
    let product = tokens.checked_mul(rate).ok_or_else(|| {
        CostError::new(
            "cost",
            CostFailure::Overflow,
            None,
            "token price product overflowed checked i128 arithmetic",
        )
    })?;
    // Half-up to the nearest micro-currency unit: (product + 500_000) / 1_000_000.
    let rounded = product
        .checked_add(500_000)
        .and_then(|value| value.checked_div(1_000_000))
        .ok_or_else(|| {
            CostError::new(
                "cost",
                CostFailure::Overflow,
                None,
                "token price rounding overflowed checked i128 arithmetic",
            )
        })?;
    Ok(rounded)
}
