//! Typed identifiers used by the model catalog.

use std::fmt;

use crate::error::{ValidationError, ValidationReason};

macro_rules! catalog_id {
    ($name:ident, $label:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Creates a non-empty, boundary-trimmed identifier.
            ///
            /// # Errors
            ///
            /// Returns a validation error when the identifier is empty, has boundary
            /// whitespace, or exceeds 256 bytes.
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(ValidationError::new(
                        $label,
                        ValidationReason::Empty,
                        "identifier must not be empty",
                    ));
                }
                if value.trim() != value {
                    return Err(ValidationError::new(
                        $label,
                        ValidationReason::BoundaryWhitespace,
                        "identifier must not have boundary whitespace",
                    ));
                }
                if value.len() > 256 {
                    return Err(ValidationError::new(
                        $label,
                        ValidationReason::OutOfRange,
                        "identifier exceeds 256 bytes",
                    ));
                }
                Ok(Self(value))
            }
            /// Returns the identifier text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

catalog_id!(
    ProductId,
    "product_id",
    "Exact provider product identifier."
);
catalog_id!(
    ProviderModelId,
    "provider_model_id",
    "Provider-owned model identifier."
);
catalog_id!(
    DeploymentId,
    "deployment_id",
    "Provider deployment identifier."
);
catalog_id!(
    WireModelValue,
    "wire_model_value",
    "Exact model value serialized on the wire."
);
catalog_id!(
    CatalogSourceId,
    "catalog_source_id",
    "Catalog evidence source identifier."
);
