use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Token-bucket per session_id — in-memory, lite, at adapter layer.
/// LiteLLM still handles provider-side quota. This protects the daemon.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug, Clone)]
pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl RateLimiter {
    pub fn new(per_sec: f64, burst: usize) -> Self {
        Self {
            capacity: burst as f64,
            refill_per_sec: per_sec,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Try to consume 1 token for session_id. Returns true if allowed.
    pub fn try_acquire(&self, session_id: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(session_id.to_string()).or_insert(Bucket { tokens: self.capacity, last_refill: now });
        // refill
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn is_rate_limited(&self, session_id: &str) -> bool {
        !self.try_acquire(session_id)
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(10.0, 20)
    }
}

/// HMAC-SHA256 verification — delegates to canonical knocode-core::secrets (v0.6.0 single impl)
pub fn verify_hmac(secret: &str, body: &str, signature: &str) -> bool {
    knocode_core::secrets::verify_hmac(secret, body, signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn test_rate_limit_allows_burst() {
        let rl = RateLimiter::new(10.0, 5);
        for _ in 0..5 { assert!(rl.try_acquire("sess1")); }
        assert!(!rl.try_acquire("sess1"));
    }

    #[test]
    fn test_rate_limit_refills() {
        let rl = RateLimiter::new(100.0, 2);
        assert!(rl.try_acquire("s"));
        assert!(rl.try_acquire("s"));
        assert!(!rl.try_acquire("s"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(rl.try_acquire("s"));
    }

    #[test]
    fn test_rate_limit_isolation() {
        let rl = RateLimiter::new(1.0, 1);
        assert!(rl.try_acquire("a"));
        assert!(rl.try_acquire("b")); // different session not limited
        assert!(!rl.try_acquire("a"));
    }

    #[test]
    fn test_hmac_verify() {
        let secret = "test-secret";
        let body = r#"{"workflow_id":"wf_1"}"#;
        let sig = knocode_core::secrets::hmac_hex(secret, body);
        assert!(verify_hmac(secret, body, &sig));
        assert!(!verify_hmac(secret, body, "bad"));
        assert!(!verify_hmac("other", body, &sig));
    }
}
