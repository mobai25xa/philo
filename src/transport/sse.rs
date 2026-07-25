//! Incremental, protocol-neutral Server-Sent Events framing.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;

use super::ByteStream;
use crate::error::{ErrorStage, LlmError, ProtocolError, ValidationError, ValidationReason};

const DEFAULT_MAX_CHUNK_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_BYTES_PER_POLL: usize = 64 * 1024;
const DEFAULT_MAX_CHUNKS_PER_POLL: usize = 16;
const DEFAULT_MAX_EVENTS_PER_POLL: usize = 32;

/// Resource limits applied while decoding one SSE event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct SseConfig {
    max_event_bytes: usize,
    max_line_bytes: usize,
    max_fields_per_event: Option<usize>,
    max_chunk_bytes: usize,
    max_bytes_per_poll: usize,
    max_chunks_per_poll: usize,
    max_events_per_poll: usize,
}

impl SseConfig {
    /// Creates limits for event and line byte lengths.
    pub fn new(max_event_bytes: usize, max_line_bytes: usize) -> Result<Self, ValidationError> {
        if max_event_bytes == 0 {
            return Err(ValidationError::new(
                "max_event_bytes",
                ValidationReason::Zero,
                "SSE event byte limit must be positive",
            ));
        }
        if max_line_bytes == 0 {
            return Err(ValidationError::new(
                "max_line_bytes",
                ValidationReason::Zero,
                "SSE line byte limit must be positive",
            ));
        }
        Ok(Self {
            max_event_bytes,
            max_line_bytes,
            max_fields_per_event: Some(128),
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_bytes_per_poll: DEFAULT_MAX_BYTES_PER_POLL,
            max_chunks_per_poll: DEFAULT_MAX_CHUNKS_PER_POLL,
            max_events_per_poll: DEFAULT_MAX_EVENTS_PER_POLL,
        })
    }

    /// Sets an optional maximum number of non-comment fields per event.
    #[must_use]
    pub fn with_max_fields_per_event(mut self, limit: Option<usize>) -> Self {
        self.max_fields_per_event = limit;
        self
    }

    /// Sets the maximum retained upstream body chunk.
    pub fn with_max_chunk_bytes(mut self, limit: usize) -> Result<Self, ValidationError> {
        validate_positive_limit("max_chunk_bytes", limit)?;
        self.max_chunk_bytes = limit;
        Ok(self)
    }

    /// Sets cooperative byte, chunk, and decoded-event work budgets for one poll.
    pub fn with_poll_budget(
        mut self,
        max_bytes: usize,
        max_chunks: usize,
        max_events: usize,
    ) -> Result<Self, ValidationError> {
        validate_positive_limit("max_bytes_per_poll", max_bytes)?;
        validate_positive_limit("max_chunks_per_poll", max_chunks)?;
        validate_positive_limit("max_events_per_poll", max_events)?;
        self.max_bytes_per_poll = max_bytes;
        self.max_chunks_per_poll = max_chunks;
        self.max_events_per_poll = max_events;
        Ok(self)
    }

    /// Returns the maximum raw bytes accepted for one event.
    pub fn max_event_bytes(self) -> usize {
        self.max_event_bytes
    }

    /// Returns the maximum bytes accepted for one line, excluding its terminator.
    pub fn max_line_bytes(self) -> usize {
        self.max_line_bytes
    }

    /// Returns the optional field-count limit.
    pub fn max_fields_per_event(self) -> Option<usize> {
        self.max_fields_per_event
    }

    /// Returns the maximum retained upstream body chunk.
    pub fn max_chunk_bytes(self) -> usize {
        self.max_chunk_bytes
    }

    /// Returns the maximum bytes processed during one decoder poll.
    pub fn max_bytes_per_poll(self) -> usize {
        self.max_bytes_per_poll
    }

    /// Returns the maximum upstream chunks accepted during one decoder poll.
    pub fn max_chunks_per_poll(self) -> usize {
        self.max_chunks_per_poll
    }

    /// Returns the maximum SSE events consumed by one protocol-stream poll.
    pub fn max_events_per_poll(self) -> usize {
        self.max_events_per_poll
    }
}

fn validate_positive_limit(field: &'static str, value: usize) -> Result<(), ValidationError> {
    if value == 0 {
        return Err(ValidationError::new(
            field,
            ValidationReason::Zero,
            "stream work and buffer limits must be positive",
        ));
    }
    Ok(())
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            max_event_bytes: 1024 * 1024,
            max_line_bytes: 64 * 1024,
            max_fields_per_event: Some(128),
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_bytes_per_poll: DEFAULT_MAX_BYTES_PER_POLL,
            max_chunks_per_poll: DEFAULT_MAX_CHUNKS_PER_POLL,
            max_events_per_poll: DEFAULT_MAX_EVENTS_PER_POLL,
        }
    }
}

/// One complete SSE event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseEvent {
    data: String,
    event_type: Option<String>,
    id: Option<String>,
    retry_millis: Option<u64>,
}

