//! Signatures on the key-to-id answers of a binary cache (`docs/cache.md`).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A new key pair as (secret, public), each 64 hex characters.
pub fn generate() -> Result<(String, String), String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("random bytes: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    Ok((to_hex(&seed), to_hex(key.verifying_key().as_bytes())))
}

/// The signature of `message` under the secret key, 128 hex characters.
pub fn sign(secret: &str, message: &str) -> Result<String, String> {
    let seed = from_hex::<32>(secret).ok_or("secret key: expected 64 hex characters")?;
    Ok(to_hex(
        &SigningKey::from_bytes(&seed)
            .sign(message.as_bytes())
            .to_bytes(),
    ))
}

/// Whether `signature` is the signature of `message` under the public key.
pub fn verify(public: &str, message: &str, signature: &str) -> bool {
    let Some(key) = from_hex::<32>(public).and_then(|b| VerifyingKey::from_bytes(&b).ok()) else {
        return false;
    };
    let Some(signature) = from_hex::<64>(signature) else {
        return false;
    };
    key.verify(message.as_bytes(), &Signature::from_bytes(&signature))
        .is_ok()
}
