use std::error::Error;

use philo::{GenerateRequest, LlmClient, Message, ModelRef, OfficialOpenAiProfile};

pub(crate) type ExampleResult<T = ()> = Result<T, Box<dyn Error>>;

pub(crate) fn client() -> ExampleResult<LlmClient> {
    let key = std::env::var("OPENAI_API_KEY")?;
    let runtime = OfficialOpenAiProfile::from_api_key(key)?.build()?;
    Ok(LlmClient::with_reqwest(runtime)?)
}

pub(crate) fn request(prompt: &str) -> ExampleResult<GenerateRequest> {
    let model = std::env::var("OPENAI_MODEL")?;
    Ok(GenerateRequest::new(
        ModelRef::new("official-openai", model)?,
        vec![Message::user(prompt)],
    ))
}
