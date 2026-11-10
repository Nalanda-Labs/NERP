//! Encryption of secrets stored in the database (mailbox passwords).
//!
//! AES-256-GCM with a random 96-bit nonce per value; stored as nonce || ciphertext. The
//! key comes from DATA_ENCRYPTION_KEY, or is derived from JWT_SECRET when that is unset.
//! Changing the key makes stored secrets unreadable (users re-enter their passwords).

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use sha2::{Digest, Sha256};

pub fn key_from(secret: &str) -> [u8; 32] {
    Sha256::digest(format!("nerp:data-key:v1:{secret}").as_bytes()).into()
}

pub fn seal(key: &[u8; 32], plaintext: &str) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let mut nonce = [0u8; 12];
    rand::fill(&mut nonce);
    let ct = cipher.encrypt(&Nonce::try_from(&nonce[..]).expect("12-byte nonce"), plaintext.as_bytes()).expect("encryption cannot fail");
    [nonce.to_vec(), ct].concat()
}

pub fn open(key: &[u8; 32], data: &[u8]) -> Option<String> {
    if data.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::try_from(&data[..12]).ok()?;
    let pt = cipher.decrypt(&nonce, &data[12..]).ok()?;
    String::from_utf8(pt).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper() {
        let k = key_from("secret");
        let sealed = seal(&k, "pässwörd");
        assert_eq!(open(&k, &sealed).as_deref(), Some("pässwörd"));
        assert_ne!(seal(&k, "x"), seal(&k, "x"), "nonces differ");
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&k, &bad).is_none());
        assert!(open(&key_from("other"), &sealed).is_none());
    }
}
