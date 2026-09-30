//! Authenticated encryption for resumable permission payloads.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use serde::Serialize;
use zeroize::Zeroize;

const NONCE_BYTES: usize = 12;

#[derive(Clone)]
pub struct ApprovalPayloadVault {
    cipher: Aes256Gcm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedApprovalPayload {
    pub payload_ref: String,
    pub payload_digest: String,
    pub encryption_key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPayloadContext<'a> {
    pub permission_request_id: &'a str,
    pub session_id: &'a str,
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub step_id: &'a str,
    pub tool_call_id: &'a str,
    pub provider_id: &'a str,
    pub descriptor_revision: &'a str,
    pub policy_revision: &'a str,
}

impl ApprovalPayloadContext<'_> {
    pub fn to_aad(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self)
            .map_err(|error| format!("approval payload context serialization failed: {error}"))
    }
}

impl ApprovalPayloadVault {
    pub fn from_encoded_key(encoded_key: &str) -> Result<Self, String> {
        let mut key = sdkwork_utils_rust::base64url_decode(encoded_key)
            .ok_or_else(|| "approval payload encryption key is not valid base64url".to_string())?;
        if key.len() != 32 {
            // Zeroize malformed key material before dropping so it never
            // lingers in freed heap memory.
            key.zeroize();
            return Err("approval payload encryption key must decode to 32 bytes".to_string());
        }
        let cipher = Aes256Gcm::new_from_slice(&key);
        key.zeroize();
        let cipher = cipher.map_err(|_| "approval payload encryption key is invalid".to_string())?;
        Ok(Self { cipher })
    }

    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> Result<SealedApprovalPayload, String> {
        let mut nonce = [0_u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .map_err(|_| "approval payload encryption failed".to_string())?;
        let mut sealed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(SealedApprovalPayload {
            payload_ref: sdkwork_utils_rust::base64url_encode(&sealed),
            payload_digest: sdkwork_utils_rust::sha256_hash(plaintext.as_bytes()),
            encryption_key_id: "approval-payload-v1".to_string(),
        })
    }

    pub fn open(&self, payload_ref: &str, aad: &[u8]) -> Result<String, String> {
        let sealed = sdkwork_utils_rust::base64url_decode(payload_ref)
            .ok_or_else(|| "approval payload is not valid base64url".to_string())?;
        if sealed.len() <= NONCE_BYTES {
            return Err("approval payload is truncated".to_string());
        }
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&sealed[..NONCE_BYTES]),
                Payload {
                    msg: &sealed[NONCE_BYTES..],
                    aad,
                },
            )
            .map_err(|_| "approval payload authentication failed".to_string())?;
        String::from_utf8(plaintext)
            .map_err(|_| "approval payload plaintext is not UTF-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> ApprovalPayloadVault {
        ApprovalPayloadVault::from_encoded_key(&sdkwork_utils_rust::base64url_encode(&[7_u8; 32]))
            .expect("vault")
    }

    #[test]
    fn seal_roundtrip_hides_plaintext_and_binds_aad() {
        let vault = vault();
        let sealed = vault
            .seal("sensitive tool input", b"permission.1|session.1|tool.1")
            .expect("sealed");
        assert!(!sealed.payload_ref.contains("sensitive"));
        assert_eq!(sealed.payload_digest.len(), 64);
        assert_eq!(
            vault
                .open(&sealed.payload_ref, b"permission.1|session.1|tool.1")
                .expect("opened"),
            "sensitive tool input"
        );
        assert!(vault
            .open(&sealed.payload_ref, b"permission.2|session.1|tool.1")
            .is_err());
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let vault = vault();
        let sealed = vault.seal("payload", b"aad").expect("sealed");
        let mut bytes = sdkwork_utils_rust::base64url_decode(&sealed.payload_ref).expect("decode");
        let last = bytes.last_mut().expect("ciphertext");
        *last ^= 1;
        assert!(vault
            .open(&sdkwork_utils_rust::base64url_encode(&bytes), b"aad")
            .is_err());
    }
}
