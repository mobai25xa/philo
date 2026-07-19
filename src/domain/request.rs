//! Provider-independent generation request and preflight validation.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use http::{HeaderMap, HeaderName};
use tokio::time::Instant;

use super::tools::{ParallelToolCalls, ToolChoice, ToolDefinition, validate_tool_options};
use super::{ContentPart, Message, MessageRole, ModelRef};
use crate::error::{CapabilityError, LlmError, ValidationError, ValidationReason};

/// A three-state capability declaration used for fail-closed validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    /// The selected model explicitly supports the option.
    Supported,
    /// The selected model explicitly rejects the option.
    Unsupported,
    /// The selected model's support is not known.
    Unknown,
}

/// Provider-independent reasoning effort requested from a capable model.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReasoningEffort {
    /// Disable reasoning when the selected model supports this value.
    None,
    /// Minimal reasoning effort.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    XHigh,
    /// Maximum reasoning effort when exposed by the selected model.
    Max,
}

/// Exact set of reasoning effort values supported by a model.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ReasoningEffortSupport {
    /// The model explicitly does not support reasoning effort.
    Unsupported,
    /// Model support has not been declared.
    #[default]
    Unknown,
    /// The model supports exactly the contained values.
    Supported(BTreeSet<ReasoningEffort>),
}

/// Capabilities required by provider-independent generation options and content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySet {
    /// Temperature support.
    pub temperature: CapabilityStatus,
    /// Max-output-token support.
    pub max_output_tokens: CapabilityStatus,
    /// Function tool support.
    pub function_tools: CapabilityStatus,
    /// Required tool-choice support.
    pub tool_choice_required: CapabilityStatus,
    /// Specific function tool-choice support.
    pub tool_choice_specific: CapabilityStatus,
    /// Parallel tool-call support.
    pub parallel_tool_calls: CapabilityStatus,
    /// Strict function-schema support.
    pub strict_tools: CapabilityStatus,
    /// Image input support.
    pub vision_input: CapabilityStatus,
    /// Original image-detail support.
    pub image_detail_original: CapabilityStatus,
    /// JSON object response-format support.
    pub response_format_json_object: CapabilityStatus,
    /// JSON schema response-format support.
    pub response_format_json_schema: CapabilityStatus,
    /// Exact reasoning efforts supported by the selected model.
    pub reasoning_efforts: ReasoningEffortSupport,
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self {
            temperature: CapabilityStatus::Supported,
            max_output_tokens: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
        }
    }
}

/// Local metadata for tracing and diagnostics. It is never serialized by this module.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestMetadata(BTreeMap<String, String>);
impl RequestMetadata {
    /// Creates empty local metadata.
    pub fn new() -> Self {
        Self::default()
    }
    /// Adds a local key/value pair.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Option<String>, ValidationError> {
        let key = key.into();
        if key.is_empty() || key.trim() != key {
            return Err(ValidationError::new(
                "request_metadata",
                ValidationReason::InvalidMetadata,
                "metadata keys must be non-empty and trimmed",
            ));
        }
        Ok(self.0.insert(key, value.into()))
    }
    /// Returns a local value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    /// Iterates local metadata.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// Overall request timeout or absolute deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestTimeout {
    /// A relative, non-zero duration.
    After(Duration),
    /// An absolute Tokio deadline.
    At(Instant),
}
impl RequestTimeout {
    /// Creates a relative timeout, rejecting zero.
    pub fn after(duration: Duration) -> Result<Self, ValidationError> {
        if duration.is_zero() {
            Err(ValidationError::new(
                "timeout",
                ValidationReason::Zero,
                "timeout must be non-zero",
            ))
        } else {
            Ok(Self::After(duration))
        }
    }
    /// Creates a deadline, rejecting an already elapsed instant.
    pub fn at(deadline: Instant) -> Result<Self, ValidationError> {
        if deadline <= Instant::now() {
            Err(ValidationError::new(
                "deadline",
                ValidationReason::Expired,
                "deadline has elapsed",
            ))
        } else {
            Ok(Self::At(deadline))
        }
    }
}