impl SseEvent {
    /// Returns data lines joined with a single newline.
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Returns the optional SSE `event` field.
    pub fn event_type(&self) -> Option<&str> {
        self.event_type.as_deref()
    }

    /// Returns the optional SSE `id` field.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Returns a valid decimal SSE `retry` field in milliseconds.
    pub fn retry_millis(&self) -> Option<u64> {
        self.retry_millis
    }
}

/// The resource whose configured SSE limit was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SseLimit {
    /// Bytes retained from one upstream body chunk.
    ChunkBytes,
    /// Raw bytes belonging to one event.
    EventBytes,
    /// Bytes belonging to one line.
    LineBytes,
    /// Non-comment fields belonging to one event.
    Fields,
}

/// A controlled SSE framing or upstream stream failure.
#[derive(Debug)]
pub enum SseError {
    /// The byte stream failed before framing completed.
    Upstream(LlmError),
    /// A complete SSE line was not valid UTF-8.
    InvalidUtf8 {
        /// One-based line number.
        line: u64,
    },
    /// A configured resource limit was exceeded.
    LimitExceeded {
        /// Resource that exceeded its limit.
        resource: SseLimit,
        /// Configured maximum.
        limit: usize,
        /// Smallest observed value known to exceed the limit.
        observed: usize,
        /// One-based current line number.
        line: u64,
    },
}

impl SseError {
    /// Converts framing failures to the public protocol taxonomy while preserving upstream errors.
    pub fn into_llm_error(self) -> LlmError {
        match self {
            Self::Upstream(error) => error,
            Self::InvalidUtf8 { line } => ProtocolError::at_stage(
                ErrorStage::Sse,
                format!("invalid UTF-8 on SSE line {line}"),
            )
            .into(),
            Self::LimitExceeded {
                resource,
                limit,
                observed,
                line,
            } => ProtocolError::at_stage(
                ErrorStage::Sse,
                format!(
                    "SSE {resource:?} limit exceeded on line {line}: limit {limit}, observed at least {observed}"
                ),
            )
            .into(),
        }
    }
}

impl fmt::Display for SseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upstream(_) => formatter.write_str("upstream byte stream failed"),
            Self::InvalidUtf8 { line } => write!(formatter, "invalid UTF-8 on SSE line {line}"),
            Self::LimitExceeded {
                resource,
                limit,
                observed,
                line,
            } => write!(
                formatter,
                "SSE {resource:?} limit exceeded on line {line}: limit {limit}, observed at least {observed}"
            ),
        }
    }
}

impl std::error::Error for SseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Upstream(error) => Some(error),
            Self::InvalidUtf8 { .. } | Self::LimitExceeded { .. } => None,
        }
    }
}

/// Backpressure-aware incremental SSE decoder.
///
/// EOF dispatches an unterminated event when at least one `data` field was seen.
/// A comment-only or empty tail ends normally. Chat-level completion markers are
/// deliberately not interpreted here.
pub struct SseDecoder {
    upstream: ByteStream,
    config: SseConfig,
    chunk: Option<Bytes>,
    chunk_offset: usize,
    line: Vec<u8>,
    data_lines: Vec<String>,
    event_type: Option<String>,
    last_event_id: Option<String>,
    retry_millis: Option<u64>,
    has_data: bool,
    saw_cr: bool,
    event_bytes: usize,
    field_count: usize,
    line_number: u64,
    pending: Option<Result<SseEvent, SseError>>,
    terminal: bool,
}

