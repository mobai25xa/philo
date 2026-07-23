use std::collections::BTreeMap;

use super::super::{Message, ToolCallId};

/// Stable diagnostic codes for lossy history transformations.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// Developer role converted to system (reserved for later profiles).
    ConvertedDeveloperToSystem,
    /// Opaque thinking was dropped by replay policy.
    DroppedThinkingOpaque,
    /// A tool-call id was sanitized for the target wire format.
    SanitizedToolCallId,
    /// An empty assistant message was removed.
    RemovedEmptyAssistant,
    /// A missing tool result was synthesized (reserved for later profiles).
    SynthesizedMissingToolResult,
    /// An unsupported image was dropped (reserved for later profiles).
    DroppedUnsupportedImage,
    /// Adjacent same-role messages were merged (reserved for later profiles).
    MergedAdjacentMessages,
}

/// Records an old→new tool-call id mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdMapping {
    original: ToolCallId,
    normalized: ToolCallId,
}

impl IdMapping {
    /// Creates a mapping entry.
    pub fn new(original: ToolCallId, normalized: ToolCallId) -> Self {
        Self {
            original,
            normalized,
        }
    }

    /// Returns the original domain id.
    pub fn original(&self) -> &ToolCallId {
        &self.original
    }

    /// Returns the normalized id.
    pub fn normalized(&self) -> &ToolCallId {
        &self.normalized
    }
}

/// Counts of one lossy transformation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NormalizationDiagnostic {
    code: DiagnosticCode,
    count: u32,
}

impl NormalizationDiagnostic {
    /// Creates a diagnostic count entry.
    pub fn new(code: DiagnosticCode, count: u32) -> Self {
        Self { code, count }
    }

    /// Returns the diagnostic code.
    pub fn code(self) -> DiagnosticCode {
        self.code
    }

    /// Returns how many times the code was observed.
    pub fn count(self) -> u32 {
        self.count
    }
}

/// Output of a successful normalization pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedContext {
    messages: Vec<Message>,
    id_mappings: Vec<IdMapping>,
    diagnostics: Vec<NormalizationDiagnostic>,
}

impl NormalizedContext {
    pub(super) fn from_parts(
        messages: Vec<Message>,
        id_mappings: Vec<IdMapping>,
        diagnostics: Vec<NormalizationDiagnostic>,
    ) -> Self {
        Self {
            messages,
            id_mappings,
            diagnostics,
        }
    }

    /// Returns normalized messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Returns tool-call id mappings produced during normalization.
    pub fn id_mappings(&self) -> &[IdMapping] {
        &self.id_mappings
    }

    /// Returns aggregated diagnostics without message bodies.
    pub fn diagnostics(&self) -> &[NormalizationDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Default)]
pub(super) struct DiagnosticCounter {
    counts: BTreeMap<DiagnosticCode, u32>,
}

impl DiagnosticCounter {
    pub(super) fn increment(&mut self, code: DiagnosticCode) {
        *self.counts.entry(code).or_insert(0) += 1;
    }

    pub(super) fn into_vec(self) -> Vec<NormalizationDiagnostic> {
        self.counts
            .into_iter()
            .map(|(code, count)| NormalizationDiagnostic::new(code, count))
            .collect()
    }
}
