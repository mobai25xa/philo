//! Registers two explicit products and protocols under one provider identity.

use std::error::Error;

use http::HeaderName;
use philo::{
    AuthScheme, EndpointConfig, ProductId, ProviderCapabilities, ProviderDefinition, ProviderId,
    ProviderRegistration, ProviderRegistry,
};

fn main() -> Result<(), Box<dyn Error>> {
    let provider = ProviderId::new("example-multi-protocol-gateway")?;
    let chat_product = ProductId::new("chat")?;
    let messages_product = ProductId::new("messages")?;
    let endpoint = "https://gateway.example.com/v1";
    let chat = ProviderDefinition::openai_chat(provider.clone(), chat_product.clone())
        .with_endpoint(EndpointConfig::absolute(endpoint)?)
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::bearer())
        .with_capabilities(ProviderCapabilities::conservative_chat_completions())
        .allow_unregistered_models()
        .build()?;
    let messages =
        ProviderDefinition::anthropic_messages(provider.clone(), messages_product.clone())
            .with_endpoint(EndpointConfig::absolute(endpoint)?)
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::api_key_header(HeaderName::from_static(
                "x-api-key",
            ))?)
            .with_anthropic_version("2023-06-01")?
            .with_capabilities(ProviderCapabilities::conservative_messages())
            .allow_unregistered_models()
            .build()?;

    let registry = ProviderRegistry::new();
    registry.register(ProviderRegistration::from_definition(chat)?)?;
    registry.register(ProviderRegistration::from_definition(messages)?)?;

    assert_eq!(
        registry
            .get_product(&provider, &chat_product)?
            .expect("chat product")
            .protocol_id()
            .expect("static protocol")
            .as_str(),
        "openai-chat-completions"
    );
    assert_eq!(
        registry
            .get_product(&provider, &messages_product)?
            .expect("messages product")
            .protocol_id()
            .expect("static protocol")
            .as_str(),
        "anthropic-messages"
    );
    Ok(())
}
