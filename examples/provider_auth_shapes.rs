//! Constructs static and dynamic `AuthProvider` shapes without resolving credentials.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use http::HeaderName;
use philo::{
    ApiKey, ApiKeyHeaderAuth, AuthProvider, BearerAuth, BearerCredential, CredentialFuture,
    CredentialIdentity, DynamicAuth, DynamicCredential, DynamicCredentialContext,
    DynamicCredentialSource, MultiHeaderAuth, NoAuth, TenantId,
};
use tokio::time::Instant;

#[derive(Debug)]
struct ApplicationCredentialSource;

impl DynamicCredentialSource for ApplicationCredentialSource {
    fn acquire(&self, _context: DynamicCredentialContext) -> CredentialFuture {
        Box::pin(async {
            DynamicCredential::bearer(
                ApiKey::new("application-supplied-short-lived-token").unwrap(),
                Instant::now() + Duration::from_mins(5),
            )
        })
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let audience = philo::CredentialAudience::OfficialOpenAi;
    let bearer = BearerAuth::new(BearerCredential::new(
        ApiKey::new("resolved-static-token")?,
        audience.clone(),
    ));
    let api_key = ApiKeyHeaderAuth::new(
        HeaderName::from_static("x-api-key"),
        ApiKey::new("resolved-api-key")?,
        audience.clone(),
    )?;
    let multi = MultiHeaderAuth::new(
        vec![
            (
                HeaderName::from_static("x-api-key"),
                ApiKey::new("resolved-api-key")?,
            ),
            (
                HeaderName::from_static("x-api-signature"),
                ApiKey::new("resolved-signature")?,
            ),
        ],
        audience.clone(),
    )?;
    let dynamic = DynamicAuth::new(
        Arc::new(ApplicationCredentialSource),
        audience,
        TenantId::new("tenant-partition")?,
        CredentialIdentity::new("workload-identity")?,
    );

    for auth in [
        &bearer as &dyn AuthProvider,
        &api_key,
        &multi,
        &dynamic,
        &NoAuth,
    ] {
        println!(
            "scheme={:?} source={:?} headers={:?}",
            auth.scheme_kind(),
            auth.credential_source_kind(),
            auth.protected_headers()
        );
    }
    Ok(())
}
