#![allow(dead_code)]

use std::collections::HashMap;
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
static PEER_ECDH_KEYS: Mutex<Option<HashMap<u64, [u8; 32]>>> = Mutex::new(None);

/// Obtém ou gera o par de chaves X25519 efêmero na RAM para esta sessão
pub fn get_or_create_local_ecdh_keypair() -> [u8; 32] {
    let mut pub_lock = LOCAL_ECDH_PUBLIC.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pub_bytes) = *pub_lock {
        return pub_bytes;
    }
    let secret = StaticSecret::random_from_rng(rand::thread_rng());
    let public = PublicKey::from(&secret);
    let pub_bytes = *public.as_bytes();
    *LOCAL_ECDH_SECRET.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret);
    *pub_lock = Some(pub_bytes);
    pub_bytes
}

/// Deriva a chave simétrica AES-256-GCM exclusiva via multiplicação escalar X25519 ECDH
pub fn compute_peer_shared_key(peer_uid: u64, peer_pub_bytes: &[u8; 32], fallback_cid: u64) -> [u8; 32] {
    let secret_lock = LOCAL_ECDH_SECRET.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref local_secret) = *secret_lock {
        let peer_public = PublicKey::from(*peer_pub_bytes);
        let shared_secret = local_secret.diffie_hellman(&peer_public);
        let mut hasher = Sha256::new();
        hasher.update(b"litecord_x25519_ecdh_aes256_gcm_salt_v1_2026");
        hasher.update(shared_secret.as_bytes());
        hasher.update(&peer_uid.to_be_bytes());
        let res = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&res);

        let mut keys_guard = PEER_ECDH_KEYS.lock().unwrap_or_else(|e| e.into_inner());
        let map = keys_guard.get_or_insert_with(HashMap::new);
        map.insert(peer_uid, key);
        return key;
    }
    get_voice_encryption_key(fallback_cid)
}

/// Retorna a chave negociada via ECDH para o peer ou fallback para a chave do canal
pub fn get_peer_encryption_key(peer_uid: u64, fallback_cid: u64) -> [u8; 32] {
    if peer_uid != 0 {
        if let Some(ref map) = *PEER_ECDH_KEYS.lock().unwrap_or_else(|e| e.into_inner()) {
            if let Some(&key) = map.get(&peer_uid) {
                return key;
            }
        }
    }
    get_voice_encryption_key(fallback_cid)
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
