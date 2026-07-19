//! Provider-independent content blocks.
#![allow(clippy::must_use_candidate)]

use std::fmt;

use bytes::Bytes;
use url::Url;

use super::{GenerationId, ModelId, ProtocolId, ProviderId, ToolCall};

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
    /// Returns the image source.
    pub fn source(&self) -> &ImageSource {
        &self.source
    }

    /// Returns the requested image detail.
    pub fn detail(&self) -> ImageDetail {
        self.detail
    }

    #[allow(dead_code)]
    pub(crate) fn from_validated(source: ImageSource, detail: ImageDetail) -> Self {
        Self { source, detail }
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
    /// Provider-fetched HTTPS URL. P2-009 owns public validated construction.
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
