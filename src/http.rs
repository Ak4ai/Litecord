use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use log::{info, warn, error};

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
        // Same client properties the Gateway IDENTIFY sends (see utils::client_identity).
        if let Ok(props) = HeaderValue::from_str(&crate::utils::client_identity::super_properties_header()) {
            headers.insert("x-super-properties", props);
        }

        let builder = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(crate::utils::client_identity::user_agent());
        let client = crate::utils::apply_proxy_to_builder(builder)
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

    pub async fn get_user_settings(&self) -> Result<serde_json::Value, String> {
        let url = "https://discord.com/api/v10/users/@me/settings";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(settings) = resp.json::<serde_json::Value>().await {
                        return Ok(settings);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn get_user_dms(&self) -> Result<Vec<serde_json::Value>, String> {
        let url = "https://discord.com/api/v10/users/@me/channels";
        match self.client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(dms) = resp.json::<Vec<serde_json::Value>>().await {
                        return Ok(dms);
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

    pub async fn get_channel_permissions(&self, guild_id: &str) -> Result<crate::channel_permissions::ChannelPermissions, String> {
        static PERMISSIONS_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, crate::channel_permissions::ChannelPermissions)>>> = std::sync::OnceLock::new();
        let cache = PERMISSIONS_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        if let Ok(guard) = cache.lock() {
            if let Some((inserted, perms)) = guard.get(guild_id) {
                // Cache permissions for 5 minutes per guild to eliminate redundant REST calls
                if inserted.elapsed() < std::time::Duration::from_secs(300) {
                    return Ok(perms.clone());
                }
            }
        }

        let user_id = {
            let my_uid = crate::gateway::get_my_user_id();
            if my_uid != 0 {
                my_uid.to_string()
            } else {
                let user = self.get_current_user().await?;
                user["id"].as_str().ok_or("Missing current user id")?.to_string()
            }
        };
        let guild_url = format!("https://discord.com/api/v10/guilds/{}", guild_id);
        let member_url = format!("https://discord.com/api/v10/guilds/{}/members/{}", guild_id, user_id);
        let (guild, member) = tokio::try_join!(
            self.get_permission_data(&guild_url), self.get_permission_data(&member_url)
        )?;
        let perms = crate::channel_permissions::ChannelPermissions::new(&guild, &member, &user_id)?;
        if let Ok(mut guard) = cache.lock() {
            guard.insert(guild_id.to_string(), (std::time::Instant::now(), perms.clone()));
        }
        Ok(perms)
    }

    async fn get_permission_data(&self, url: &str) -> Result<serde_json::Value, String> {
        let response = self.client.get(url).send().await.map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Permission lookup HTTP {}", response.status()));
        }
        response.json().await.map_err(|e| e.to_string())
    }

    pub async fn get_forum_threads_active(&self, channel_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/threads/active", channel_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(obj) = resp.json::<serde_json::Value>().await {
                        if let Some(threads) = obj.get("threads").and_then(|t| t.as_array()) {
                            return Ok(threads.clone());
                        }
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn get_channel_messages(&self, channel_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages?limit=30", channel_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(msgs) = resp.json::<Vec<serde_json::Value>>().await {
                        if !msgs.is_empty() {
                            return Ok(msgs);
                        }
                    }
                }
                // Fallback for Forum channels (Type 15) which do not have direct messages but have active threads
                if status.as_u16() == 400 || status.is_success() {
                    if let Ok(threads) = self.get_forum_threads_active(channel_id).await {
                        if !threads.is_empty() {
                            let futures: Vec<_> = threads.iter().take(15).map(|th| {
                                let tid = th["id"].as_str().unwrap_or("").to_string();
                                let tname = th["name"].as_str().unwrap_or("Oferta / Post").to_string();
                                let total_msgs = th.get("message_count").and_then(|m| m.as_u64()).unwrap_or(0);
                                let client = self.client.clone();
                                let channel_id = channel_id.to_string();
                                async move {
                                    // Fetch the original post's first message to get real embeds and links
                                    let msg_url = format!("https://discord.com/api/v10/channels/{}/messages?limit=1", tid);
                                    let mut real_msg: Option<serde_json::Value> = None;
                                    if let Ok(resp_th) = client.get(&msg_url).send().await {
                                        if resp_th.status().is_success() {
                                            if let Ok(mut th_msgs) = resp_th.json::<Vec<serde_json::Value>>().await {
                                                if !th_msgs.is_empty() {
                                                    real_msg = Some(th_msgs.remove(0));
                                                }
                                            }
                                        }
                                    }

                                    if let Some(mut m) = real_msg {
                                        let cur_content = m["content"].as_str().unwrap_or("");
                                        let new_content = if cur_content.is_empty() {
                                            format!("📌 **{}**", tname)
                                        } else {
                                            format!("📌 **{}**\n{}", tname, cur_content)
                                        };
                                        m["content"] = serde_json::Value::String(new_content);
                                        m
                                    } else {
                                        serde_json::json!({
                                            "id": tid,
                                            "channel_id": channel_id,
                                            "author": {
                                                "id": "forum_bot",
                                                "username": "🎮 Oferta",
                                                "global_name": "🎮 Oferta",
                                                "avatar": null
                                            },
                                            "content": format!("📌 **{}**\n💬 {} respostas nesta oferta.", tname, total_msgs),
                                            "type": 0,
                                            "timestamp": chrono::Utc::now().to_rfc3339()
                                        })
                                    }
                                }
                            }).collect();

                            let synth_msgs = futures_util::future::join_all(futures).await;
                            return Ok(synth_msgs);
                        }
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP: {:?}", e)),
        }
    }

    pub async fn get_channel_messages_before(&self, channel_id: &str, before_msg_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://discord.com/api/v10/channels/{}/messages?limit=30&before={}", channel_id, before_msg_id);
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

    pub async fn get_guild_application_command_index(&self, guild_id: &str) -> Result<serde_json::Value, String> {
        let url = format!("https://discord.com/api/v9/guilds/{}/application-command-index", guild_id);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        return Ok(data);
                    }
                }
                Err(format!("Status HTTP {}", status))
            }
            Err(e) => Err(format!("Erro HTTP ao buscar índice de comandos do servidor: {:?}", e)),
        }
    }

    pub async fn send_slash_command_interaction(
        &self,
        guild_id: &str,
        channel_id: &str,
        app_id: &str,
        cmd_id: &str,
        cmd_name: &str,
        version: &str,
        options: Vec<serde_json::Value>,
        session_id: &str,
    ) -> Result<(), String> {
        let url = "https://discord.com/api/v9/interactions";
        let nonce = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
        
        let payload = json!({
            "type": 2,
            "application_id": app_id,
            "guild_id": guild_id,
            "channel_id": channel_id,
            "session_id": if session_id.is_empty() { "a0000000000000000000000000000000" } else { session_id },
            "data": {
                "version": if version.is_empty() { cmd_id } else { version },
                "id": cmd_id,
                "name": cmd_name,
                "type": 1,
                "options": options
            },
            "nonce": nonce
        });

        match self.client.post(url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("Slash command interaction '/{}' enviada com sucesso!", cmd_name);
                    Ok(())
                } else {
                    let err = format!("Status de erro HTTP na interação: {}", resp.status());
                    error!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Falha de rede ao enviar slash command interaction: {:?}", e);
                error!("{}", err);
                Err(err)
            }
        }
    }

    pub async fn ring_dm_call(&self, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v9/channels/{}/call/ring", channel_id);
        let payload = json!({ "recipients": serde_json::Value::Null });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("🔔 Chamada de DM tocando no canal {}...", channel_id);
                    Ok(())
                } else {
                    let err = format!("Status de erro HTTP ao iniciar toque da DM: {}", resp.status());
                    warn!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Falha de rede ao iniciar toque da DM: {:?}", e);
                warn!("{}", err);
                Err(err)
            }
        }
    }

    pub async fn stop_ringing_dm_call(&self, channel_id: &str) -> Result<(), String> {
        let url = format!("https://discord.com/api/v9/channels/{}/call/stop-ringing", channel_id);
        let payload = json!({ "recipients": serde_json::Value::Null });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() || resp.status().as_u16() == 204 {
                    info!("🔕 Toque da chamada de DM interrompido no canal {}.", channel_id);
                    Ok(())
                } else {
                    let err = format!("Status de erro HTTP ao parar toque da DM: {}", resp.status());
                    warn!("{}", err);
                    Err(err)
                }
            }
            Err(e) => {
                let err = format!("Falha de rede ao parar toque da DM: {:?}", e);
                warn!("{}", err);
                Err(err)
            }
        }
    }
}
