use aes_gcm::{aead::{AeadInPlace, NewAead}, Aes256Gcm, Key, Nonce};
use anyhow::{bail, Result};
use rand_core_05::OsRng;
use rand_core_05::RngCore;
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};

extern crate rand_core_05;

pub struct LocalKeypair {
    secret: Option<EphemeralSecret>,
    pub public: PublicKey,
}
impl LocalKeypair {
    pub fn generate() -> Self {
        let secret = EphemeralSecret::new(OsRng);
        let public  = PublicKey::from(&secret);
        Self { secret: Some(secret), public }
    }
    pub fn diffie_hellman(mut self, peer_public: &PublicKey) -> Result<SharedSecret> {
        let s = self.secret.take().ok_or_else(|| anyhow::anyhow!("already consumed"))?;
        Ok(s.diffie_hellman(peer_public))
    }
    pub fn public_bytes(&self) -> [u8; 32] { *self.public.as_bytes() }
}

pub struct SessionCipher { cipher: Aes256Gcm }
impl SessionCipher {
    pub fn from_shared_secret(shared: &SharedSecret) -> Self {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new(); h.update(shared.as_bytes());
        let kb = h.finalize();
        Self { cipher: Aes256Gcm::new(Key::from_slice(kb.as_slice())) }
    }
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut nb = [0u8; 12]; OsRng.fill_bytes(&mut nb);
        let nonce = Nonce::from_slice(&nb);
        let mut buf = plaintext.to_vec();
        let tag = self.cipher.encrypt_in_place_detached(nonce, b"", &mut buf)
            .map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;
        buf.extend_from_slice(tag.as_slice());
        Ok((nb.to_vec(), buf))
    }
    pub fn decrypt(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        if nonce.len() != 12 { bail!("bad nonce"); }
        if ciphertext.len() < 16 { bail!("too short"); }
        let (ct, tag_b) = ciphertext.split_at(ciphertext.len() - 16);
        let tag = aes_gcm::Tag::from_slice(tag_b);
        let mut buf = ct.to_vec();
        self.cipher.decrypt_in_place_detached(Nonce::from_slice(nonce), b"", &mut buf, tag)
            .map_err(|e| anyhow::anyhow!("decrypt: {e}"))?;
        Ok(buf)
    }
}
