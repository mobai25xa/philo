//! Provider-independent content blocks.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::Bytes;
use url::Url;

use super::{GenerationId, ModelId, ProtocolId, ProviderId, ResourceLimits, ToolCall};
use crate::error::{ValidationError, ValidationReason};

/// Provider-independent content part preserving generation order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentPart {
    /// Text content preserved exactly as supplied.
    Text {
        /// Unmodified UTF-8 text.
        text: String,
    },
    /// Image input with a validated source and detail intent.
    Image(ImageContent),
    /// Visible and optional opaque reasoning content.
    Thinking(ThinkingContent),
    /// A model refusal kept separate from ordinary text.
    Refusal(RefusalContent),
    /// A completed tool call.
    ToolCall(ToolCall),
}

impl ContentPart {
    /// Creates a text part while preserving the text exactly.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Returns text for a text part.
    ///
    /// # Panics
    ///
    /// Panics when called for a non-text part. Use [`Self::text_value`] when the
    /// content kind is not already known.
    pub fn as_text(&self) -> &str {
        self.text_value()
            .expect("ContentPart::as_text requires ContentPart::Text")
    }

    /// Returns text when this is a text part.
    pub fn text_value(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image(_) | Self::Thinking(_) | Self::Refusal(_) | Self::ToolCall(_) => None,
        }
    }
}

/// Image content prepared for later protocol validation and encoding.
#[derive(Clone, Eq, PartialEq)]
pub struct ImageContent {
    source: ImageSource,
    detail: ImageDetail,
}

impl ImageContent {
    /// Creates image content from an HTTPS URL after deterministic preflight.
    pub fn from_url(url: Url, detail: ImageDetail) -> Result<Self, ValidationError> {
        validate_https_image_url(&url)?;
        Ok(Self {
            source: ImageSource::Url(url),
            detail,
        })
    }

    /// Creates image content from an HTTPS URL string after deterministic preflight.
    pub fn parse_url(url: &str, detail: ImageDetail) -> Result<Self, ValidationError> {
        let parsed = Url::parse(url).map_err(|_| {
            ValidationError::new(
                "image.url",
                ValidationReason::InvalidIdentifier,
                "image URL is not a valid absolute URL",
            )
        })?;
        Self::from_url(parsed, detail)
    }

    /// Creates image content from inline bytes after MIME and magic validation.
    pub fn from_inline(
        mime: ImageMime,
        bytes: Bytes,
        detail: ImageDetail,
    ) -> Result<Self, ValidationError> {
        validate_inline_image(mime, &bytes, ResourceLimits::official())?;
        Ok(Self {
            source: ImageSource::Inline { mime, bytes },
            detail,
        })
    }

    /// Creates image content from a validated image data URL.
    pub fn from_data_url(
        data_url: impl Into<String>,
        detail: ImageDetail,
    ) -> Result<Self, ValidationError> {
        let data_url = data_url.into();
        let (mime, payload) = parse_image_data_url(&data_url)?;
        validate_inline_image(mime, &payload, ResourceLimits::official())?;
        Ok(Self {
            source: ImageSource::DataUrl(data_url),
            detail,
        })
    }

    /// Returns the image source.
    pub fn source(&self) -> &ImageSource {
        &self.source
    }

    /// Returns the requested image detail.
    pub fn detail(&self) -> ImageDetail {
        self.detail
    }
}

impl fmt::Debug for ImageContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageContent")
            .field("source", &self.source)
            .field("detail", &self.detail)
            .finish()
    }
}

/// Supported image source representations.
#[derive(Clone, Eq, PartialEq)]
pub enum ImageSource {
    /// Provider-fetched HTTPS URL.
    Url(Url),
    /// Inline validated image bytes.
    Inline {
        /// Declared image format.
        mime: ImageMime,
        /// Original binary payload.
        bytes: Bytes,
    },
    /// Validated image data URL.
    DataUrl(String),
}

