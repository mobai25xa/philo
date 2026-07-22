//! JSON Schema response format with optional live complete. Validation happens at Done boundary.

mod support;

use philo::{
    GenerateRequest, GenerationOptions, ModelRef, ResponseFormat, StructuredSchema, ToolSchema,
    collect_assistant_message_for_format,
};
use serde_json::json;
use support::ExampleResult;

fn answer_schema() -> ExampleResult<StructuredSchema> {
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "answer": { "type": "string", "minLength": 1 },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
        },
        "required": ["answer", "confidence"],
        "additionalProperties": false
    }))?;
    Ok(StructuredSchema::new(
        "brief_answer",
        Some("Short answer with confidence".to_owned()),
        schema,
        false,
    )?)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let structured = answer_schema()?;
    let format = ResponseFormat::JsonSchema(structured);

    if !support::has_live_credentials() {
        // Prove the collector helper is the complete-path validator surface.
        use futures_util::stream;
        use philo::{AssistantEvent, ContentIndex, FinishReason, LocalRequestId};

        let events = stream::iter(vec![
            Ok(AssistantEvent::Start {
                local_request_id: LocalRequestId::new("example-local")?,
                provider_request_id: None,
                generation_id: None,
            }),
            Ok(AssistantEvent::TextStart {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::TextDelta {
                index: ContentIndex::new(0),
                delta: r#"{"answer":"ok","confidence":0.9}"#.to_owned(),
            }),
            Ok(AssistantEvent::TextEnd {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ]);
        let message = collect_assistant_message_for_format(events, &format).await?;
        println!(
            "offline structured output: {:?}",
            message.structured_output()
        );
        return Ok(());
    }

    let client = support::client_with_phase2_capabilities()?;
    let request = GenerateRequest::new(
        ModelRef::new("official-openai", std::env::var("OPENAI_MODEL")?)?,
        vec![philo::Message::user(
            "Return JSON with answer and confidence for: What is 2+2?",
        )],
    )
    .with_options(GenerationOptions::new().with_response_format(format));

    // complete() still consumes one stream; it does not open a second request.
    let message = client.complete(request).await?;
    match message.structured_output() {
        Some(value) => println!("structured output: {value}"),
        None => println!("text fallback: {}", message.text()),
    }
    Ok(())
}
