use regex::Regex;
use std::sync::LazyLock;

/// Compiled once — avoids Regex::new per call on hot path
static SECRET_RES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r#"(?i)["']?api[_-]?key["']?\s*[:=]\s*["']?[A-Za-z0-9\-_\.]{8,}"#).unwrap(), "api_key=[REDACTED]"),
        (Regex::new(r#"(?i)["']?secret["']?\s*[:=]\s*["']?[A-Za-z0-9\-_\.]{8,}"#).unwrap(), "secret=[REDACTED]"),
        (Regex::new(r#"(?i)["']?token["']?\s*[:=]\s*["']?[A-Za-z0-9\-_\.]{8,}"#).unwrap(), "token=[REDACTED]"),
        (Regex::new(r#"sk-[A-Za-z0-9]{10,}"#).unwrap(), "[REDACTED]"),
        (Regex::new(r#"ghp_[A-Za-z0-9]{10,}"#).unwrap(), "[REDACTED]"),
    ]
});

/// Redact secrets before any outbound API call (spec §6 step 11, packaging & hardening)
/// Replaces patterns like `api_key: sk-...`, `secret=...`, `token ...` with `[REDACTED]`
pub fn redact_secrets(text: &str) -> String {
    // No backrefs — Rust regex crate doesn't support \2
    let mut out = text.to_string();
    for (re, rep) in SECRET_RES.iter() {
        out = re.replace_all(&out, *rep).to_string();
    }
    out
}

/// Canonical HMAC verification — single impl for DBOS + ratelimit (v0.6.0)
/// Uses `hmac` crate `Hmac<Sha256>` (not sha256(secret+body) pre-v0.6.0)
pub fn verify_hmac(secret: &str, body: &str, signature: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body.as_bytes());
    let result = mac.finalize();
    let expected = result.into_bytes();
    let mut hex = String::with_capacity(expected.len() * 2);
    for b in expected.iter() {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex == signature
}

pub fn hmac_hex(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(body.as_bytes());
    let result = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(result.len() * 2);
    for b in result.iter() {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

/// Check whether text contains a secret (for logging WARN before redacting)
pub fn contains_secret(text: &str) -> bool {
    redact_secrets(text) != text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_api_key() {
        let input = r#"{"api_key": "sk-abc1234567890abcdef"}"#;
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("sk-abc"));
    }

    #[test]
    fn test_redact_secret() {
        let input = "secret=supersecretvalue123";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_no_false_positive() {
        let input = "hello world, implement auth feature";
        assert_eq!(redact_secrets(input), input);
        assert!(!contains_secret(input));
    }

    #[test]
    fn test_redact_token() {
        let input = "token: ghp_abc1234567890abcdef123456";
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_verify_hmac() {
        let secret = "s3cret";
        let body = r#"{"task":"hi"}"#;
        let sig = hmac_hex(secret, body);
        assert!(verify_hmac(secret, body, &sig));
        assert!(!verify_hmac(secret, body, "bad"));
        assert!(!verify_hmac("other", body, &sig));
    }

    #[test]
    fn test_hmac_hex_deterministic() {
        let secret = "a-secret";
        let body = "hello world";
        let h1 = hmac_hex(secret, body);
        let h2 = hmac_hex(secret, body);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA256 hex len
        assert_ne!(hmac_hex(secret, "different"), h1);
        assert_ne!(hmac_hex("other-secret", body), h1);
    }

    #[test]
    fn test_verify_hmac_empty() {
        let sig = hmac_hex("", "");
        assert!(verify_hmac("", "", &sig));
        assert!(!verify_hmac("", "", "00"));
        assert!(!verify_hmac("", "body", &sig));
    }

    #[test]
    fn test_verify_hmac_wrong_len() {
        assert!(!verify_hmac("k", "b", "abc"));
        assert!(!verify_hmac("k", "b", ""));
        assert!(!verify_hmac("k", "b", &"a".repeat(63)));
        assert!(!verify_hmac("k", "b", &"a".repeat(65)));
    }

    #[test]
    fn test_redact_multiple_secrets() {
        let input = r#"api_key="sk-abc123456789" secret: mysecretval123 token ghp_abc1234567890"#;
        let redacted = redact_secrets(input);
        // sk- pattern and ghp_ should be redacted, plus api_key/secret/token wrappers
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("sk-abc"));
        assert!(!redacted.contains("ghp_abc"));
        // Original markers should be gone
        assert!(!redacted.contains("sk-abc123456789"));
    }

    #[test]
    fn test_redact_empty_and_no_match() {
        assert_eq!(redact_secrets(""), "");
        assert_eq!(redact_secrets("no secrets here"), "no secrets here");
        // short tokens <8 chars should not be redacted (threshold)
        assert_eq!(redact_secrets("api_key: short"), "api_key: short");
    }

    #[test]
    fn test_contains_secret_variants() {
        assert!(contains_secret("api_key=sk-abc1234567890abcdef"));
        assert!(contains_secret("ghp_12345678901234567890"));
        assert!(!contains_secret("api_key: value")); // too short
        assert!(!contains_secret(""));
    }

    #[test]
    fn test_redact_case_insensitive() {
        let input = r#"API_KEY: "sk-XYZ1234567890ABCDE" SECRET=MySecretValue123 TOKEN: ghp_ABCDEF1234567890"#;
        let redacted = redact_secrets(input);
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_preserves_non_secret() {
        let input = "implement auth feature for API endpoint";
        assert_eq!(redact_secrets(input), input);
        // Check token "token" word alone without secret value not redacted
        let input2 = "the token bucket algorithm is used for rate limiting";
        // This contains "token" but not token=VALUE pattern with 8+ alphanum — should be redacted? our pattern requires token\s*[:=]
        // So this should not be redacted
        assert!(!contains_secret(input2));
    }
}
