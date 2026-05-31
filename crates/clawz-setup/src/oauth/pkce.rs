//! PKCE (RFC 7636) helpers for setup OAuth flows.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use getrandom::getrandom;
use sha2::{Digest, Sha256};

/// A PKCE verifier/challenge pair for S256.
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

/// Generate a cryptographically random PKCE pair (`S256`).
pub fn generate_pkce() -> PkcePair {
    let mut bytes = [0u8; 32];
    getrandom(&mut bytes).expect("OS random");
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    PkcePair {
        verifier,
        challenge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let pair = generate_pkce();
        let digest = Sha256::digest(pair.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(pair.challenge, expected);
    }
}
