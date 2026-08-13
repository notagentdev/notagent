//! PKCE verifier and challenge.
//!
//! 1:1 port of `packages/ai/src/auth/oauth/pkce.ts` (34 LOC). Substitution class 3:
//! WebCrypto becomes sha2 plus a random source.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use sha2::{Digest, Sha256};

/// `base64urlEncode(bytes)`
pub fn base64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The verifier/challenge pair of [`generate_pkce`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// `generatePKCE()` — 32 random bytes as the verifier, its SHA-256 as the challenge.
pub fn generate_pkce() -> Pkce {
    let mut verifier_bytes = [0u8; 32];
    rand::rng().fill(&mut verifier_bytes);
    let verifier = base64url_encode(&verifier_bytes);
    let challenge = base64url_encode(&Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

/// The challenge for a given verifier; exposed for tests and manual flows.
pub fn challenge_for(verifier: &str) -> String {
    base64url_encode(&Sha256::digest(verifier.as_bytes()))
}
