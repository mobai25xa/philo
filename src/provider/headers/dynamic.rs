//! Restricted dynamic header policy callbacks.

use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt as _;
use http::HeaderName;
use tokio::time::Instant;

use crate::domain::{LocalRequestId, ModelId, ProtocolId, ProviderId};
use crate::error::{HeaderPolicyError, HeaderPolicyFailure, LlmError};
use crate::protocol::ResponseFormatKind;
use crate::provider::catalog::ProductId;
use crate::transport::{CancellationToken, RequestLifecycle, await_with_lifecycle};

use super::HeaderOperation;

/// Value-free response format exposed to dynamic header policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DynamicResponseFormat {
    /// Plain text response.
    Text,
    /// JSON object response.
    JsonObject,
    /// JSON schema response.
    JsonSchema,
}

impl From<ResponseFormatKind> for DynamicResponseFormat {
    fn from(value: ResponseFormatKind) -> Self {
        match value {
            ResponseFormatKind::Text => Self::Text,
            ResponseFormatKind::JsonObject => Self::JsonObject,
            ResponseFormatKind::JsonSchema => Self::JsonSchema,
        }
    }
}

/// Value-free context available to a dynamic header callback.
#[derive(Clone, Debug)]
pub struct DynamicHeaderContext {
    provider_id: ProviderId,
    product_id: ProductId,
    model_id: ModelId,
    protocol_id: ProtocolId,
    local_request_id: LocalRequestId,
    attempt_number: u32,
    contains_tools: bool,
    contains_images: bool,
    reasoning_enabled: bool,
    response_format: DynamicResponseFormat,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl DynamicHeaderContext {
    /// Creates a callback context from value-free request facts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_attempt(
        provider_id: ProviderId,
        product_id: ProductId,
        model_id: ModelId,
        protocol_id: ProtocolId,
        local_request_id: LocalRequestId,
        attempt_number: u32,
        contains_tools: bool,
        contains_images: bool,
        reasoning_enabled: bool,
        response_format: DynamicResponseFormat,
        lifecycle: &RequestLifecycle,
    ) -> Self {
        Self {
            provider_id,
            product_id,
            model_id,
            protocol_id,
            local_request_id,
            attempt_number,
            contains_tools,
            contains_images,
            reasoning_enabled,
            response_format,
            deadline: lifecycle.deadline(),
            cancellation: lifecycle.cancellation().clone(),
        }
    }

    /// Returns provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }
    /// Returns product identifier.
    pub fn product_id(&self) -> &ProductId {
        &self.product_id
    }
    /// Returns domain model identifier.
    pub fn model_id(&self) -> &ModelId {
        &self.model_id
    }
    /// Returns protocol identifier.
    pub fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }
    /// Returns local request correlation identifier.
    pub fn local_request_id(&self) -> &LocalRequestId {
        &self.local_request_id
    }
    /// Returns retry attempt number.
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    /// Returns whether the request contains tools.
    pub fn contains_tools(&self) -> bool {
        self.contains_tools
    }
    /// Returns whether the request contains images.
    pub fn contains_images(&self) -> bool {
        self.contains_images
    }
    /// Returns whether reasoning is enabled.
    pub fn reasoning_enabled(&self) -> bool {
        self.reasoning_enabled
    }
    /// Returns response format classification.
    pub fn response_format(&self) -> DynamicResponseFormat {
        self.response_format
    }
    /// Returns the absolute request deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    /// Returns the request cancellation handle.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Future returned by a dynamic header callback.
pub type DynamicHeaderFuture =
    Pin<Box<dyn Future<Output = Result<Vec<HeaderOperation>, HeaderPolicyError>> + Send + 'static>>;

/// External source for controlled, non-sensitive header operations.
pub trait DynamicHeaderSource: fmt::Debug + Send + Sync {
    /// Computes operations from value-free request context.
    fn resolve(&self, context: DynamicHeaderContext) -> DynamicHeaderFuture;
}

/// Dynamic header policy with explicit allowlist and resource budgets.
#[derive(Clone)]
pub struct DynamicHeaderPolicy {
    source: Arc<dyn DynamicHeaderSource>,
    allowed_headers: Arc<[HeaderName]>,
    timeout: Duration,
    max_operations: usize,
    max_bytes: usize,
}

