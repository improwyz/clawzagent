//! JWT token creation and verification for session-based authentication.
//!
//! This module wraps the `jsonwebtoken` crate to provide a thin, gateway-specific
//! layer for signing and validating HS256 tokens. Tokens carry identity claims
//! ([`Claims`]) that the auth middleware promotes into an [`AuthContext`].
//!
//! # Security considerations
//! - The signing secret must be provided by the caller (typically from the
//!   `JWT_SECRET` environment variable).
//! - Tokens are short-lived; expiry is enforced during verification.
//!
//! # Key dependencies
//! - `jsonwebtoken` — encode/decode and signature verification.
//! - `chrono` — expiry and issued-at timestamps.

use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};

/// JWT payload claims used by the gateway.
///
/// Maps to the standard JWT claim names where applicable.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    /// Subject — unique identifier of the authenticated user (standard `sub`).
    pub sub: String,
    /// Email address of the user.
    pub email: String,
    /// Role / permission group (e.g. "admin", "operator", "agent").
    pub role: String,
    /// Expiration time as a Unix timestamp (standard `exp`).
    pub exp: usize,
    /// Issued-at time as a Unix timestamp (standard `iat`).
    pub iat: usize,
}

/// Create a new signed JWT for the given principal.
///
/// # Arguments
/// * `user_id` — subject identifier (`sub` claim).
/// * `email` — email claim.
/// * `role` — role claim.
/// * `secret` — symmetric signing key (HS256).
/// * `expiry_hours` — token lifetime in hours from now.
///
/// # Errors
/// Returns `jsonwebtoken::errors::Error` if encoding fails (e.g. invalid secret).
pub fn create_token(
    user_id: &str,
    email: &str,
    role: &str,
    secret: &str,
    expiry_hours: i64,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();
    let expiry = now + Duration::hours(expiry_hours);
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        role: role.to_string(),
        // jsonwebtoken expects usize timestamps; chrono gives i64.
        exp: expiry.timestamp() as usize,
        iat: now.timestamp() as usize,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

/// Verify and decode a JWT string, returning the enclosed claims.
///
/// # Arguments
/// * `token` — the raw JWT string (without the "Bearer " prefix).
/// * `secret` — symmetric signing key that must match the key used at creation.
///
/// # Errors
/// Returns `jsonwebtoken::errors::Error` if the token is malformed, expired,
/// or the signature does not verify.
pub fn verify_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let mut validation = Validation::new(Algorithm::HS256);
    // Explicitly enable expiry validation even though it is the default.
    // This makes the security intent obvious to readers.
    validation.validate_exp = true;
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;
    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_verify_token() {
        let secret = "test-secret-key";
        let token = create_token("user123", "user@example.com", "admin", secret, 24).unwrap();
        let claims = verify_token(&token, secret).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email, "user@example.com");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_invalid_token_rejected() {
        let result = verify_token("not.a.token", "secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret_rejected() {
        let token = create_token("user1", "u@e.com", "operator", "secret1", 1).unwrap();
        let result = verify_token(&token, "wrong-secret");
        assert!(result.is_err());
    }
}