/// Generation options exposed by the domain API.
#[derive(Clone, Debug, Default)]
pub struct GenerationOptions {
    temperature: Option<f64>,
    max_output_tokens: Option<u32>,
    tools: Vec<ToolDefinition>,
    tool_choice: Option<ToolChoice>,
    parallel_tool_calls: Option<ParallelToolCalls>,
    timeout: Option<RequestTimeout>,
    headers: HeaderMap,
    metadata: RequestMetadata,
}
impl GenerationOptions {
    /// Creates empty options.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets temperature.
    pub fn with_temperature(mut self, value: f64) -> Self {
        self.temperature = Some(value);
        self
    }
    /// Sets maximum output tokens.
    pub fn with_max_output_tokens(mut self, value: u32) -> Self {
        self.max_output_tokens = Some(value);
        self
    }
    /// Declares function tools for this request.
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }
    /// Sets the tool selection strategy.
    pub fn with_tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }
    /// Sets whether parallel tool calls may be produced.
    pub fn with_parallel_tool_calls(mut self, value: ParallelToolCalls) -> Self {
        self.parallel_tool_calls = Some(value);
        self
    }
    /// Sets a relative timeout.
    pub fn with_timeout(mut self, value: Duration) -> Result<Self, ValidationError> {
        self.timeout = Some(RequestTimeout::after(value)?);
        Ok(self)
    }
    /// Sets an absolute deadline.
    pub fn with_deadline(mut self, value: Instant) -> Result<Self, ValidationError> {
        self.timeout = Some(RequestTimeout::at(value)?);
        Ok(self)
    }
    /// Adds a request header. Protected headers are rejected during validation.
    pub fn with_header(mut self, name: HeaderName, value: http::HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }
    /// Adds local request metadata.
    pub fn with_metadata(mut self, metadata: RequestMetadata) -> Self {
        self.metadata = metadata;
        self
    }
    /// Returns temperature.
    pub fn temperature(&self) -> Option<f64> {
        self.temperature
    }
    /// Returns max output tokens.
    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }
    /// Returns declared tools.
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }
    /// Returns tool selection strategy when set.
    pub fn tool_choice(&self) -> Option<&ToolChoice> {
        self.tool_choice.as_ref()
    }
    /// Returns parallel tool-call preference when set.
    pub fn parallel_tool_calls(&self) -> Option<ParallelToolCalls> {
        self.parallel_tool_calls
    }
    /// Returns timeout/deadline.
    pub fn timeout(&self) -> Option<RequestTimeout> {
        self.timeout
    }
    /// Returns request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    /// Returns local metadata.
    pub fn metadata(&self) -> &RequestMetadata {
        &self.metadata
    }
}

/// A validated, provider-independent generation request.
#[derive(Clone, Debug)]
pub struct GenerateRequest {
    model: ModelRef,
    messages: Vec<Message>,
    options: GenerationOptions,
}

/// Alias used by higher-level client APIs.
pub type LlmRequest = GenerateRequest;

