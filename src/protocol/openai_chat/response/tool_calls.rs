use std::collections::BTreeMap;
use std::fmt;

use super::super::wire::ToolCallDeltaWire;
use super::protocol;
use crate::domain::{
    AssistantEvent, ContentIndex, ToolArguments, ToolCall, ToolCallId, ToolName, WireToolIndex,
};
use crate::error::LlmError;
use crate::provider::call_policy::ResponseLimits;

pub(super) struct ToolCallAccumulator {
    pending_by_wire_index: BTreeMap<WireToolIndex, PendingToolCall>,
    seen_provider_call_ids: BTreeMap<ToolCallId, WireToolIndex>,
    order: Vec<WireToolIndex>,
    total_argument_bytes: usize,
    limits: ResponseLimits,
}

impl ToolCallAccumulator {
    pub(super) fn new(limits: ResponseLimits) -> Self {
        Self {
            pending_by_wire_index: BTreeMap::new(),
            seen_provider_call_ids: BTreeMap::new(),
            order: Vec::new(),
            total_argument_bytes: 0,
            limits,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.order.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Validates capacity and returns whether machine must allocate a new content index.
    pub(super) fn prepare(&self, wire_index: WireToolIndex) -> Result<bool, LlmError> {
        if self.pending_by_wire_index.contains_key(&wire_index) {
            return Ok(false);
        }
        if self.order.len() >= self.limits.max_tool_calls {
            return Err(protocol("tool call count exceeds resource limit"));
        }
        Ok(true)
    }

    pub(super) fn observe_delta(
        &mut self,
        wire_index: WireToolIndex,
        new_content_index: Option<ContentIndex>,
        delta: &ToolCallDeltaWire,
    ) -> Result<Vec<AssistantEvent>, LlmError> {
        let parsed_id = delta.id.as_deref().map(parse_tool_call_id).transpose()?;
        let mut events = Vec::new();

        if let Some(pending) = self.pending_by_wire_index.get(&wire_index) {
            if pending.ended {
                return Err(protocol("tool call delta received after tool call end"));
            }
            if let (Some(existing), Some(parsed)) = (&pending.provider_call_id, &parsed_id)
                && existing != parsed
            {
                return Err(protocol("conflicting tool call id for wire index"));
            }
            if let Some(id) = &parsed_id {
                self.register_id(wire_index, id)?;
                if self
                    .pending_by_wire_index
                    .get(&wire_index)
                    .is_some_and(|pending| pending.provider_call_id.is_none())
                {
                    self.pending_by_wire_index
                        .get_mut(&wire_index)
                        .expect("tool accumulator checked above")
                        .provider_call_id = Some(id.clone());
                }
            }
        } else {
            let domain_content_index = new_content_index
                .ok_or_else(|| protocol("missing content index for new tool call"))?;
            if let Some(id) = &parsed_id {
                self.register_id(wire_index, id)?;
            }
            self.order.push(wire_index);
            self.pending_by_wire_index.insert(
                wire_index,
                PendingToolCall {
                    wire_index,
                    domain_content_index,
                    provider_call_id: parsed_id.clone(),
                    name_buffer: String::new(),
                    arguments_buffer: String::new(),
                    start_emitted: true,
                    ended: false,
                },
            );
            events.push(AssistantEvent::ToolCallStart {
                index: domain_content_index,
                wire_index,
                id: parsed_id,
            });
        }

        self.append_fragments(wire_index, delta, &mut events)?;
        Ok(events)
    }

    fn register_id(&mut self, wire_index: WireToolIndex, id: &ToolCallId) -> Result<(), LlmError> {
        match self.seen_provider_call_ids.get(id) {
            Some(existing) if *existing != wire_index => {
                Err(protocol("duplicate tool call id across wire indexes"))
            }
            Some(_) => Ok(()),
            None => {
                self.seen_provider_call_ids.insert(id.clone(), wire_index);
                Ok(())
            }
        }
    }

    fn append_fragments(
        &mut self,
        wire_index: WireToolIndex,
        delta: &ToolCallDeltaWire,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        let pending = self
            .pending_by_wire_index
            .get_mut(&wire_index)
            .ok_or_else(|| protocol("missing tool call accumulator"))?;
        if pending.ended {
            return Err(protocol("tool call delta received after tool call end"));
        }

        let name_delta = delta
            .function
            .as_ref()
            .and_then(|function| function.name.as_ref())
            .filter(|name| !name.is_empty())
            .cloned();
        let arguments_delta = delta
            .function
            .as_ref()
            .and_then(|function| function.arguments.as_ref())
            .filter(|arguments| !arguments.is_empty())
            .cloned();

        if let Some(delta) = &name_delta {
            if pending.name_buffer.len().saturating_add(delta.len()) > ToolName::MAX_BYTES {
                return Err(protocol("tool name exceeds resource limit"));
            }
            pending.name_buffer.push_str(delta);
        }
        if let Some(delta) = &arguments_delta {
            let next_call = pending.arguments_buffer.len().saturating_add(delta.len());
            if next_call > self.limits.max_tool_arguments_bytes {
                return Err(protocol("tool arguments exceed per-call resource limit"));
            }
            let next_total = self.total_argument_bytes.saturating_add(delta.len());
            if next_total > self.limits.max_all_tool_arguments_bytes {
                return Err(protocol("tool arguments exceed aggregate resource limit"));
            }
            pending.arguments_buffer.push_str(delta);
            self.total_argument_bytes = next_total;
        }

        if name_delta.is_some() || arguments_delta.is_some() {
            events.push(AssistantEvent::ToolCallDelta {
                index: pending.domain_content_index,
                wire_index,
                name_delta,
                arguments_delta,
            });
        }
        Ok(())
    }

    pub(super) fn finish_all(&mut self) -> Result<Vec<AssistantEvent>, LlmError> {
        let mut events = Vec::with_capacity(self.order.len());
        for wire_index in self.order.clone() {
            events.push(self.finish_call(wire_index)?);
        }
        Ok(events)
    }

    fn finish_call(&mut self, wire_index: WireToolIndex) -> Result<AssistantEvent, LlmError> {
        let pending = self
            .pending_by_wire_index
            .get_mut(&wire_index)
            .ok_or_else(|| protocol("missing tool call accumulator"))?;
        if pending.ended {
            return Err(protocol("duplicate tool call end"));
        }
        let id = pending
            .provider_call_id
            .clone()
            .ok_or_else(|| protocol("tool call completed without an id"))?;
        if pending.name_buffer.is_empty() {
            return Err(protocol("tool call completed without a name"));
        }
        let name = ToolName::new(pending.name_buffer.clone())
            .map_err(|_| protocol("tool call completed with an invalid name"))?;
        let arguments = ToolArguments::from_raw_json(pending.arguments_buffer.clone())
            .map_err(|_| protocol("tool call completed with incomplete JSON arguments"))?;
        if json_depth(arguments.value()) > self.limits.max_schema_depth {
            return Err(protocol(
                "tool call arguments exceed maximum JSON nesting depth",
            ));
        }
        let call = ToolCall::new(id, name, arguments);
        let index = pending.domain_content_index;
        pending.ended = true;
        Ok(AssistantEvent::ToolCallEnd { index, call })
    }

    pub(super) fn has_open_calls(&self) -> bool {
        self.pending_by_wire_index.values().any(|tool| !tool.ended)
    }

    pub(super) fn parse_wire_index(raw: i64) -> Result<WireToolIndex, LlmError> {
        if raw < 0 {
            return Err(protocol("tool call index must be non-negative"));
        }
        let value =
            u32::try_from(raw).map_err(|_| protocol("tool call index exceeds u32 range"))?;
        Ok(WireToolIndex::new(value))
    }

    #[cfg(test)]
    pub(super) fn from_pending(
        pending: PendingToolCall,
        limits: ResponseLimits,
        total_argument_bytes: usize,
    ) -> Self {
        let wire_index = pending.wire_index;
        let mut seen_provider_call_ids = BTreeMap::new();
        if let Some(id) = &pending.provider_call_id {
            seen_provider_call_ids.insert(id.clone(), wire_index);
        }
        Self {
            pending_by_wire_index: BTreeMap::from([(wire_index, pending)]),
            seen_provider_call_ids,
            order: vec![wire_index],
            total_argument_bytes,
            limits,
        }
    }
}

pub(super) struct PendingToolCall {
    pub(super) wire_index: WireToolIndex,
    pub(super) domain_content_index: ContentIndex,
    pub(super) provider_call_id: Option<ToolCallId>,
    pub(super) name_buffer: String,
    pub(super) arguments_buffer: String,
    pub(super) start_emitted: bool,
    pub(super) ended: bool,
}

impl fmt::Debug for PendingToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingToolCall")
            .field("wire_index", &self.wire_index.get())
            .field("domain_content_index", &self.domain_content_index.get())
            .field("has_id", &self.provider_call_id.is_some())
            .field("name_bytes", &self.name_buffer.len())
            .field("arguments_bytes", &self.arguments_buffer.len())
            .field("start_emitted", &self.start_emitted)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

fn parse_tool_call_id(raw: &str) -> Result<ToolCallId, LlmError> {
    ToolCallId::new(raw).map_err(|_| protocol("invalid tool call id"))
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}
