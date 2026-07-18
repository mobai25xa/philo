//! Strongly typed identifiers shared across request lifecycle boundaries.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

macro_rules! lifecycle_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Creates a non-empty identifier without normalization.
            pub fn new(value: impl Into<String>) -> Result<Self, crate::error::ValidationError> {
                let value = value.into();
                let reason = if value.is_empty() {
                    Some(crate::error::ValidationReason::Empty)
                } else if value.trim() != value {
                    Some(crate::error::ValidationReason::BoundaryWhitespace)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    return Err(crate::error::ValidationError::new(
                        stringify!($name),
                        reason,
                        "lifecycle id must be non-empty and have no boundary whitespace",
                    ));
                }
                Ok(Self(value))
            }

            /// Returns the identifier.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns its string value.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

lifecycle_id!(
    LocalRequestId,
    "Identifier allocated by philo for one local request attempt."
);
lifecycle_id!(
    ProviderRequestId,
    "Identifier returned in provider response headers."
);
lifecycle_id!(
    GenerationId,
    "Identifier returned by the generation protocol body."
);
lifecycle_id!(
    TraceId,
    "Application telemetry identifier that may correlate multiple SDK requests."
);
