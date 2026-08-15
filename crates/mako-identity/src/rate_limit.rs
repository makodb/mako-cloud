use std::{collections::HashMap, error::Error, fmt, sync::Arc, sync::Mutex};

use futures::executor::block_on;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{SignInAttemptOutcome, SignInThrottle, SignInThrottleDecision, SignInThrottleKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedWindowRateLimitConfig {
    maximum_attempts: u32,
    window_seconds: u64,
}

impl FixedWindowRateLimitConfig {
    pub fn new(maximum_attempts: u32, window_seconds: u64) -> Result<Self, RateLimitError> {
        if maximum_attempts == 0
            || maximum_attempts > 10_000
            || !(1..=24 * 60 * 60).contains(&window_seconds)
        {
            return Err(RateLimitError::InvalidConfiguration);
        }
        Ok(Self {
            maximum_attempts,
            window_seconds,
        })
    }
}

#[derive(Debug)]
pub struct FixedWindowSignInThrottle {
    config: FixedWindowRateLimitConfig,
    windows: Mutex<HashMap<SignInThrottleKey, AttemptWindow>>,
}

impl FixedWindowSignInThrottle {
    #[must_use]
    pub fn new(config: FixedWindowRateLimitConfig) -> Self {
        Self {
            config,
            windows: Mutex::new(HashMap::new()),
        }
    }
}

impl SignInThrottle for FixedWindowSignInThrottle {
    fn check(&self, key: &SignInThrottleKey, now_unix_seconds: u64) -> SignInThrottleDecision {
        let Ok(mut windows) = self.windows.lock() else {
            return SignInThrottleDecision::Throttled {
                retry_after_seconds: self.config.window_seconds,
            };
        };
        let window = windows.entry(key.clone()).or_insert(AttemptWindow {
            started_at_unix_seconds: now_unix_seconds,
            admitted_attempts: 0,
        });
        let deadline = window
            .started_at_unix_seconds
            .saturating_add(self.config.window_seconds);
        if now_unix_seconds >= deadline {
            *window = AttemptWindow {
                started_at_unix_seconds: now_unix_seconds,
                admitted_attempts: 0,
            };
        } else if now_unix_seconds < window.started_at_unix_seconds {
            return SignInThrottleDecision::Throttled {
                retry_after_seconds: self.config.window_seconds,
            };
        }
        if window.admitted_attempts >= self.config.maximum_attempts {
            return SignInThrottleDecision::Throttled {
                retry_after_seconds: window
                    .started_at_unix_seconds
                    .saturating_add(self.config.window_seconds)
                    .saturating_sub(now_unix_seconds)
                    .max(1),
            };
        }
        window.admitted_attempts += 1;
        SignInThrottleDecision::Allowed
    }

    fn record(&self, key: &SignInThrottleKey, outcome: SignInAttemptOutcome) {
        if outcome == SignInAttemptOutcome::Succeeded
            && let Ok(mut windows) = self.windows.lock()
        {
            windows.remove(key);
        }
    }
}

/// Durable, fail-closed sign-in admission state for production service graphs.
/// The key is already a one-way digest of tenant, email, and network partition,
/// so no credential or address value is persisted.
#[derive(Clone)]
pub struct PersistentSignInThrottle {
    adapter: Arc<dyn KvAdapter>,
    config: FixedWindowRateLimitConfig,
    durability: Durability,
}

impl fmt::Debug for PersistentSignInThrottle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistentSignInThrottle")
            .field("config", &self.config)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl PersistentSignInThrottle {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        config: FixedWindowRateLimitConfig,
        durability: Durability,
    ) -> Result<Self, RateLimitError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(RateLimitError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            config,
            durability,
        })
    }

    async fn reserve(
        &self,
        key: &SignInThrottleKey,
        now_unix_seconds: u64,
    ) -> Result<SignInThrottleDecision, RateLimitError> {
        let key = persistent_key(key);
        for _ in 0..16 {
            let previous = self
                .adapter
                .get(&key)
                .await
                .map_err(|_| RateLimitError::StorageUnavailable)?;
            let mut window = previous
                .as_deref()
                .map(|value| {
                    serde_json::from_slice::<StoredAttemptWindow>(value)
                        .map_err(|_| RateLimitError::CorruptState)
                })
                .transpose()?
                .unwrap_or(StoredAttemptWindow {
                    started_at_unix_seconds: now_unix_seconds,
                    admitted_attempts: 0,
                });
            let deadline = window
                .started_at_unix_seconds
                .saturating_add(self.config.window_seconds);
            if now_unix_seconds >= deadline {
                window = StoredAttemptWindow {
                    started_at_unix_seconds: now_unix_seconds,
                    admitted_attempts: 0,
                };
            } else if now_unix_seconds < window.started_at_unix_seconds {
                return Ok(SignInThrottleDecision::Throttled {
                    retry_after_seconds: self.config.window_seconds,
                });
            }
            if window.admitted_attempts >= self.config.maximum_attempts {
                return Ok(SignInThrottleDecision::Throttled {
                    retry_after_seconds: window
                        .started_at_unix_seconds
                        .saturating_add(self.config.window_seconds)
                        .saturating_sub(now_unix_seconds)
                        .max(1),
                });
            }
            window.admitted_attempts = window
                .admitted_attempts
                .checked_add(1)
                .ok_or(RateLimitError::CorruptState)?;
            let condition = previous.as_ref().map_or_else(
                || KeyCondition::Missing { key: key.clone() },
                |value| KeyCondition::ValueEquals {
                    key: key.clone(),
                    value: value.clone(),
                },
            );
            let mut batch = WriteBatch::new();
            batch.put(
                &key,
                serde_json::to_vec(&window).map_err(|_| RateLimitError::CorruptState)?,
            );
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability: self.durability,
                })
                .await
                .map_err(|_| RateLimitError::StorageUnavailable)?
                == CompareAndWriteResult::Applied
            {
                return Ok(SignInThrottleDecision::Allowed);
            }
        }
        Err(RateLimitError::ConcurrentUpdate)
    }

    async fn clear(&self, key: &SignInThrottleKey) -> Result<(), RateLimitError> {
        let key = persistent_key(key);
        for _ in 0..16 {
            let Some(previous) = self
                .adapter
                .get(&key)
                .await
                .map_err(|_| RateLimitError::StorageUnavailable)?
            else {
                return Ok(());
            };
            let mut batch = WriteBatch::new();
            batch.delete(&key);
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: key.clone(),
                        value: previous,
                    }],
                    batch,
                    durability: self.durability,
                })
                .await
                .map_err(|_| RateLimitError::StorageUnavailable)?
                == CompareAndWriteResult::Applied
            {
                return Ok(());
            }
        }
        Err(RateLimitError::ConcurrentUpdate)
    }
}

