use std::path::PathBuf;
use tokio::sync::mpsc;
use log::info;
use serde::{Deserialize, Serialize};

const REPO_OWNER: &str = "Ak4ai";
const REPO_NAME: &str = "Litecord";
const CONFIG_FILE: &str = ".litecord_updater_config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version: String,
    pub download_url: String,
    pub release_name: String,
    pub release_body: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct UpdaterConfig {
    ignored_version: Option<String>,
}

fn get_config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE)
}

pub fn is_version_ignored(tag: &str) -> bool {
    let path = get_config_path();
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<UpdaterConfig>(&data) {
                if let Some(ref ignored) = cfg.ignored_version {
                    return ignored == tag;
                }
            }
        }
    }
    false
}

pub fn save_ignored_version(tag: &str) {
    let cfg = UpdaterConfig {
        ignored_version: Some(tag.to_string()),
    };
    if let Ok(data) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(get_config_path(), data);
        info!("🏷️ Versão {} marcada como ignorada em {}", tag, CONFIG_FILE);
    }
}

/// Compares version string a with b (e.g., "0.2.2" > "0.2.1")
pub fn is_newer_version(remote_ver: &str, current_ver: &str) -> bool {
    let parse_ver = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };

    let remote_parts = parse_ver(remote_ver);
    let current_parts = parse_ver(current_ver);

    let max_len = remote_parts.len().max(current_parts.len());
    for i in 0..max_len {
        let r = remote_parts.get(i).copied().unwrap_or(0);
        let c = current_parts.get(i).copied().unwrap_or(0);
        if r > c {
            return true;
        } else if r < c {
            return false;
        }
    }
    false
}

/// Checks GitHub API for the latest release (respecting ignored versions)
pub async fn check_for_updates() -> Option<ReleaseInfo> {
    check_for_updates_internal(true).await.ok().flatten()
}

/// Manually checks GitHub API for the latest release (ignoring the skip list)
pub async fn check_for_updates_manual() -> Result<Option<ReleaseInfo>, String> {
    check_for_updates_internal(false).await
}

async fn check_for_updates_internal(respect_ignored: bool) -> Result<Option<ReleaseInfo>, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let url = format!("https://api.github.com/repos/{}/{}/releases/latest", REPO_OWNER, REPO_NAME);

    info!("🔍 Verificando se há novas versões do Litecord no GitHub (Atual: v{})...", current_version);

    let client = reqwest::Client::builder()
        .user_agent("Litecord-App-Updater")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Erro ao inicializar cliente HTTP: {:?}", e))?;

    let resp = client.get(&url).send().await
        .map_err(|e| format!("Falha de conexão com o GitHub: {:?}", e))?;

    if !resp.status().is_success() {
        return Err(format!("API do GitHub retornou status {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Erro ao decodificar JSON do GitHub: {:?}", e))?;

    let tag_name = match json["tag_name"].as_str() {
        Some(t) => t.to_string(),
        None => return Err("Formato de release inválido no GitHub.".to_string()),
    };
    let clean_ver = tag_name.trim_start_matches('v').to_string();

    if respect_ignored && is_version_ignored(&tag_name) {
        info!("Versão remota {} está marcada como ignorada pelo usuário.", tag_name);
        return Ok(None);
    }

    if !is_newer_version(&clean_ver, current_version) {
        info!("O Litecord já está na versão mais recente (v{}).", current_version);
        return Ok(None);
    }

    info!("🚀 NOVA VERSÃO ENCONTRADA: {} (Atual: v{})!", tag_name, current_version);

    // Target asset selection based on OS
    let target_asset_name = if cfg!(target_os = "windows") {
        "Litecord-Setup-x64.exe"
    } else {
        "litecord-linux-x64.tar.gz"
    };

    let mut download_url = String::new();
    if let Some(assets) = json["assets"].as_array() {
        for asset in assets {
            if let Some(name) = asset["name"].as_str() {
                if name == target_asset_name {
                    if let Some(durl) = asset["browser_download_url"].as_str() {
                        download_url = durl.to_string();
                        break;
                    }
                }
            }
        }
    }

    // Fallback URL if asset not matched directly
    if download_url.is_empty() {
        download_url = format!(
            "https://github.com/{}/{}/releases/download/{}/{}",
            REPO_OWNER, REPO_NAME, tag_name, target_asset_name
        );
    }

    Ok(Some(ReleaseInfo {
        tag_name: tag_name.clone(),
        version: clean_ver,
        download_url,
        release_name: json["name"].as_str().unwrap_or(&tag_name).to_string(),
        release_body: json["body"].as_str().unwrap_or("").to_string(),
    }))
}

