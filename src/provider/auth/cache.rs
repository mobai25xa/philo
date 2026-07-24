//! Tenant and audience aware dynamic credential cache.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;

use super::dynamic::{CredentialIdentity, DynamicCredential, TenantId};
use crate::domain::ProviderId;
use crate::error::LlmError;
use crate::provider::catalog::ProductId;
use crate::provider::endpoint::CredentialAudience;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AuthCacheKey {
    pub(super) tenant_id: TenantId,
    pub(super) provider_id: ProviderId,
    pub(super) product_id: ProductId,
    pub(super) audience: CredentialAudience,
    pub(super) credential_identity: CredentialIdentity,
}

#[derive(Default)]
struct CacheState {
    credentials: HashMap<AuthCacheKey, DynamicCredential>,
    inflight: HashMap<AuthCacheKey, Arc<Notify>>,
}

/// Shared in-memory cache for short-lived credentials.
#[derive(Clone, Default)]
pub struct DynamicCredentialCache {
    state: Arc<Mutex<CacheState>>,
}

impl DynamicCredentialCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) async fn get_or_refresh<F, Fut>(
        &self,
        key: AuthCacheKey,
        refresh_window: Duration,
        allow_still_valid_fallback: bool,
        fetch: F,
    ) -> Result<DynamicCredential, LlmError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<DynamicCredential, LlmError>>,
    {
        loop {
            let (wait, fallback) = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                if let Some(cached) = state.credentials.get(&key)
                    && cached.is_fresh_at(now, refresh_window)
                {
                    return Ok(cached.clone());
                }
                let fallback = state
                    .credentials
                    .get(&key)
                    .filter(|cached| cached.expires_at() > now)
                    .cloned();
                if let Some(notify) = state.inflight.get(&key) {
                    (Some(notify.clone()), fallback)
                } else {
                    state.inflight.insert(key.clone(), Arc::new(Notify::new()));
                    (None, fallback)
                }
            };

            if let Some(notify) = wait {
                notify.notified().await;
                continue;
            }

            let result = fetch().await;
            let mut state = self.state.lock().await;
            if let Ok(credential) = &result {
                state.credentials.insert(key.clone(), credential.clone());
            }
            let notify = state.inflight.remove(&key);
            drop(state);
            if let Some(notify) = notify {
                notify.notify_waiters();
            }
            return match result {
                Ok(credential) => Ok(credential),
                Err(_) if allow_still_valid_fallback && fallback.is_some() => {
                    Ok(fallback.expect("checked above"))
                }
                Err(error) => Err(error),
            };
        }
    }
}

impl fmt::Debug for DynamicCredentialCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicCredentialCache")
            .field("contents", &"[REDACTED]")
            .finish()
    }
}
