#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use super::types::get_process_instance_id;

static LOCAL_ECDH_SECRET: Mutex<Option<StaticSecret>> = Mutex::new(None);
static LOCAL_ECDH_PUBLIC: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// Obtém ou gera o par de chaves X25519 efêmero na RAM para esta sessão
pub fn get_or_create_local_ecdh_keypair() -> [u8; 32] {
    let mut pub_lock = LOCAL_ECDH_PUBLIC.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pub_bytes) = *pub_lock {
        return pub_bytes;
    }
    let secret = StaticSecret::from(rand::random::<[u8; 32]>());
    let public = PublicKey::from(&secret);
    let pub_bytes = *public.as_bytes();
    *LOCAL_ECDH_SECRET.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret);
    *pub_lock = Some(pub_bytes);
    pub_bytes
}

/// The key two peers share: X25519 of our secret with their public key, bound to both
/// user ids (in a fixed order, so both sides derive the same one) and the channel.
pub fn pair_key(peer_public: &[u8; 32], my_uid: u64, peer_uid: u64, cid: u64) -> Option<[u8; 32]> {
    get_or_create_local_ecdh_keypair();
    let secret = LOCAL_ECDH_SECRET.lock().unwrap_or_else(|e| e.into_inner());
    Some(pair_key_from(secret.as_ref()?, peer_public, my_uid, peer_uid, cid))
}

fn pair_key_from(secret: &StaticSecret, peer_public: &[u8; 32], uid_a: u64, uid_b: u64, cid: u64) -> [u8; 32] {
    let shared = secret.diffie_hellman(&PublicKey::from(*peer_public));
    let mut hasher = Sha256::new();
    hasher.update(b"litecord-p2p-pair-v3\0");
    hasher.update(shared.as_bytes());
    hasher.update(uid_a.min(uid_b).to_be_bytes());
    hasher.update(uid_a.max(uid_b).to_be_bytes());
    hasher.update(cid.to_be_bytes());
    hasher.finalize().into()
}

/// Deriva uma chave AES de 32 bytes exclusiva e sincronizada para todos os participantes do canal de voz
pub fn get_voice_encryption_key(cid: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"litecord_e2ee_voice_p2p_channel_salt_v3_2026");
    hasher.update(&cid.to_be_bytes());
    let res = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&res);
    key
}

static ENCRYPT_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Criptografa o payload usando AES-256-GCM com Nonce determinístico de 12 bytes ultrarrápido (0ns overhead)
pub fn encrypt_signaling_payload(key_bytes: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let mut nonce_bytes = [0u8; 12];
    let inst = get_process_instance_id();
    let ctr = ENCRYPT_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    nonce_bytes[0..4].copy_from_slice(&inst.to_be_bytes());
    nonce_bytes[4..12].copy_from_slice(&ctr.to_be_bytes());
    let nonce = Nonce::from_slice(&nonce_bytes);

    match cipher.encrypt(nonce, plaintext) {
        Ok(ciphertext) => {
            let mut out = Vec::with_capacity(12 + ciphertext.len());
            out.extend_from_slice(&nonce_bytes);
            out.extend_from_slice(&ciphertext);
            Some(out)
        }
        Err(_) => None,
    }
}

/// Descriptografa o payload via AES-256-GCM. Rejeita pacotes forjados ou sem autenticação válida.
pub fn decrypt_signaling_payload(key_bytes: &[u8; 32], encrypted_data: &[u8]) -> Option<Vec<u8>> {
    if encrypted_data.len() < 12 + 16 {
        return None;
    }
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&encrypted_data[..12]);
    let ciphertext = &encrypted_data[12..];

    cipher.decrypt(nonce, ciphertext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_peers_derive_the_same_pair_key_and_only_they_open_what_it_wraps() {
        let (a, b, c) = (StaticSecret::from([7u8; 32]), StaticSecret::from([9u8; 32]), StaticSecret::from([11u8; 32]));
        let (pa, pb) = (*PublicKey::from(&a).as_bytes(), *PublicKey::from(&b).as_bytes());
        let ka = pair_key_from(&a, &pb, 100, 200, 5);
        let kb = pair_key_from(&b, &pa, 200, 100, 5);
        assert_eq!(ka, kb);
        // Another channel, or a third party with B's public key, gets something else.
        assert_ne!(ka, pair_key_from(&a, &pb, 100, 200, 6));
        let kc = pair_key_from(&c, &pb, 100, 200, 5);
        assert_ne!(ka, kc);

        let stream_key = [42u8; 32];
        let wrapped = encrypt_signaling_payload(&ka, &stream_key).unwrap();
        assert_eq!(wrapped.len(), 60);
        assert_eq!(decrypt_signaling_payload(&kb, &wrapped).unwrap(), stream_key);
        assert!(decrypt_signaling_payload(&kc, &wrapped).is_none());
    }
}