/// Downloads the release file and launches the updater/installer
pub async fn download_and_install_update(
    download_url: String,
    progress_tx: mpsc::Sender<f32>,
) -> Result<(), String> {
    info!("Iniciando download da atualização: {}", download_url);
    if !download_url.starts_with("https://github.com/Ak4ai/Litecord/")
        && !download_url.starts_with("https://objects.githubusercontent.com/")
        && !download_url.starts_with("https://github-releases.githubusercontent.com/")
    {
        return Err("URL de atualização rejeitada por segurança: domínio não oficial.".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("Litecord-App-Updater")
        .build()
        .map_err(|e| format!("Erro ao criar cliente HTTP: {:?}", e))?;

    let mut response = client.get(&download_url).send().await
        .map_err(|e| format!("Falha na conexão de download: {:?}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download falhou com status HTTP {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(0);
    let temp_dir = std::env::temp_dir();

    if cfg!(target_os = "windows") {
        let installer_path = temp_dir.join("Litecord-Update-Setup.exe");
        let mut file = tokio::fs::File::create(&installer_path).await
            .map_err(|e| format!("Falha ao criar arquivo temporário: {:?}", e))?;

        let mut downloaded: u64 = 0;

        while let Some(chunk) = response.chunk().await.map_err(|e| format!("Erro no download: {:?}", e))? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await
                .map_err(|e| format!("Erro ao gravar dados: {:?}", e))?;

            downloaded += chunk.len() as u64;
            if total_size > 0 {
                let progress = (downloaded as f32 / total_size as f32).clamp(0.0, 1.0);
                let _ = progress_tx.try_send(progress);
            }
        }

        tokio::io::AsyncWriteExt::flush(&mut file).await
            .map_err(|e| format!("Erro no flush do arquivo: {:?}", e))?;
        drop(file);

        let _ = progress_tx.try_send(1.0);
        info!("Download concluído! Executando instalador: {:?}", installer_path);

        // Run installer and terminate current application cleanly
        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            let installer_str = installer_path.to_string_lossy().to_string();

            // Run detached via cmd with a 1-second delay so this process terminates and unlocks all files
            let res = Command::new("cmd")
                .args([
                    "/c",
                    "timeout",
                    "/t",
                    "1",
                    "/nobreak",
                    ">nul",
                    "&",
                    "start",
                    "",
                    &installer_str,
                    "/SP-",
                    "/SILENT",
                    "/FORCECLOSEAPPLICATIONS",
                    "/RESTARTAPPLICATIONS",
                ])
                .spawn();

            match res {
                Ok(_) => {
                    info!("Instalador agendado com sucesso. Encerrando processo do Litecord para atualização...");
                    unsafe {
                        windows_sys::Win32::System::Threading::TerminateProcess(
                            windows_sys::Win32::System::Threading::GetCurrentProcess(),
                            0,
                        );
                    }
                    std::process::exit(0);
                }
                Err(_e) => {
                    // Fallback direct spawn
                    let _ = Command::new(&installer_path)
                        .args(["/SP-", "/FORCECLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
                        .spawn();
                    std::process::exit(0);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(())
        }
    } else {
        // Linux: Download .tar.gz and extract binary to ~/.local/bin/litecord
        let tar_path = temp_dir.join("litecord-update.tar.gz");
        let mut file = tokio::fs::File::create(&tar_path).await
            .map_err(|e| format!("Falha ao criar arquivo temporário: {:?}", e))?;

        let mut downloaded: u64 = 0;

        while let Some(chunk) = response.chunk().await.map_err(|e| format!("Erro no download: {:?}", e))? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await
                .map_err(|e| format!("Erro ao gravar dados: {:?}", e))?;

            downloaded += chunk.len() as u64;
            if total_size > 0 {
                let progress = (downloaded as f32 / total_size as f32).clamp(0.0, 1.0);
                let _ = progress_tx.try_send(progress);
            }
        }

        tokio::io::AsyncWriteExt::flush(&mut file).await
            .map_err(|e| format!("Erro no flush do arquivo: {:?}", e))?;
        drop(file);

        let _ = progress_tx.try_send(1.0);
        info!("Download concluído! Atualizando binário Linux...");

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let install_bin_dir = PathBuf::from(&home).join(".local/bin");
        let _ = std::fs::create_dir_all(&install_bin_dir);

        // Extract tar.gz in temp directory
        let extract_dir = temp_dir.join("litecord_extracted");
        let _ = std::fs::remove_dir_all(&extract_dir);
        let _ = std::fs::create_dir_all(&extract_dir);

        let extract_res = std::process::Command::new("tar")
            .args(["-xzf", tar_path.to_str().unwrap(), "-C", extract_dir.to_str().unwrap()])
            .status();

        if let Ok(status) = extract_res {
            if status.success() {
                let new_bin = extract_dir.join("litecord");
                let target_bin = install_bin_dir.join("litecord");
                if new_bin.exists() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755));
                    }

                    // On Linux/Unix, replacing a running binary in-place with fs::copy fails with ETXTBSY (Text file busy).
                    // We write to a temporary file in the same directory and use atomic rename (or remove_file + copy).
                    let temp_target = install_bin_dir.join(format!(".litecord_update_{}", std::process::id()));
                    let mut updated = false;
                    if let Ok(_) = std::fs::copy(&new_bin, &temp_target) {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(&temp_target, std::fs::Permissions::from_mode(0o755));
                        }
                        if std::fs::rename(&temp_target, &target_bin).is_ok() {
                            updated = true;
                        }
                    }

                    if !updated {
                        // Fallback: unlink running destination first, then copy
                        let _ = std::fs::remove_file(&target_bin);
                        if let Ok(_) = std::fs::copy(&new_bin, &target_bin) {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(&target_bin, std::fs::Permissions::from_mode(0o755));
                            }
                            updated = true;
                        }
                    }

                    // Also update the currently running executable location if it's outside ~/.local/bin
                    let mut bin_to_spawn = target_bin.clone();
                    if let Ok(curr_exe) = std::env::current_exe() {
                        if curr_exe.exists() {
                            if curr_exe != target_bin {
                                if let Some(parent) = curr_exe.parent() {
                                    let curr_temp = parent.join(format!(".litecord_currexe_update_{}", std::process::id()));
                                    if let Ok(_) = std::fs::copy(&new_bin, &curr_temp) {
                                        #[cfg(unix)]
                                        {
                                            use std::os::unix::fs::PermissionsExt;
                                            let _ = std::fs::set_permissions(&curr_temp, std::fs::Permissions::from_mode(0o755));
                                        }
                                        if std::fs::rename(&curr_temp, &curr_exe).is_ok() {
                                            bin_to_spawn = curr_exe;
                                        }
                                    }
                                }
                            } else {
                                bin_to_spawn = curr_exe;
                            }
                        }
                    }

                    // Update desktop icon if packaged in archive
                    let new_icon = extract_dir.join("assets/app_icon.png");
                    if new_icon.exists() {
                        let icon_512 = PathBuf::from(&home).join(".local/share/icons/hicolor/512x512/apps/litecord.png");
                        let icon_256 = PathBuf::from(&home).join(".local/share/icons/hicolor/256x256/apps/litecord.png");
                        let icon_pix = PathBuf::from(&home).join(".local/share/pixmaps/litecord.png");
                        let _ = std::fs::create_dir_all(icon_512.parent().unwrap());
                        let _ = std::fs::create_dir_all(icon_256.parent().unwrap());
                        let _ = std::fs::create_dir_all(icon_pix.parent().unwrap());
                        let _ = std::fs::copy(&new_icon, &icon_512);
                        let _ = std::fs::copy(&new_icon, &icon_256);
                        let _ = std::fs::copy(&new_icon, &icon_pix);
                    }

                    info!("✅ Litecord atualizado com sucesso em {:?}! Reiniciando...", bin_to_spawn);
                    let _ = std::process::Command::new(&bin_to_spawn).spawn();
                    std::process::exit(0);
                }
            }
        }
        Err("Falha ao descompactar e instalar a atualização no Linux.".to_string())
    }
}
