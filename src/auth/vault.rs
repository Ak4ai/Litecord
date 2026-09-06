#![allow(dead_code)]

use log::info;
use crate::AppWindow;
use crate::AccountItem;

#[cfg(target_os = "windows")]
#[repr(C)]
#[allow(non_snake_case)]
pub struct DATA_BLOB {
    cbData: u32,
    pbData: *mut u8,
}

#[cfg(target_os = "windows")]
#[link(name = "crypt32")]
extern "system" {
    pub fn CryptProtectData(
        pDataIn: *const DATA_BLOB,
        szDataDescr: *const u16,
        pOptionalEntropy: *const DATA_BLOB,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DATA_BLOB,
    ) -> i32;
    pub fn CryptUnprotectData(
        pDataIn: *const DATA_BLOB,
        ppszDataDescr: *mut *mut u16,
        pOptionalEntropy: *const DATA_BLOB,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DATA_BLOB,
    ) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    pub fn ShellExecuteW(
        hwnd: isize,
        lpOperation: *const u16,
        lpFile: *const u16,
        lpParameters: *const u16,
        lpDirectory: *const u16,
        nShowCmd: i32,
    ) -> isize;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    pub fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(target_os = "windows")]
pub fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut in_blob = DATA_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DATA_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let res = unsafe {
        CryptProtectData(
            &mut in_blob,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        )
    };

    if res != 0 && !out_blob.pbData.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
        let result = slice.to_vec();
        unsafe {
            std::ptr::write_bytes(out_blob.pbData, 0, out_blob.cbData as usize);
            LocalFree(out_blob.pbData as _);
        };
        Some(result)
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn dpapi_protect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "windows")]
pub fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut in_blob = DATA_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DATA_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let res = unsafe {
        CryptUnprotectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        )
    };

    if res != 0 && !out_blob.pbData.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
        let result = slice.to_vec();
        unsafe {
            std::ptr::write_bytes(out_blob.pbData, 0, out_blob.cbData as usize);
            LocalFree(out_blob.pbData as _);
        };
        Some(result)
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn dpapi_unprotect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

pub fn get_secure_token_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let mut primary_path = std::path::PathBuf::from(".litecord_token");
    #[cfg(unix)]
    {
        if let Ok(home) = std::env::var("HOME") {
            let config_dir = std::path::PathBuf::from(home).join(".config").join("litecord");
            let _ = std::fs::create_dir_all(&config_dir);
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o700));
            primary_path = config_dir.join("session.vault");
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let config_dir = std::path::PathBuf::from(appdata).join("Litecord");
            let _ = std::fs::create_dir_all(&config_dir);
            primary_path = config_dir.join("session.vault");
        }
    }
    let fallback_path = std::path::PathBuf::from(".litecord_token");
    (primary_path, fallback_path)
}

#[cfg(not(target_os = "windows"))]
pub fn get_linux_vault_key() -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .unwrap_or_else(|_| "litecord_static_mid_fallback".to_string());
    let user_id = std::env::var("USER")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| "default_user".to_string());
    
    let mut hasher = Sha256::new();
    hasher.update(b"litecord_linux_vault_key_salt_v1_2026");
    hasher.update(machine_id.trim().as_bytes());
    hasher.update(user_id.trim().as_bytes());
    let res = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&res);
    key
}