impl DynamicHeaderPolicy {
    /// Creates a policy for a finite set of ordinary extension headers.
    pub fn new(
        source: Arc<dyn DynamicHeaderSource>,
        allowed_headers: Vec<HeaderName>,
    ) -> Result<Self, LlmError> {
        if allowed_headers.is_empty() || allowed_headers.len() > 32 {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::InvalidOperation).into());
        }
        if allowed_headers.iter().any(is_protected) {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::InvalidOperation).into());
        }
        Ok(Self {
            source,
            allowed_headers: allowed_headers.into(),
            timeout: Duration::from_secs(1),
            max_operations: 16,
            max_bytes: 4096,
        })
    }

    /// Returns the finite value-free header-name allowlist.
    #[must_use]
    pub fn allowed_headers(&self) -> &[HeaderName] {
        &self.allowed_headers
    }

    /// Sets the callback timeout. Zero is rejected.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, LlmError> {
        if timeout.is_zero() {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::Timeout).into());
        }
        self.timeout = timeout;
        Ok(self)
    }

    /// Sets operation and aggregate byte budgets.
    pub fn with_budget(
        mut self,
        max_operations: usize,
        max_bytes: usize,
    ) -> Result<Self, LlmError> {
        if max_operations == 0 || max_operations > 64 || max_bytes == 0 || max_bytes > 64 * 1024 {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::BudgetExceeded).into());
        }
        self.max_operations = max_operations;
        self.max_bytes = max_bytes;
        Ok(self)
    }

    /// Resolves and validates one attempt's dynamic operations.
    pub async fn resolve(
        &self,
        context: DynamicHeaderContext,
        lifecycle: &RequestLifecycle,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        let source = Arc::clone(&self.source);
        let callback = catch_unwind(AssertUnwindSafe(|| source.resolve(context)))
            .map_err(|_| HeaderPolicyError::new(HeaderPolicyFailure::Callback))?;
        let result = await_with_lifecycle(
            lifecycle,
            tokio::time::timeout(self.timeout, AssertUnwindSafe(callback).catch_unwind()),
        )
        .await?;
        let operations = result
            .map_err(|_| HeaderPolicyError::new(HeaderPolicyFailure::Timeout))?
            .map_err(|_| HeaderPolicyError::new(HeaderPolicyFailure::Callback))?
            .map_err(|_| HeaderPolicyError::new(HeaderPolicyFailure::Callback))?;
        if operations.len() > self.max_operations {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::BudgetExceeded).into());
        }
        let mut bytes = 0usize;
        for operation in &operations {
            match operation {
                HeaderOperation::Set { name, value } => {
                    if !self.allowed_headers.iter().any(|allowed| allowed == name)
                        || value.is_sensitive()
                        || is_protected(name)
                    {
                        return Err(
                            HeaderPolicyError::new(HeaderPolicyFailure::InvalidOperation).into(),
                        );
                    }
                    bytes =
                        bytes.saturating_add(name.as_str().len() + value.value().as_bytes().len());
                }
                HeaderOperation::Remove { name } => {
                    if !self.allowed_headers.iter().any(|allowed| allowed == name)
                        || is_protected(name)
                    {
                        return Err(
                            HeaderPolicyError::new(HeaderPolicyFailure::InvalidOperation).into(),
                        );
                    }
                    bytes = bytes.saturating_add(name.as_str().len());
                }
            }
        }
        if bytes > self.max_bytes {
            return Err(HeaderPolicyError::new(HeaderPolicyFailure::BudgetExceeded).into());
        }
        Ok(operations)
    }
}

impl fmt::Debug for DynamicHeaderPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicHeaderPolicy")
            .field("source", &"<dynamic header source>")
            .field("allowed_headers", &self.allowed_headers)
            .field("timeout", &self.timeout)
            .field("max_operations", &self.max_operations)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

fn is_protected(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "host"
            | "content-length"
            | "content-type"
            | "accept"
            | "transfer-encoding"
            | "connection"
            | "cookie"
            | "set-cookie"
            | "user-agent"
    )
}
