//! Catalog provenance and evidence freshness.

use crate::error::LlmError;

use super::ids::CatalogSourceId;

/// Source metadata attached to catalog facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSource {
    id: CatalogSourceId,
    reviewed_at: String,
    expires_at: Option<String>,
}

impl CatalogSource {
    /// Creates source metadata using ISO-8601 calendar dates.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when a date is invalid or expiry precedes review.
    pub fn new(
        id: CatalogSourceId,
        reviewed_at: impl Into<String>,
        expires_at: Option<impl Into<String>>,
    ) -> Result<Self, LlmError> {
        let reviewed_at = reviewed_at.into();
        validate_date(&reviewed_at)?;
        let expires_at = expires_at.map(Into::into);
        if let Some(date) = &expires_at {
            validate_date(date)?;
            if date < &reviewed_at {
                return Err(LlmError::Configuration(
                    "catalog expiry precedes review date".to_owned(),
                ));
            }
        }
        Ok(Self {
            id,
            reviewed_at,
            expires_at,
        })
    }

    /// Returns source ID.
    #[must_use]
    pub fn id(&self) -> &CatalogSourceId {
        &self.id
    }
    /// Returns review date.
    #[must_use]
    pub fn reviewed_at(&self) -> &str {
        &self.reviewed_at
    }
    /// Returns expiry date.
    #[must_use]
    pub fn expires_at(&self) -> Option<&str> {
        self.expires_at.as_deref()
    }

    /// Reports whether the source is stale at a calendar date.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when `today` is not a valid ISO-8601 calendar date.
    pub fn is_stale_on(&self, today: &str) -> Result<bool, LlmError> {
        validate_date(today)?;
        Ok(self
            .expires_at
            .as_deref()
            .is_some_and(|expiry| today > expiry))
    }
}

fn validate_date(value: &str) -> Result<(), LlmError> {
    let valid = value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    if !valid {
        return Err(LlmError::Configuration(
            "catalog dates must use YYYY-MM-DD".to_owned(),
        ));
    }
    let year = value[0..4].parse::<u32>().unwrap_or_default();
    let month = value[5..7].parse::<u32>().unwrap_or_default();
    let day = value[8..10].parse::<u32>().unwrap_or_default();
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if day == 0 || day > max_day {
        return Err(LlmError::Configuration(
            "catalog date is not a valid calendar day".to_owned(),
        ));
    }
    Ok(())
}
