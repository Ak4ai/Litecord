use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use log::{info, error};

pub const NETWORK_SETTINGS_FILE: &str = ".litecord_network_settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// "off" | "http" | "socks5" | "system"
    pub proxy_mode: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub proxy_password: String,
    /// Se ativado, tenta rotear chamadas de mídia; por padrão falso (mantém UDP direto para mínima latência)
    pub route_media: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            proxy_mode: "off".to_string(),
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 8080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            route_media: false,
        }
    }
}

static NETWORK_SETTINGS: OnceLock<Mutex<NetworkSettings>> = OnceLock::new();

pub fn get_network_settings_store() -> &'static Mutex<NetworkSettings> {
    NETWORK_SETTINGS.get_or_init(|| {
        let settings = if let Ok(data) = std::fs::read_to_string(NETWORK_SETTINGS_FILE) {
            serde_json::from_str::<NetworkSettings>(&data).unwrap_or_default()
        } else {
            NetworkSettings::default()
        };
        info!("🌐 Configurações de Rede e Proxy carregadas: modo={}", settings.proxy_mode);
        Mutex::new(settings)
    })
}

pub fn get_network_settings() -> NetworkSettings {
    get_network_settings_store().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn save_network_settings(settings: &NetworkSettings) {
    {
        let mut store = get_network_settings_store().lock().unwrap_or_else(|e| e.into_inner());
        *store = settings.clone();
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(NETWORK_SETTINGS_FILE, json);
    }
}

pub fn update_network_settings(
    mode: String,
    host: String,
    port: u16,
    user: String,
    pass: String,
    route_media: bool,
) {
    let settings = NetworkSettings {
        proxy_mode: mode,
        proxy_host: host,
        proxy_port: port,
        proxy_username: user,
        proxy_password: pass,
        route_media,
    };
    save_network_settings(&settings);
    info!("🌐 Configurações de Proxy salvas: modo={}, host={}:{}", settings.proxy_mode, settings.proxy_host, settings.proxy_port);
}

/// Aplica a configuração de proxy ativa a qualquer `reqwest::ClientBuilder`
pub fn apply_proxy_to_builder(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let settings = get_network_settings();
    match settings.proxy_mode.as_str() {
        "http" | "https" => {
            let url = format!("http://{}:{}", settings.proxy_host.trim(), settings.proxy_port);
            match reqwest::Proxy::all(&url) {
                Ok(mut p) => {
                    if !settings.proxy_username.is_empty() {
                        p = p.basic_auth(&settings.proxy_username, &settings.proxy_password);
                    }
                    info!("🌐 Aplicando Proxy HTTP/HTTPS: {}", url);
                    builder = builder.proxy(p);
                }
                Err(e) => {
                    error!("❌ Falha ao configurar Proxy HTTP ({}): {:?}", url, e);
                }
            }
        }
        "socks5" => {
            let url = format!("socks5://{}:{}", settings.proxy_host.trim(), settings.proxy_port);
            match reqwest::Proxy::all(&url) {
                Ok(mut p) => {
                    if !settings.proxy_username.is_empty() {
                        p = p.basic_auth(&settings.proxy_username, &settings.proxy_password);
                    }
                    info!("🌐 Aplicando Proxy SOCKS5: {}", url);
                    builder = builder.proxy(p);
                }
                Err(e) => {
                    error!("❌ Falha ao configurar Proxy SOCKS5 ({}): {:?}", url, e);
                }
            }
        }
        "system" => {
            info!("🌐 Utilizando Proxy do Sistema Operacional.");
            // reqwest consulta variáveis de ambiente do sistema (HTTP_PROXY / HTTPS_PROXY / ALL_PROXY)
        }
        _ => {
            // "off" (Direct) -> Desativa qualquer proxy de ambiente
            builder = builder.no_proxy();
        }
    }
    builder
}

/// Constrói um `reqwest::Client` genérico com o proxy e user-agent configurados
pub fn build_generic_http_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Discord/1.0.9000 Chrome/120.0.6099.291 Electron/28.2.10 Safari/537.36");
    apply_proxy_to_builder(builder).build().unwrap_or_default()
}

/// Testa a conexão do proxy tentando bater na API pública do Discord
pub async fn test_proxy_connection() -> Result<String, String> {
    let client = build_generic_http_client();
    let url = "https://discord.com/api/v10/gateway";
    let start = std::time::Instant::now();
    match client.get(url).timeout(std::time::Duration::from_secs(6)).send().await {
        Ok(resp) => {
            let latency_ms = start.elapsed().as_millis();
            if resp.status().is_success() {
                Ok(format!("Conexão bem-sucedida! Latência: {}ms (Status: {})", latency_ms, resp.status()))
            } else {
                Ok(format!("Proxy respondeu em {}ms, mas com status HTTP {}", latency_ms, resp.status()))
            }
        }
        Err(e) => {
            Err(format!("Falha ao conectar via Proxy: {}", e))
        }
    }
}
