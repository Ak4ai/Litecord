use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use log::{info, error};

#[derive(Clone)]
pub struct DiscordHttpClient {
    client: reqwest::Client,
    #[allow(dead_code)]
    token: String,
}

impl DiscordHttpClient {
    pub fn new(token: String) -> Self {
        let clean_token = token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&clean_token).unwrap_or(HeaderValue::from_static("")));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Discord/1.0.9000 Chrome/120.0.6099.291 Electron/28.2.10 Safari/537.36")
            .build()
            .unwrap_or_default();

        Self { client, token: clean_token }
    }

    pub async fn get_current_user(&self) -> Result<serde_json::Value, String> {
        let url = "https://discord.com/api/v10/users/@me";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        return Ok(json);
                    }
                    Err("Falha ao ler JSON da resposta".to_string())
                } else if status.as_u16() == 401 {
                    Err("HTTP 401 Unauthorized: Token recusado pelo Discord!".to_string())
                } else {
                    Err(format!("Status HTTP {}", status))
                }
            }
            Err(e) => Err(format!("Erro de rede HTTP: {:?}", e)),
        }
    }

    pub async fn get_user_guilds(&self) -> Result<Vec<serde_json::Value>, String> {
        let url = "https://discord.com/api/v10/users/@me/guilds";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(guilds) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(guilds);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn get_guild_channels(&self, guild_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/channels", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(channels) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(channels);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn get_channel_messages(&self, channel_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages?limit=50", channel_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(msgs) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(msgs);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn download_image(&self, url: &str) -> Result<Vec<u8>, String> {
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        return Ok(bytes.to_vec());
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages", channel_id);
        let payload = json!({ "content": content });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Mensagem enviada com sucesso para o canal {}", channel_id);
                    Ok(())
                } else {
                    let err = format!("Status de erro HTTP: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Falha ao enviar mensagem via HTTP: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    #[allow(dead_code)]
    pub async fn update_my_voice_state(&self, guild_id: &str, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/voice-states/@me", guild_id);
        let payload = json!({
            "channel_id": channel_id,
            "suppress": false
        });

        match self.client.patch(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Voice State REST @me atualizado com SUCESSO no servidor {}!", guild_id);
                    Ok(())
                } else {
                    let err = format!("Status de erro HTTP Voice State: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Falha ao atualizar Voice State via HTTP: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    pub async fn get_user_profile(&self, user_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v10/users/{}", user_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(user) = resp.json::<serde_json::Value>().await {
                        return Ok(user);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP ao buscar usuário: {:?}", e)),
        }
    }

    pub async fn get_guild_member(&self, guild_id: &str, user_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/members/{}", guild_id, user_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(member) = resp.json::<serde_json::Value>().await {
                        return Ok(member);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP ao buscar membro do servidor: {:?}", e)),
        }
    }

    pub async fn get_guild_members(&self, guild_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/guilds/{}/members?limit=1000", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(members) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(members);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP ao buscar membros do servidor: {:?}", e)),
        }
    }
}
