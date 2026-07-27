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

use super::schema::{SchemaLimits, ToolSchema};
use super::structured::ResponseFormat;
use super::tools::{
    ParallelToolCalls, ToolChoice, ToolDefinition, ToolLimits, validate_tool_options,
    validate_tool_options_with_limits,
};
use super::{
    ContentPart, ImageDetail, ImageSource, Message, MessageRole, ModelRef, ResourceLimits,
};
use crate::error::{CapabilityError, LlmError, ValidationError, ValidationReason};
use crate::protocol_options::ProtocolOptions;

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

/// Domain reasoning intent that remains separate from wire field names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThinkingRequest {
    /// Explicitly request `none` reasoning effort when supported.
    Disabled,
    /// Request one of the frozen effort values.
    Effort(ReasoningEffort),
    /// Leave the provider default in place by omitting the wire field.
    #[default]
    ProviderDefault,
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
    reasoning: ThinkingRequest,
    response_format: ResponseFormat,
    tools: Vec<ToolDefinition>,
    tool_choice: Option<ToolChoice>,
    parallel_tool_calls: Option<ParallelToolCalls>,
    timeout: Option<RequestTimeout>,
    headers: HeaderMap,
    metadata: RequestMetadata,
    protocol_options: Option<ProtocolOptions>,
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
    /// Sets the domain reasoning intent.
    pub fn with_reasoning(mut self, reasoning: ThinkingRequest) -> Self {
        self.reasoning = reasoning;
        self
    }
    /// Sets the structured response format. Defaults to [`ResponseFormat::Text`].
    pub fn with_response_format(mut self, response_format: ResponseFormat) -> Self {
        self.response_format = response_format;
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
    /// Sets protocol-scoped options. The selected runtime must have the same protocol ID.
    pub fn with_protocol_options(mut self, options: impl Into<ProtocolOptions>) -> Self {
        self.protocol_options = Some(options.into());
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
    /// Returns reasoning intent.
    pub fn reasoning(&self) -> ThinkingRequest {
        self.reasoning
    }
    /// Returns the structured response format.
    pub fn response_format(&self) -> &ResponseFormat {
        &self.response_format
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
    /// Returns protocol-scoped options when set.
    pub fn protocol_options(&self) -> Option<&ProtocolOptions> {
        self.protocol_options.as_ref()
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

/// Provider-independent request ceilings compiled before target-aware validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub(crate) struct RequestValidationLimits {
    pub(crate) max_body_bytes: usize,
    pub(crate) max_messages: usize,
    pub(crate) max_text_bytes: usize,
    pub(crate) max_tools: usize,
    pub(crate) max_tool_description_bytes: usize,
    pub(crate) max_schema_bytes: usize,
    pub(crate) max_schema_depth: usize,
    pub(crate) max_json_array_items: usize,
    pub(crate) max_images: usize,
    pub(crate) max_inline_image_bytes: usize,
    pub(crate) max_image_url_bytes: usize,
}

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
    /// Performs provider-neutral, no-I/O conservative validation.
    ///
    /// This public convenience check cannot replace target-aware planning by
    /// `LlmClient`: exact model capabilities, compatibility policy, history
    /// normalization, and profile-specific resource limits are resolved only
    /// by the crate-private call planner.
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
        let mut has_non_empty_user_text = false;
        let mut image_count = 0usize;
        let mut needs_original_detail = false;
        let limits = ResourceLimits::official();

        for (index, message) in self.messages.iter().enumerate() {
            match message.role() {
                MessageRole::Tool => {
                    let Some(result) = message.tool_result() else {
                        return Err(ValidationError::new(
                            format!("messages[{index}]"),
                            ValidationReason::TextPartCount,
                            "tool role requires a tool result payload",
                        )
                        .into());
                    };
                    let [ContentPart::Text { text }] = result.content() else {
                        return Err(ValidationError::new(
                            format!("messages[{index}].content"),
                            ValidationReason::TextPartCount,
                            "official tool results require exactly one text part",
                        )
                        .into());
                    };
                    if text.is_empty() {
                        return Err(ValidationError::new(
                            format!("messages[{index}].content[0]"),
                            ValidationReason::Empty,
                            "official tool results require non-empty text",
                        )
                        .into());
                    }
                }
                MessageRole::Developer | MessageRole::System => {
                    validate_single_text_message(message, index)?;
                }
                MessageRole::User => {
                    has_user = true;
                    if message.content().is_empty() {
                        return Err(ValidationError::new(
                            format!("messages[{index}].content"),
                            ValidationReason::Empty,
                            "user messages require at least one content part",
                        )
                        .into());
                    }
                    for (part_index, part) in message.content().iter().enumerate() {
                        match part {
                            ContentPart::Text { text } => {
                                if !text.trim().is_empty() {
                                    has_non_empty_user_text = true;
                                }
                            }
                            ContentPart::Image(image) => {
                                image_count = image_count.saturating_add(1);
                                if matches!(image.detail(), ImageDetail::Original) {
                                    needs_original_detail = true;
                                }
                                match image.source() {
                                    ImageSource::Url(url)
                                        if url.as_str().len() > limits.max_image_url_bytes =>
                                    {
                                        return Err(ValidationError::new(
                                            format!("messages[{index}].content[{part_index}]"),
                                            ValidationReason::OutOfRange,
                                            "image URL exceeds the frozen UTF-8 byte limit",
                                        )
                                        .into());
                                    }
                                    ImageSource::Inline { bytes, .. }
                                        if bytes.len() > limits.max_inline_image_bytes =>
                                    {
                                        return Err(ValidationError::new(
                                            format!("messages[{index}].content[{part_index}]"),
                                            ValidationReason::OutOfRange,
                                            "inline image exceeds the frozen byte limit",
                                        )
                                        .into());
                                    }
                                    _ => {}
                                }
                            }
                            ContentPart::Thinking(_)
                            | ContentPart::Refusal(_)
                            | ContentPart::ToolCall(_) => {
                                return Err(ValidationError::new(
                                    format!("messages[{index}].content[{part_index}]"),
                                    ValidationReason::TextPartCount,
                                    "user messages only accept text and image parts",
                                )
                                .into());
                            }
                        }
                    }
                }
                MessageRole::Assistant => {
                    validate_assistant_request_message(message, index)?;
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
        if !has_non_empty_user_text {
            return Err(ValidationError::new(
                "messages",
                ValidationReason::EmptyUserText,
                "at least one user text part must contain non-whitespace",
            )
            .into());
        }
        if image_count > 0 {
            check_capability("messages.image", capabilities.vision_input)?;
        }
        if image_count > limits.max_images {
            return Err(ValidationError::new(
                "messages.image",
                ValidationReason::OutOfRange,
                "image count exceeds the frozen request limit",
            )
            .into());
        }
        if needs_original_detail {
            check_capability("image.detail", capabilities.image_detail_original)?;
        }
        validate_generation_options(&self.options, capabilities)
    }
}

fn validate_generation_options(
    options: &GenerationOptions,
    capabilities: &CapabilitySet,
) -> Result<(), LlmError> {
    if let Some(value) = options.temperature {
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
    if let Some(value) = options.max_output_tokens {
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
    validate_reasoning_request(options.reasoning(), &capabilities.reasoning_efforts)?;
    validate_response_format(options.response_format(), capabilities)?;
    validate_tool_options(
        options.tools(),
        options.tool_choice(),
        options.parallel_tool_calls(),
        capabilities,
    )?;
    if let Some(RequestTimeout::At(deadline)) = options.timeout
        && deadline <= Instant::now()
    {
        return Err(ValidationError::new(
            "deadline",
            ValidationReason::Expired,
            "deadline has elapsed",
        )
        .into());
    }
    for name in options.headers.keys() {
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

/// Validates raw request shape and target-dependent options before history normalization.
pub(crate) fn validate_request_shape(
    request: &GenerateRequest,
    capabilities: &CapabilitySet,
    limits: &RequestValidationLimits,
) -> Result<(), LlmError> {
    if request.model().provider().as_str().is_empty() {
        return Err(ValidationError::new(
            "model.provider",
            ValidationReason::MissingProvider,
            "provider is required",
        )
        .into());
    }
    validate_target_options(request, capabilities)?;
    validate_request_resources(request.messages(), request.options(), limits)
}

/// Validates normalized request content under the fully compiled target policy.
pub(crate) fn validate_planned_request(
    model: &ModelRef,
    messages: &[Message],
    options: &GenerationOptions,
    capabilities: &CapabilitySet,
    limits: &RequestValidationLimits,
) -> Result<(), LlmError> {
    let normalized =
        GenerateRequest::new(model.clone(), messages.to_vec()).with_options(options.clone());
    normalized.validate(capabilities)?;
    validate_request_resources(messages, options, limits)
}

fn validate_target_options(
    request: &GenerateRequest,
    capabilities: &CapabilitySet,
) -> Result<(), LlmError> {
    validate_generation_options(request.options(), capabilities)
}

fn validate_request_resources(
    messages: &[Message],
    options: &GenerationOptions,
    limits: &RequestValidationLimits,
) -> Result<(), LlmError> {
    if messages.len() > limits.max_messages {
        return Err(ValidationError::new(
            "messages",
            ValidationReason::OutOfRange,
            "message count exceeds the resolved request limit",
        )
        .into());
    }

    let mut image_count = 0usize;
    for (message_index, message) in messages.iter().enumerate() {
        for (part_index, part) in message.content().iter().enumerate() {
            if let ContentPart::Image(image) = part {
                image_count = image_count.saturating_add(1);
                match image.source() {
                    ImageSource::Url(url) if url.as_str().len() > limits.max_image_url_bytes => {
                        return Err(ValidationError::new(
                            format!("messages[{message_index}].content[{part_index}]"),
                            ValidationReason::OutOfRange,
                            "image URL exceeds the resolved request limit",
                        )
                        .into());
                    }
                    ImageSource::Inline { bytes, .. }
                        if bytes.len() > limits.max_inline_image_bytes =>
                    {
                        return Err(ValidationError::new(
                            format!("messages[{message_index}].content[{part_index}]"),
                            ValidationReason::OutOfRange,
                            "inline image exceeds the resolved request limit",
                        )
                        .into());
                    }
                    _ => {}
                }
            }
        }
    }
    if image_count > limits.max_images {
        return Err(ValidationError::new(
            "messages.image",
            ValidationReason::OutOfRange,
            "image count exceeds the resolved request limit",
        )
        .into());
    }

    validate_tool_options_with_limits(
        options.tools(),
        options.tool_choice(),
        options.parallel_tool_calls(),
        &CapabilitySet {
            temperature: CapabilityStatus::Supported,
            max_output_tokens: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Supported,
            tool_choice_required: CapabilityStatus::Supported,
            tool_choice_specific: CapabilityStatus::Supported,
            parallel_tool_calls: CapabilityStatus::Supported,
            strict_tools: CapabilityStatus::Supported,
            vision_input: CapabilityStatus::Supported,
            image_detail_original: CapabilityStatus::Supported,
            response_format_json_object: CapabilityStatus::Supported,
            response_format_json_schema: CapabilityStatus::Supported,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
        },
        ToolLimits {
            max_tools: limits.max_tools,
            max_tool_description_bytes: limits.max_tool_description_bytes,
        },
    )?;

    let schema_limits = SchemaLimits {
        max_schema_bytes: limits.max_schema_bytes,
        max_schema_depth: limits.max_schema_depth,
        max_json_array_items: limits.max_json_array_items,
    };
    for tool in options.tools() {
        ToolSchema::with_limits(tool.parameters().value().clone(), schema_limits)?;
    }
    if let ResponseFormat::JsonSchema(structured) = options.response_format() {
        ToolSchema::with_limits(structured.schema().value().clone(), schema_limits)?;
    }
    Ok(())
}

fn validate_single_text_message(message: &Message, index: usize) -> Result<(), LlmError> {
    if message.content().len() != 1 {
        return Err(ValidationError::new(
            format!("messages[{index}].content"),
            ValidationReason::TextPartCount,
            "developer and system messages require exactly one text part",
        )
        .into());
    }
    let ContentPart::Text { text } = &message.content()[0] else {
        return Err(ValidationError::new(
            format!("messages[{index}].content[0]"),
            ValidationReason::TextPartCount,
            "developer and system messages require text content",
        )
        .into());
    };
    if text.is_empty() {
        return Err(ValidationError::new(
            format!("messages[{index}].content[0]"),
            ValidationReason::Empty,
            "developer and system text must be non-empty",
        )
        .into());
    }
    Ok(())
}

fn validate_assistant_request_message(message: &Message, index: usize) -> Result<(), LlmError> {
    if message.content().is_empty() {
        return Err(ValidationError::new(
            format!("messages[{index}].content"),
            ValidationReason::Empty,
            "assistant messages require at least one content part",
        )
        .into());
    }
    let mut seen_tool_call = false;
    for (part_index, part) in message.content().iter().enumerate() {
        match part {
            ContentPart::Text { .. } => {
                if seen_tool_call {
                    return Err(ValidationError::new(
                        format!("messages[{index}].content[{part_index}]"),
                        ValidationReason::TextPartCount,
                        "assistant text after tool calls is not allowed",
                    )
                    .into());
                }
            }
            ContentPart::ToolCall(_) => {
                seen_tool_call = true;
            }
            ContentPart::Thinking(_) => {
                if seen_tool_call {
                    return Err(ValidationError::new(
                        format!("messages[{index}].content[{part_index}]"),
                        ValidationReason::TextPartCount,
                        "assistant thinking after tool calls is not allowed",
                    )
                    .into());
                }
            }
            ContentPart::Image(_) | ContentPart::Refusal(_) => {
                return Err(ValidationError::new(
                    format!("messages[{index}].content[{part_index}]"),
                    ValidationReason::TextPartCount,
                    "assistant request messages do not accept image or refusal content",
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_response_format(
    response_format: &ResponseFormat,
    capabilities: &CapabilitySet,
) -> Result<(), LlmError> {
    match response_format {
        ResponseFormat::Text => Ok(()),
        ResponseFormat::JsonObject => check_capability(
            "response_format.json_object",
            capabilities.response_format_json_object,
        ),
        ResponseFormat::JsonSchema(_) => check_capability(
            "response_format.json_schema",
            capabilities.response_format_json_schema,
        ),
    }
}

fn validate_reasoning_request(
    request: ThinkingRequest,
    support: &ReasoningEffortSupport,
) -> Result<(), LlmError> {
    match request {
        ThinkingRequest::ProviderDefault => Ok(()),
        ThinkingRequest::Disabled => match support {
            ReasoningEffortSupport::Supported(values)
                if values.contains(&ReasoningEffort::None) =>
            {
                Ok(())
            }
            ReasoningEffortSupport::Supported(_) | ReasoningEffortSupport::Unsupported => {
                Err(CapabilityError::new("reasoning", "reasoning_effort", "Unsupported").into())
            }
            ReasoningEffortSupport::Unknown => {
                Err(CapabilityError::new("reasoning", "reasoning_effort", "Unknown").into())
            }
        },
        ThinkingRequest::Effort(effort) => match support {
            ReasoningEffortSupport::Supported(values) if values.contains(&effort) => Ok(()),
            ReasoningEffortSupport::Supported(_) | ReasoningEffortSupport::Unsupported => {
                Err(CapabilityError::new("reasoning", "reasoning_effort", "Unsupported").into())
            }
            ReasoningEffortSupport::Unknown => {
                Err(CapabilityError::new("reasoning", "reasoning_effort", "Unknown").into())
            }
        },
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
    crate::protected::is_protected_header(name)
}