impl fmt::Debug for SseDecoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SseDecoder")
            .field("config", &self.config)
            .field("buffered_line_bytes", &self.line.len())
            .field("event_bytes", &self.event_bytes)
            .field("field_count", &self.field_count)
            .field("line_number", &self.line_number)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl SseDecoder {
    /// Wraps an SDK byte stream using default limits.
    pub fn new(upstream: ByteStream) -> Self {
        Self::with_config(upstream, SseConfig::default())
    }

    /// Wraps an SDK byte stream using explicit resource limits.
    pub fn with_config(upstream: ByteStream, config: SseConfig) -> Self {
        Self {
            upstream,
            config,
            chunk: None,
            chunk_offset: 0,
            line: Vec::new(),
            data_lines: Vec::new(),
            event_type: None,
            last_event_id: None,
            retry_millis: None,
            has_data: false,
            saw_cr: false,
            event_bytes: 0,
            field_count: 0,
            line_number: 0,
            pending: None,
            terminal: false,
        }
    }

    fn fail(&mut self, error: SseError) {
        self.line.clear();
        self.data_lines.clear();
        self.chunk = None;
        self.pending = Some(Err(error));
        self.terminal = true;
    }

    fn process_byte(&mut self, byte: u8) {
        if self.saw_cr {
            self.saw_cr = false;
            if byte == b'\n' {
                return;
            }
        }

        self.event_bytes = self.event_bytes.saturating_add(1);
        if self.event_bytes > self.config.max_event_bytes {
            self.fail(SseError::LimitExceeded {
                resource: SseLimit::EventBytes,
                limit: self.config.max_event_bytes,
                observed: self.event_bytes,
                line: self.line_number.saturating_add(1),
            });
            return;
        }

        match byte {
            b'\r' => {
                self.finish_line();
                self.saw_cr = true;
            }
            b'\n' => self.finish_line(),
            _ => {
                self.line.push(byte);
                if self.line.len() > self.config.max_line_bytes {
                    self.fail(SseError::LimitExceeded {
                        resource: SseLimit::LineBytes,
                        limit: self.config.max_line_bytes,
                        observed: self.line.len(),
                        line: self.line_number.saturating_add(1),
                    });
                }
            }
        }
    }

    fn finish_line(&mut self) {
        self.line_number = self.line_number.saturating_add(1);
        let bytes = std::mem::take(&mut self.line);
        if bytes.is_empty() {
            self.dispatch_event();
            return;
        }

        let Ok(line) = std::str::from_utf8(&bytes) else {
            self.fail(SseError::InvalidUtf8 {
                line: self.line_number,
            });
            return;
        };
        if line.starts_with(':') {
            return;
        }

        self.field_count = self.field_count.saturating_add(1);
        if let Some(limit) = self.config.max_fields_per_event
            && self.field_count > limit
        {
            self.fail(SseError::LimitExceeded {
                resource: SseLimit::Fields,
                limit,
                observed: self.field_count,
                line: self.line_number,
            });
            return;
        }

        let (field, mut value) = line.split_once(':').unwrap_or((line, ""));
        if let Some(without_space) = value.strip_prefix(' ') {
            value = without_space;
        }
        match field {
            "data" => {
                self.has_data = true;
                self.data_lines.push(value.to_owned());
            }
            "event" => self.event_type = Some(value.to_owned()),
            "id" if !value.contains('\0') => self.last_event_id = Some(value.to_owned()),
            "retry" => {
                if !value.is_empty()
                    && value.bytes().all(|byte| byte.is_ascii_digit())
                    && let Ok(retry) = value.parse()
                {
                    self.retry_millis = Some(retry);
                }
            }
            _ => {}
        }
    }

    fn dispatch_event(&mut self) {
        if self.has_data {
            self.pending = Some(Ok(SseEvent {
                data: self.data_lines.join("\n"),
                event_type: self.event_type.take(),
                id: self.last_event_id.clone(),
                retry_millis: self.retry_millis.take(),
            }));
        }
        self.data_lines.clear();
        self.event_type = None;
        self.retry_millis = None;
        self.has_data = false;
        self.event_bytes = 0;
        self.field_count = 0;
    }

    fn finish_eof(&mut self) {
        if !self.line.is_empty() {
            self.finish_line();
        }
        if self.pending.is_none() && !self.terminal {
            self.dispatch_event();
        }
        self.terminal = true;
    }
}

impl Stream for SseDecoder {
    type Item = Result<SseEvent, SseError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let decoder = self.get_mut();
        if let Some(item) = decoder.pending.take() {
            return Poll::Ready(Some(item));
        }
        if decoder.terminal {
            return Poll::Ready(None);
        }

        let mut bytes_processed = 0;
        let mut chunks_polled = 0;
        loop {
            if let Some(chunk) = decoder.chunk.as_ref() {
                if decoder.chunk_offset < chunk.len() {
                    if bytes_processed >= decoder.config.max_bytes_per_poll {
                        context.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    let byte = chunk[decoder.chunk_offset];
                    decoder.chunk_offset += 1;
                    bytes_processed += 1;
                    decoder.process_byte(byte);
                    if let Some(item) = decoder.pending.take() {
                        return Poll::Ready(Some(item));
                    }
                    if decoder.terminal {
                        return Poll::Ready(None);
                    }
                    continue;
                }
                decoder.chunk = None;
                decoder.chunk_offset = 0;
            }

            if chunks_polled >= decoder.config.max_chunks_per_poll {
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            match decoder.upstream.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => {
                    chunks_polled += 1;
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    chunks_polled += 1;
                    if chunk.len() > decoder.config.max_chunk_bytes {
                        decoder.fail(SseError::LimitExceeded {
                            resource: SseLimit::ChunkBytes,
                            limit: decoder.config.max_chunk_bytes,
                            observed: chunk.len(),
                            line: decoder.line_number.saturating_add(1),
                        });
                        return Poll::Ready(decoder.pending.take());
                    }
                    decoder.chunk = Some(chunk);
                }
                Poll::Ready(Some(Err(error))) => {
                    decoder.fail(SseError::Upstream(error));
                    return Poll::Ready(decoder.pending.take());
                }
                Poll::Ready(None) => {
                    decoder.finish_eof();
                    return Poll::Ready(decoder.pending.take());
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