impl SignInThrottle for PersistentSignInThrottle {
    fn check(&self, key: &SignInThrottleKey, now_unix_seconds: u64) -> SignInThrottleDecision {
        std::thread::scope(|scope| {
            scope
                .spawn(|| block_on(self.reserve(key, now_unix_seconds)))
                .join()
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or(SignInThrottleDecision::Throttled {
            retry_after_seconds: self.config.window_seconds,
        })
    }

    fn record(&self, key: &SignInThrottleKey, outcome: SignInAttemptOutcome) {
        if outcome == SignInAttemptOutcome::Succeeded {
            let _ = std::thread::scope(|scope| scope.spawn(|| block_on(self.clear(key))).join());
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAttemptWindow {
    started_at_unix_seconds: u64,
    admitted_attempts: u32,
}

fn persistent_key(key: &SignInThrottleKey) -> Vec<u8> {
    const PREFIX: &[u8] = b"mako:identity:signin-throttle:v1/";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = Vec::with_capacity(PREFIX.len() + 64);
    encoded.extend_from_slice(PREFIX);
    for byte in key.0 {
        encoded.push(HEX[usize::from(byte >> 4)]);
        encoded.push(HEX[usize::from(byte & 0x0f)]);
    }
    encoded
}

#[derive(Clone, Copy, Debug)]
struct AttemptWindow {
    started_at_unix_seconds: u64,
    admitted_attempts: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitError {
    InvalidConfiguration,
    UnsupportedDurability,
    StorageUnavailable,
    CorruptState,
    ConcurrentUpdate,
}

impl fmt::Display for RateLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "invalid authentication rate-limit configuration",
            Self::UnsupportedDurability => "authentication rate-limit durability is unsupported",
            Self::StorageUnavailable => "authentication rate-limit storage is unavailable",
            Self::CorruptState => "authentication rate-limit state is corrupt",
            Self::ConcurrentUpdate => "authentication rate-limit state is contended",
        })
    }
}

impl Error for RateLimitError {}

#[cfg(test)]
mod tests {
    use mako_storage::MemoryAdapter;

    use super::*;

    #[test]
    fn limiter_reserves_admission_fails_closed_and_reports_retry_window() {
        let limiter =
            FixedWindowSignInThrottle::new(FixedWindowRateLimitConfig::new(2, 10).expect("config"));
        let first = SignInThrottleKey([1; 32]);
        let second = SignInThrottleKey([2; 32]);

        assert_eq!(limiter.check(&first, 100), SignInThrottleDecision::Allowed);
        assert_eq!(limiter.check(&first, 101), SignInThrottleDecision::Allowed);
        assert_eq!(
            limiter.check(&first, 102),
            SignInThrottleDecision::Throttled {
                retry_after_seconds: 8
            }
        );
        assert_eq!(limiter.check(&second, 102), SignInThrottleDecision::Allowed);
        assert_eq!(limiter.check(&first, 110), SignInThrottleDecision::Allowed);
        assert!(matches!(
            limiter.check(&first, 109),
            SignInThrottleDecision::Throttled { .. }
        ));
        limiter.record(&first, SignInAttemptOutcome::Succeeded);
        assert_eq!(limiter.check(&first, 109), SignInThrottleDecision::Allowed);
    }

    #[test]
    fn persistent_limiter_survives_recomposition_and_clears_success() {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let config = FixedWindowRateLimitConfig::new(2, 10).expect("config");
        let key = SignInThrottleKey([7; 32]);
        let first = PersistentSignInThrottle::new(Arc::clone(&adapter), config, Durability::Memory)
            .expect("limiter");
        assert_eq!(first.check(&key, 100), SignInThrottleDecision::Allowed);
        assert_eq!(first.check(&key, 101), SignInThrottleDecision::Allowed);

        let restarted =
            PersistentSignInThrottle::new(adapter, config, Durability::Memory).expect("limiter");
        assert!(matches!(
            restarted.check(&key, 102),
            SignInThrottleDecision::Throttled { .. }
        ));
        restarted.record(&key, SignInAttemptOutcome::Succeeded);
        assert_eq!(restarted.check(&key, 103), SignInThrottleDecision::Allowed);
    }
}