impl GenerateRequest {
    /// Creates a request. Detailed validation is performed by [`Self::validate`].
    pub fn new(model: ModelRef, messages: Vec<Message>) -> Self {
        Self {
            model,
            messages,
            options: GenerationOptions::default(),
        }
    }
    /// Replaces generation options.
    pub fn with_options(mut self, options: GenerationOptions) -> Self {
        self.options = options;
        self
    }
    /// Returns selected model.
    pub fn model(&self) -> &ModelRef {
        &self.model
    }
    /// Returns messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
    /// Returns generation options.
    pub fn options(&self) -> &GenerationOptions {
        &self.options
    }
    /// Validates all phase-one request rules before a transport can be called.
    pub fn validate(&self, capabilities: &CapabilitySet) -> Result<(), LlmError> {
        if self.model.provider().as_str().is_empty() {
            return Err(ValidationError::new(
                "model.provider",
                ValidationReason::MissingProvider,
                "provider is required",
            )
            .into());
        }
        if self.messages.is_empty() {
            return Err(ValidationError::new(
                "messages",
                ValidationReason::Empty,
                "at least one message is required",
            )
            .into());
        }
        let mut has_user = false;
        for (index, message) in self.messages.iter().enumerate() {
            let count = message.content().len();
            if count != 1 {
                return Err(ValidationError::new(
                    format!("messages[{index}].content"),
                    ValidationReason::TextPartCount,
                    "phase one requires exactly one text part",
                )
                .into());
            }
            let ContentPart::Text { text } = &message.content()[0] else {
                return Err(ValidationError::new(
                    format!("messages[{index}].content[0]"),
                    ValidationReason::TextPartCount,
                    "phase one requires text content",
                )
                .into());
            };
            if message.role() == MessageRole::User {
                has_user = true;
                if text.trim().is_empty() {
                    return Err(ValidationError::new(
                        format!("messages[{index}].content[0]"),
                        ValidationReason::EmptyUserText,
                        "user text must contain non-whitespace",
                    )
                    .into());
                }
            }
        }
        if !has_user {
            return Err(ValidationError::new(
                "messages",
                ValidationReason::MissingUserMessage,
                "at least one user message is required",
            )
            .into());
        }
        if let Some(value) = self.options.temperature {
            if !value.is_finite() {
                return Err(ValidationError::new(
                    "temperature",
                    ValidationReason::NonFinite,
                    "temperature must be finite",
                )
                .into());
            }
            if !(0.0..=2.0).contains(&value) {
                return Err(ValidationError::new(
                    "temperature",
                    ValidationReason::OutOfRange,
                    "temperature must be in 0..=2",
                )
                .into());
            }
            check_capability("temperature", capabilities.temperature)?;
        }
        if let Some(value) = self.options.max_output_tokens {
            if value == 0 {
                return Err(ValidationError::new(
                    "max_output_tokens",
                    ValidationReason::Zero,
                    "max_output_tokens must be positive",
                )
                .into());
            }
            check_capability("max_output_tokens", capabilities.max_output_tokens)?;
        }
        validate_tool_options(
            self.options.tools(),
            self.options.tool_choice(),
            self.options.parallel_tool_calls(),
            capabilities,
        )?;
        if let Some(RequestTimeout::At(deadline)) = self.options.timeout
            && deadline <= Instant::now()
        {
            return Err(ValidationError::new(
                "deadline",
                ValidationReason::Expired,
                "deadline has elapsed",
            )
            .into());
        }
        for name in self.options.headers.keys() {
            if name
                .as_str()
                .bytes()
                .any(|byte| byte == b'\r' || byte == b'\n')
            {
                return Err(ValidationError::new(
                    format!("request_headers.{name}"),
                    ValidationReason::InvalidHeader,
                    "header name contains line break",
                )
                .into());
            }
            if is_protected(name) {
                return Err(ValidationError::new(
                    format!("request_headers.{name}"),
                    ValidationReason::ProtectedHeader,
                    "protected headers are adapter-owned",
                )
                .into());
            }
        }
        Ok(())
    }
}

fn check_capability(field: &str, status: CapabilityStatus) -> Result<(), LlmError> {
    match status {
        CapabilityStatus::Supported => Ok(()),
        CapabilityStatus::Unsupported => {
            Err(CapabilityError::new(field, field, "Unsupported").into())
        }
        CapabilityStatus::Unknown => Err(CapabilityError::new(field, field, "Unknown").into()),
    }
}

fn is_protected(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "content-type"
            | "accept"
            | "host"
            | "user-agent"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "cookie"
    )
}