impl fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => f
                .debug_struct("Url")
                .field("scheme", &url.scheme())
                .field("host", &url.host_str())
                .field("path_bytes", &url.path().len())
                .field("has_query", &url.query().is_some())
                .finish(),
            Self::Inline { mime, bytes } => f
                .debug_struct("Inline")
                .field("mime", mime)
                .field("bytes", &bytes.len())
                .finish(),
            Self::DataUrl(value) => f
                .debug_struct("DataUrl")
                .field("bytes", &value.len())
                .finish_non_exhaustive(),
        }
    }
}

/// Image MIME types supported by the phase-two contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMime {
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// WebP image.
    Webp,
    /// GIF image.
    Gif,
}

impl ImageMime {
    /// Returns the official `image/*` media type string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }

    fn from_media_type(value: &str) -> Option<Self> {
        match value {
            "image/png" => Some(Self::Png),
            "image/jpeg" => Some(Self::Jpeg),
            "image/webp" => Some(Self::Webp),
            "image/gif" => Some(Self::Gif),
            _ => None,
        }
    }
}

/// Provider image-detail intent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageDetail {
    /// Let the provider choose.
    #[default]
    Auto,
    /// Low-detail processing.
    Low,
    /// High-detail processing.
    High,
    /// Original-detail processing when supported by an exact model profile.
    Original,
}

/// Source identity required before opaque reasoning can be considered for replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    provider: ProviderId,
    model: ModelId,
    protocol: ProtocolId,
    generation_id: Option<GenerationId>,
}

impl SourceIdentity {
    /// Creates a source identity.
    pub fn new(provider: ProviderId, model: ModelId, protocol: ProtocolId) -> Self {
        Self {
            provider,
            model,
            protocol,
            generation_id: None,
        }
    }

    /// Attaches the source generation identifier.
    #[must_use]
    pub fn with_generation_id(mut self, generation_id: GenerationId) -> Self {
        self.generation_id = Some(generation_id);
        self
    }

    /// Returns the source provider.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Returns the source model.
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// Returns the source protocol.
    pub fn protocol(&self) -> &ProtocolId {
        &self.protocol
    }

    /// Returns the source generation identifier.
    pub fn generation_id(&self) -> Option<&GenerationId> {
        self.generation_id.as_ref()
    }

    /// Reports whether this identity shares provider, model, and protocol with `other`.
    pub fn matches_source(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.model == other.model
            && self.protocol == other.protocol
    }
}

/// Provider state that must never be interpreted or logged as text.
#[derive(Clone, Eq, PartialEq)]
pub struct OpaqueReasoning {
    bytes: Bytes,
    source: SourceIdentity,
    redacted: bool,
}

impl OpaqueReasoning {
    /// Creates opaque reasoning data with an explicit source identity.
    pub fn new(bytes: Bytes, source: SourceIdentity, redacted: bool) -> Self {
        Self {
            bytes,
            source,
            redacted,
        }
    }

    /// Returns opaque bytes for an explicit replay-policy decision.
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Returns the source identity.
    pub fn source(&self) -> &SourceIdentity {
        &self.source
    }

    /// Reports whether the provider marked this data as redacted.
    pub fn is_redacted(&self) -> bool {
        self.redacted
    }
}

impl fmt::Debug for OpaqueReasoning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueReasoning")
            .field("present", &true)
            .field("bytes", &self.bytes.len())
            .field("source", &self.source)
            .field("redacted", &self.redacted)
            .finish()
    }
}

/// Visible thinking text and optional opaque provider state.
#[derive(Clone, Eq, PartialEq)]
pub struct ThinkingContent {
    text: String,
    opaque: Option<OpaqueReasoning>,
}

impl ThinkingContent {
    /// Creates visible thinking without opaque provider state.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            opaque: None,
        }
    }

    /// Attaches opaque provider state.
    #[must_use]
    pub fn with_opaque(mut self, opaque: OpaqueReasoning) -> Self {
        self.opaque = Some(opaque);
        self
    }

    /// Returns visible thinking text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns opaque provider state.
    pub fn opaque(&self) -> Option<&OpaqueReasoning> {
        self.opaque.as_ref()
    }
}