#[cfg(not(target_os = "windows"))]
pub fn linux_vault_protect(data: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Key, Nonce};
    use rand::RngCore;

    let key_bytes = get_linux_vault_key();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    if let Ok(ciphertext) = cipher.encrypt(nonce, data) {
        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Some(out)
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub fn linux_vault_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Key, Nonce};

    if data.len() < 12 + 16 {
        return None;
    }
    let key_bytes = get_linux_vault_key();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&data[..12]);
    let ciphertext = &data[12..];

    cipher.decrypt(nonce, ciphertext).ok()
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct SavedAccount {
    pub token: String,
    pub user_id: String,
    pub username: String,
    pub tag: String,
    pub avatar_initials: String,
    pub is_active: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct AccountVault {
    pub accounts: Vec<SavedAccount>,
}

pub fn load_account_vault() -> AccountVault {
    let (primary_path, fallback_path) = get_secure_token_paths();
    let raw = match std::fs::read_to_string(&primary_path).or_else(|_| std::fs::read_to_string(&fallback_path)) {
        Ok(s) => s,
        Err(_) => return AccountVault::default(),
    };
    let trimmed = raw.trim();

    let decrypted_str: Option<String> = {
        #[cfg(target_os = "windows")]
        {
            if let Some(rest) = trimmed.strip_prefix("DPAPI:") {
                if let Ok(encrypted_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, rest) {
                    if let Some(decrypted_bytes) = dpapi_unprotect(&encrypted_bytes) {
                        String::from_utf8(decrypted_bytes).ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Some(rest) = trimmed.strip_prefix("LNX_VAULT:") {
                if let Ok(encrypted_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, rest) {
                    if let Some(decrypted_bytes) = linux_vault_unprotect(&encrypted_bytes) {
                        String::from_utf8(decrypted_bytes).ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        }
    };

    let content = decrypted_str.unwrap_or_else(|| trimmed.to_string());
    if content.is_empty() {
        return AccountVault::default();
    }

    if let Ok(vault) = serde_json::from_str::<AccountVault>(&content) {
        return vault;
    }

    let clean = content.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
    if is_valid_token_chars(&clean) {
        AccountVault {
            accounts: vec![SavedAccount {
                token: clean,
                user_id: String::new(),
                username: "Conta 1".to_string(),
                tag: "Conta 1".to_string(),
                avatar_initials: "C".to_string(),
                is_active: true,
            }],
        }
    } else {
        AccountVault::default()
    }
}

pub fn save_account_vault(vault: &AccountVault) {
    let (primary_path, fallback_path) = get_secure_token_paths();
    let json_str = match serde_json::to_string(vault) {
        Ok(s) => s,
        Err(_) => return,
    };

    #[cfg(target_os = "windows")]
    {
        if let Some(encrypted) = dpapi_protect(json_str.as_bytes()) {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &encrypted);
            let payload = format!("DPAPI:{}", b64);
            let _ = std::fs::write(&primary_path, &payload);
            let _ = std::fs::write(&fallback_path, &payload);
        } else {
            let _ = std::fs::write(&primary_path, &json_str);
            let _ = std::fs::write(&fallback_path, &json_str);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(encrypted) = linux_vault_protect(json_str.as_bytes()) {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &encrypted);
            let payload = format!("LNX_VAULT:{}", b64);
            let _ = std::fs::write(&primary_path, &payload);
            let _ = std::fs::write(&fallback_path, &payload);
        } else {
            let _ = std::fs::write(&primary_path, &json_str);
            let _ = std::fs::write(&fallback_path, &json_str);
        }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&primary_path, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::set_permissions(&fallback_path, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn save_or_update_account(token_str: &str, user_id: &str, username: &str, tag: &str) -> AccountVault {
    let mut vault = load_account_vault();
    for acc in &mut vault.accounts {
        acc.is_active = false;
    }

    let initials = if !username.is_empty() {
        username.chars().take(2).collect::<String>().to_uppercase()
    } else if !tag.is_empty() {
        tag.chars().take(2).collect::<String>().to_uppercase()
    } else {
        "U".to_string()
    };

    if let Some(existing) = vault.accounts.iter_mut().find(|a| (!user_id.is_empty() && a.user_id == user_id) || a.token == token_str) {
        existing.token = token_str.to_string();
        if !user_id.is_empty() { existing.user_id = user_id.to_string(); }
        if !username.is_empty() { existing.username = username.to_string(); }
        if !tag.is_empty() { existing.tag = tag.to_string(); }
        existing.avatar_initials = initials;
        existing.is_active = true;
    } else {
        vault.accounts.push(SavedAccount {
            token: token_str.to_string(),
            user_id: user_id.to_string(),
            username: username.to_string(),
            tag: tag.to_string(),
            avatar_initials: initials,
            is_active: true,
        });
    }

    save_account_vault(&vault);
    vault
}

pub fn remove_single_account(user_id_or_token: &str) -> AccountVault {
    let mut vault = load_account_vault();
    vault.accounts.retain(|a| a.user_id != user_id_or_token && a.token != user_id_or_token);
    save_account_vault(&vault);
    vault
}

pub fn delete_secure_token() {
    let (primary_path, fallback_path) = get_secure_token_paths();
    for path in [&primary_path, &fallback_path] {
        if path.exists() {
            if let Ok(metadata) = std::fs::metadata(path) {
                let len = metadata.len() as usize;
                if len > 0 {
                    let zeroes = vec![0u8; len];
                    let _ = std::fs::write(path, &zeroes);
                }
            }
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn sync_ui_saved_accounts(app_weak: &slint::Weak<AppWindow>, vault: &AccountVault) {
    let app_w = app_weak.clone();
    let accounts = vault.accounts.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w.upgrade() {
            let model: Vec<AccountItem> = accounts.iter().map(|acc| {
                AccountItem {
                    id: if !acc.user_id.is_empty() { acc.user_id.clone().into() } else { acc.token.clone().into() },
                    username: acc.username.clone().into(),
                    tag: acc.tag.clone().into(),
                    avatar_initials: acc.avatar_initials.clone().into(),
                    is_active: acc.is_active,
                }
            }).collect();
            ui.set_saved_accounts(slint::ModelRc::new(slint::VecModel::from(model)));
        }
    });
}

#[allow(dead_code)]
pub fn save_secure_token(token_str: &str) {
    save_or_update_account(token_str, "", "", "");
}


pub fn load_secure_token() -> Option<String> {
    let vault = load_account_vault();
    if let Some(acc) = vault.accounts.iter().find(|a| a.is_active) {
        Some(acc.token.clone())
    } else if let Some(first) = vault.accounts.first() {
        Some(first.token.clone())
    } else {
        None
    }
}

pub fn is_valid_token_chars(token: &str) -> bool {
    token.len() >= 50 && token.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
}


pub fn auto_detect_discord_tokens() -> Vec<String> {
    use base64::Engine;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let mut tokens = Vec::new();
    let appdata = match std::env::var("APPDATA") {
        Ok(v) => v,
        Err(_) => return tokens,
    };
    let temp_dir = std::env::temp_dir();

    let discord_paths = vec![
        format!("{}/discord", appdata),
        format!("{}/discordcanary", appdata),
        format!("{}/discordptb", appdata),
        format!("{}/Lightcord", appdata),
    ];

    for path in &discord_paths {
        let local_state_path = format!("{}/Local State", path);
        if let Ok(content) = std::fs::read_to_string(&local_state_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(enc_key_b64) = v["os_crypt"]["encrypted_key"].as_str() {
                    if let Ok(key_raw) = base64::engine::general_purpose::STANDARD.decode(enc_key_b64) {
                        if key_raw.starts_with(b"DPAPI") {
                            if let Some(master_key) = dpapi_unprotect(&key_raw[5..]) {
                                if master_key.len() == 32 {
                                    let leveldb_dir = format!("{}/Local Storage/leveldb", path);
                                    if let Ok(entries) = std::fs::read_dir(&leveldb_dir) {
                                        let mut files: Vec<std::path::PathBuf> = entries
                                            .flatten()
                                            .map(|e| e.path())
                                            .filter(|p| {
                                                let fname = p.file_name().unwrap_or_default().to_string_lossy();
                                                fname.ends_with(".ldb") || fname.ends_with(".log")
                                            })
                                            .collect();

                                        files.sort_by(|a, b| {
                                            let time_a = a.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                            let time_b = b.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                            time_b.cmp(&time_a)
                                        });

                                        for file_path in files {
                                            let fname = file_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                                            let temp_file = temp_dir.join(format!("litecord_tmp_{}", fname));
                                            if std::fs::copy(&file_path, &temp_file).is_ok() {
                                                if let Ok(bytes) = std::fs::read(&temp_file) {
                                                    let _ = std::fs::remove_file(&temp_file);
                                                    let text = String::from_utf8_lossy(&bytes);
                                                    for chunk in text.split("dQw4w9WgXcQ:") {
                                                        let enc_b64: String = chunk
                                                            .chars()
                                                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
                                                            .collect();
                                                        if enc_b64.len() > 30 {
                                                            if let Ok(full_enc) = base64::engine::general_purpose::STANDARD.decode(&enc_b64) {
                                                                if full_enc.len() > 31 {
                                                                    let nonce = &full_enc[3..15];
                                                                    let ciphertext = &full_enc[15..];
                                                                    if let Ok(cipher) = Aes256Gcm::new_from_slice(&master_key) {
                                                                        let nonce_obj = Nonce::from_slice(nonce);
                                                                        if let Ok(decrypted) = cipher.decrypt(nonce_obj, ciphertext) {
                                                                            if let Ok(token) = String::from_utf8(decrypted) {
                                                                                let token_clean = token.trim().to_string();
                                                                                if is_valid_token_chars(&token_clean) && !tokens.contains(&token_clean) {
                                                                                    info!("Candidato a token do Discord encontrado!");
                                                                                    tokens.push(token_clean);
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    let _ = std::fs::remove_file(&temp_file);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    tokens
}

