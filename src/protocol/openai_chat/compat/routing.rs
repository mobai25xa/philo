//! Private closed provider-routing encoder.

use serde::Serialize;

use crate::provider::compat::ResolvedProviderRouting;
use crate::provider::{DataRetention, RoutingFallback, RoutingSort, UpstreamId};

/// Closed wire object. No map or flatten field is admitted.
#[derive(Serialize)]
pub(crate) struct ProviderRoutingWire<'a> {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    only: Vec<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ignore: Vec<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    order: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_fallbacks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_collection: Option<DataCollectionWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zdr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<RoutingSortWire>,
}

impl<'a> From<&'a ResolvedProviderRouting> for ProviderRoutingWire<'a> {
    fn from(value: &'a ResolvedProviderRouting) -> Self {
        let retention = value.data_retention.map(|item| item.0);
        Self {
            only: value.allowed.iter().map(UpstreamId::as_str).collect(),
            ignore: value.denied.iter().map(UpstreamId::as_str).collect(),
            order: value.order.iter().map(UpstreamId::as_str).collect(),
            allow_fallbacks: value.fallback.as_ref().map(RoutingFallback::enabled),
            data_collection: match retention {
                Some(DataRetention::Denied | DataRetention::ZeroDataRetention) => {
                    Some(DataCollectionWire::Deny)
                }
                Some(DataRetention::Allowed) | None => None,
            },
            zdr: matches!(retention, Some(DataRetention::ZeroDataRetention)).then_some(true),
            sort: value.sort.map(Into::into),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum DataCollectionWire {
    Deny,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum RoutingSortWire {
    Price,
    Latency,
    Throughput,
}

impl From<RoutingSort> for RoutingSortWire {
    fn from(value: RoutingSort) -> Self {
        match value {
            RoutingSort::Price => Self::Price,
            RoutingSort::Latency => Self::Latency,
            RoutingSort::Throughput => Self::Throughput,
        }
    }
}