impl fmt::Debug for ThinkingContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThinkingContent")
            .field("text_bytes", &self.text.len())
            .field("opaque", &self.opaque)
            .finish_non_exhaustive()
    }
}

/// A model refusal kept distinct from normal assistant text.
#[derive(Clone, Eq, PartialEq)]
pub struct RefusalContent {
    text: String,
}

impl RefusalContent {
    /// Creates refusal content.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Returns refusal text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for RefusalContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefusalContent")
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

fn validate_https_image_url(url: &Url) -> Result<(), ValidationError> {
    if url.scheme() != "https" {
        return Err(ValidationError::new(
            "image.url",
            ValidationReason::InvalidIdentifier,
            "image URL must use the https scheme",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ValidationError::new(
            "image.url",
            ValidationReason::InvalidIdentifier,
            "image URL must not contain embedded user information",
        ));
    }
    let encoded = url.as_str();
    if encoded.len() > ResourceLimits::official().max_image_url_bytes {
        return Err(ValidationError::new(
            "image.url",
            ValidationReason::OutOfRange,
            "image URL exceeds the frozen UTF-8 byte limit",
        ));
    }
    Ok(())
}

fn validate_inline_image(
    mime: ImageMime,
    bytes: &[u8],
    limits: ResourceLimits,
) -> Result<(), ValidationError> {
    if bytes.is_empty() {
        return Err(ValidationError::new(
            "image.bytes",
            ValidationReason::Empty,
            "inline image payload must be non-empty",
        ));
    }
    if bytes.len() > limits.max_inline_image_bytes {
        return Err(ValidationError::new(
            "image.bytes",
            ValidationReason::OutOfRange,
            "inline image exceeds the frozen byte limit",
        ));
    }
    if !matches_magic_bytes(mime, bytes) {
        return Err(ValidationError::new(
            "image.bytes",
            ValidationReason::InvalidIdentifier,
            "inline image magic bytes do not match the declared MIME type",
        ));
    }
    Ok(())
}

fn parse_image_data_url(data_url: &str) -> Result<(ImageMime, Bytes), ValidationError> {
    if data_url.contains('\n') || data_url.contains('\r') {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must not contain line breaks",
        ));
    }
    let Some(rest) = data_url.strip_prefix("data:") else {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must start with data:",
        ));
    };
    let Some((header, payload)) = rest.split_once(',') else {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must contain a base64 payload separator",
        ));
    };
    if header.starts_with(' ')
        || header.ends_with(' ')
        || payload.starts_with(' ')
        || header.contains(' ')
    {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must not contain whitespace around the header or comma",
        ));
    }
    let Some((media_type, encoding)) = header.split_once(';') else {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must declare a base64 encoding",
        ));
    };
    if encoding != "base64" {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL must use base64 encoding",
        ));
    }
    let mime = ImageMime::from_media_type(media_type).ok_or_else(|| {
        ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL MIME type is not supported",
        )
    })?;
    if payload.is_empty() {
        return Err(ValidationError::new(
            "image.data_url",
            ValidationReason::Empty,
            "image data URL payload must be non-empty",
        ));
    }
    let decoded = BASE64_STANDARD.decode(payload.as_bytes()).map_err(|_| {
        ValidationError::new(
            "image.data_url",
            ValidationReason::InvalidIdentifier,
            "image data URL payload is not valid base64",
        )
    })?;
    Ok((mime, Bytes::from(decoded)))
}

fn matches_magic_bytes(mime: ImageMime, bytes: &[u8]) -> bool {
    match mime {
        ImageMime::Png => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        ImageMime::Jpeg => {
            bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
        }
        ImageMime::Webp => bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        ImageMime::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
    }
}

/// Encodes inline bytes into a frozen image data URL at the wire boundary.
pub(crate) fn encode_inline_data_url(mime: ImageMime, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime.as_str(),
        BASE64_STANDARD.encode(bytes)
    )
}

/// Returns MIME and decoded payload for a Domain data URL already validated at construction.
pub(crate) fn decode_validated_data_url(
    data_url: &str,
) -> Result<(ImageMime, Bytes), ValidationError> {
    parse_image_data_url(data_url)
}
