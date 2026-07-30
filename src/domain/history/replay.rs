use super::super::{OpaqueReasoning, SourceIdentity, ThinkingContent};
use super::diagnostics::{DiagnosticCode, NormalizationDiagnostic};
use super::policy::ThinkingReplayPolicy;

/// Intentional helper so call sites can drop opaque reasoning explicitly.
pub fn drop_opaque_reasoning(opaque: &OpaqueReasoning) -> NormalizationDiagnostic {
    let _ = opaque;
    NormalizationDiagnostic::new(DiagnosticCode::DroppedThinkingOpaque, 1)
}

/// Pure thinking replay helper for official and synthetic dialect boundaries.
///
/// Official `OpenAI` history always uses [`ThinkingReplayPolicy::DropAll`].
/// [`ThinkingReplayPolicy::SameSourceOnly`] is retained as a pure domain helper
/// for cross-dialect fixtures and never mutates the input slice.
pub fn apply_thinking_replay_policy(
    thinking: &ThinkingContent,
    policy: ThinkingReplayPolicy,
    target: Option<&SourceIdentity>,
) -> (Option<ThinkingContent>, Vec<NormalizationDiagnostic>) {
    match policy {
        ThinkingReplayPolicy::DropAll => {
            let mut diagnostics = Vec::new();
            if thinking.opaque().is_some() {
                diagnostics.push(NormalizationDiagnostic::new(
                    DiagnosticCode::DroppedThinkingOpaque,
                    1,
                ));
            }
            (None, diagnostics)
        }
        ThinkingReplayPolicy::SameSourceOnly => {
            let Some(opaque) = thinking.opaque() else {
                return (Some(thinking.clone()), Vec::new());
            };
            let Some(target) = target else {
                return (
                    Some(ThinkingContent::new(thinking.text())),
                    vec![NormalizationDiagnostic::new(
                        DiagnosticCode::DroppedThinkingOpaque,
                        1,
                    )],
                );
            };
            if opaque.source().matches_source(target) {
                (Some(thinking.clone()), Vec::new())
            } else {
                (
                    Some(ThinkingContent::new(thinking.text())),
                    vec![NormalizationDiagnostic::new(
                        DiagnosticCode::DroppedThinkingOpaque,
                        1,
                    )],
                )
            }
        }
    }
}
