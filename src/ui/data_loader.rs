#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use tokio::sync::mpsc;
use log::{info, error};

use crate::{
    AppWindow, GuildItem, ChatMessage, LinkItem,
};
use crate::gateway::{self, GatewayClient, GatewayEvent, GatewayCommand, GuildData, ChannelData, format_discord_author, format_discord_message_parts};
use crate::http::DiscordHttpClient;
use crate::ui::helpers::*;
use crate::auth::vault::*;
use crate::utils::updater;
use crate::utils::emoji_cache;
use slint::{Image, Model};

#[derive(Clone, Debug, Default)]
pub struct RawGuildItem {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub icon_path: Option<String>,
}

pub async fn fetch_and_populate_guilds(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
) {
    info!("Buscando servidores do usuário via REST API...");
    let cache_dir = std::path::Path::new(".litecord_cache/icons");
    let _ = std::fs::create_dir_all(cache_dir);

    match http.get_user_guilds().await {
        Ok(guilds_json) => {
            info!("{} servidores encontrados via REST!", guilds_json.len());
            let mut raw_guilds: Vec<RawGuildItem> = Vec::new();
            let mut first_guild_id_opt: Option<String> = None;
            let mut pending_icon_downloads: Vec<(String, String)> = Vec::new();

            for g in guilds_json {
                let g_id = g["id"].as_str().unwrap_or("").to_string();
                let g_name = g["name"].as_str().unwrap_or("Servidor").to_string();
                let g_icon_hash = g["icon"].as_str().map(|s| s.to_string());

                if !g_id.is_empty() {
                    if first_guild_id_opt.is_none() {
                        first_guild_id_opt = Some(g_id.clone());
                    }

                    let icon_str = get_guild_initials(&g_name);
                    let local_icon_path = cache_dir.join(format!("{}.png", g_id));

                    let mut icon_path_opt = None;
                    if local_icon_path.exists() {
                        icon_path_opt = Some(local_icon_path.to_string_lossy().to_string());
                    } else if let Some(ref hash) = g_icon_hash {
                        let icon_url = format!("https://cdn.discordapp.com/icons/{}/{}.png?size=64", g_id, hash);
                        pending_icon_downloads.push((icon_url, local_icon_path.to_string_lossy().to_string()));
                    }

                    guilds_map.lock().unwrap().insert(g_id.clone(), GuildData {
                        id: g_id.clone(),
                        name: g_name.clone(),
                        channels: Vec::new(),
                    });

                    raw_guilds.push(RawGuildItem {
                        id: g_id,
                        name: g_name,
                        icon: icon_str,
                        icon_path: icon_path_opt,
                    });
                }
            }

            let raw_guilds_clone = raw_guilds.clone();
            let app_w = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    let ui_guilds: Vec<GuildItem> = raw_guilds_clone.into_iter().map(|raw| {
                        let mut has_image = false;
                        let mut icon_image = Image::default();
                        if let Some(ref path_str) = raw.icon_path {
                            if let Ok(img) = Image::load_from_path(std::path::Path::new(path_str)) {
                                has_image = true;
                                icon_image = img;
                            }
                        }
                        GuildItem {
                            id: raw.id.into(),
                            name: raw.name.into(),
                            icon: raw.icon.into(),
                            has_image,
                            icon_image,
                        }
                    }).collect();

                    let model = std::rc::Rc::new(slint::VecModel::from(ui_guilds));
                    ui.set_guilds(model.into());
                }
            });

            // Automatically select the first guild and fetch its channels & messages
            if let Some(first_gid) = first_guild_id_opt {
                fetch_and_populate_channels(
                    http,
                    app_weak.clone(),
                    guilds_map.clone(),
                    active_guild_id.clone(),
                    active_channel_id.clone(),
                    cmd_tx_store.clone(),
                    &first_gid,
                ).await;
            }

            // Download custom server icons asynchronously in the background
            if !pending_icon_downloads.is_empty() {
                let http_dl = http.clone();
                let app_w_dl = app_weak.clone();

                tokio::spawn(async move {
                    for (url, save_path) in pending_icon_downloads {
                        if let Ok(bytes) = http_dl.download_image(&url).await {
                            let _ = std::fs::write(&save_path, &bytes);
                        }
                    }

                    // Reload UI with newly cached icons
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w_dl.upgrade() {
                            let old_guilds = ui.get_guilds();
                            let mut updated = Vec::new();
                            for i in 0..old_guilds.row_count() {
                                if let Some(mut item) = old_guilds.row_data(i) {
                                    let gid = item.id.to_string();
                                    let icon_path = cache_dir.join(format!("{}.png", gid));
                                    if icon_path.exists() {
                                        if let Ok(img) = Image::load_from_path(&icon_path) {
                                            item.has_image = true;
                                            item.icon_image = img;
                                        }
                                    }
                                    updated.push(item);
                                }
                            }
                            let model = std::rc::Rc::new(slint::VecModel::from(updated));
                            ui.set_guilds(model.into());
                        }
                    });
                });
            }
        }
        Err(e) => {
            error!("Erro ao buscar servidores via REST: {}", e);
        }
    }
}

pub async fn fetch_and_populate_channels(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    guild_id: &str,
) {
    info!("Buscando canais do servidor {} via REST API...", guild_id);
    *active_guild_id.lock().unwrap() = guild_id.to_string();

    let g_name = guilds_map.lock().unwrap()
        .get(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Servidor".to_string());

    let app_w_top = app_weak.clone();
    let g_name_top = g_name.clone();
    let g_id_top = guild_id.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = app_w_top.upgrade() {
            ui.set_connection_status(format!("Servidor: {} | Gateway v9 (Online)", g_name_top).into());
            ui.set_active_guild_name(g_name_top.into());
            ui.set_active_guild_id(g_id_top.into());
        }
    });

    match http.get_guild_channels(guild_id).await {
        Ok(chans_json) => {
            info!("{} canais encontrados no servidor!", chans_json.len());

            pub struct RawChan {
                id: String,
                name: String,
                ch_type: u64,
                parent_id: Option<String>,
                position: i64,
            }

            let mut raw_cats: Vec<RawChan> = Vec::new();
            let mut raw_chans: Vec<RawChan> = Vec::new();

            for ch in chans_json {
                let ch_id = ch["id"].as_str().unwrap_or("").to_string();
                let ch_name = ch["name"].as_str().unwrap_or("canal").to_string();
                let ch_type = ch["type"].as_u64().unwrap_or(0);
                let parent_id = ch["parent_id"].as_str().map(|s| s.to_string());
                let position = ch["position"].as_i64().unwrap_or(0);

                if ch_id.is_empty() { continue; }

                if ch_type == 4 {
                    raw_cats.push(RawChan {
                        id: ch_id,
                        name: ch_name.to_uppercase(),
                        ch_type,
                        parent_id: None,
                        position,
                    });
                } else if ch_type == 0 || ch_type == 2 || ch_type == 5 || ch_type == 13 || ch_type == 15 {
                    raw_chans.push(RawChan {
                        id: ch_id,
                        name: ch_name,
                        ch_type,
                        parent_id,
                        position,
                    });
                }
            }

            raw_cats.sort_by_key(|c| c.position);

            let cat_ids: std::collections::HashSet<String> = raw_cats.iter().map(|c| c.id.clone()).collect();
            let mut cat_map: HashMap<String, Vec<RawChan>> = HashMap::new();
            let mut uncategorized: Vec<RawChan> = Vec::new();

            for ch in raw_chans {
                if let Some(ref pid) = ch.parent_id {
                    if cat_ids.contains(pid) {
                        cat_map.entry(pid.clone()).or_default().push(ch);
                        continue;
                    }
                }
                uncategorized.push(ch);
            }

            let sort_channel_list = |list: &mut Vec<RawChan>| {
                list.sort_by(|a, b| {
                    let a_is_voice = a.ch_type == 2 || a.ch_type == 13;
                    let b_is_voice = b.ch_type == 2 || b.ch_type == 13;
                    if a_is_voice == b_is_voice {
                        a.position.cmp(&b.position)
                    } else if !a_is_voice {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                });
            };

            sort_channel_list(&mut uncategorized);
            for children in cat_map.values_mut() {
                sort_channel_list(children);
            }

            let mut channels_data: Vec<ChannelData> = Vec::new();

            for ch in uncategorized {
                let is_voice = ch.ch_type == 2 || ch.ch_type == 13;
                channels_data.push(ChannelData {
                    id: ch.id,
                    name: ch.name,
                    is_voice,
                    is_category: false,
                    parent_id: None,
                    position: ch.position,
                });
            }

            for cat in raw_cats {
                let cat_id = cat.id.clone();
                let children = cat_map.remove(&cat_id).unwrap_or_default();

                channels_data.push(ChannelData {
                    id: cat.id,
                    name: cat.name,
                    is_voice: false,
                    is_category: true,
                    parent_id: None,
                    position: cat.position,
                });

                for ch in children {
                    let is_voice = ch.ch_type == 2 || ch.ch_type == 13;
                    channels_data.push(ChannelData {
                        id: ch.id,
                        name: ch.name,
                        is_voice,
                        is_category: false,
                        parent_id: Some(cat_id.clone()),
                        position: ch.position,
                    });
                }
            }

            // Subscribe via Gateway Opcode 14 for all voice channels in this guild to get live voice_states
            let voice_cids: Vec<String> = channels_data.iter().filter(|c| c.is_voice).map(|c| c.id.clone()).collect();
            if let Some(tx) = cmd_tx_store.lock().unwrap().as_ref() {
                let _ = tx.try_send(GatewayCommand::SubscribeGuild {
                    guild_id: guild_id.to_string(),
                    channel_ids: voice_cids,
                });
            }

            // Update channels in guilds_map
            if let Some(g_data) = guilds_map.lock().unwrap().get_mut(guild_id) {
                g_data.channels = channels_data.clone();
            }

            let ui_channels = {
                let cats_guard = get_collapsed_categories().lock().unwrap().clone();
                build_ui_channels(&channels_data, &cats_guard)
            };

            let app_w = app_weak.clone();
            let ui_channels_clone = ui_channels.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_channels_clone));
                    ui.set_channels(model.into());
                }
            });

            // Fetch and cache all guild members via REST API asynchronously
            if let Ok(members) = http.get_guild_members(guild_id).await {
                for m in members {
                    let uid_str = m["user"]["id"].as_str().unwrap_or("");
                    if let Ok(uid) = uid_str.parse::<u64>() {
                        let mut display_name = String::new();
                        if let Some(nick) = m["nick"].as_str() {
                            if !nick.is_empty() { display_name = nick.to_string(); }
                        }
                        if display_name.is_empty() {
                            if let Some(gname) = m["user"]["global_name"].as_str() {
                                if !gname.is_empty() { display_name = gname.to_string(); }
                            }
                        }
                        if display_name.is_empty() {
                            if let Some(uname) = m["user"]["username"].as_str() {
                                if !uname.is_empty() { display_name = uname.to_string(); }
                            }
                        }
                        if !display_name.is_empty() {
                            gateway::register_user_name(uid, display_name);
                        }
                    }
                }
                info!("✅ Membros e Bots do servidor {} pre-carregados via REST API com SUCESSO!", guild_id);
            }

            // Pre-load application commands for this guild (bots like music bots, 24/7, moderation)
            load_guild_command_index(http, app_weak.clone(), guild_id).await;

            // Try to find the first readable text channel automatically
            let text_channels: Vec<&ChannelData> = channels_data.iter().filter(|c| !c.is_voice && !c.is_category).collect();
            let mut loaded_readable = false;

            for text_ch in text_channels {
                let ch_id = text_ch.id.clone();
                let ch_name = text_ch.name.clone();

                *active_channel_id.lock().unwrap() = ch_id.clone();
                if load_messages_for_channel(http, app_weak.clone(), &ch_id).await {
                    let app_w2 = app_weak.clone();
                    let ch_id_val = ch_id.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_w2.upgrade() {
                            let clean_name = ch_name.trim_start_matches('#').trim();
                            ui.set_active_channel_name(clean_name.into());
                            ui.set_active_channel_id(ch_id_val.into());
                        }
                    });
                    request_chat_scroll_to_bottom(app_weak.clone());
                    loaded_readable = true;
                    break;
                }
            }

            if !loaded_readable {
                *active_channel_id.lock().unwrap() = String::new();
                let app_w_none = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w_none.upgrade() {
                        ui.set_active_channel_name("Nenhum canal de texto acessível".into());
                        ui.set_active_channel_id("".into());
                        let empty_msgs = vec![ChatMessage {
                            id: "".into(),
                            author: "Litecord System".into(),
                            content: "🔒 Este servidor não possui canais de texto acessíveis para a sua conta.".into(),
                            commands: slint::ModelRc::default(),
                            content_lines: slint::ModelRc::default(),
                            embed_content: "".into(),
                            embed_lines: slint::ModelRc::default(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            reply_author: "".into(),
                            reply_content: "".into(),
                            reply_command: "".into(),
                            links: slint::ModelRc::default(),
                            buttons: slint::ModelRc::default(),
                            attachments: slint::ModelRc::default(),
                            timestamp: "Agora".into(),
                        }];
                        let model = std::rc::Rc::new(slint::VecModel::from(empty_msgs));
                        ui.set_messages(model.into());
                        ui.set_has_more_older_messages(false);
                    }
                });
            }
        }
        Err(e) => {
            error!("Erro ao buscar canais do servidor via REST: {}", e);
        }
    }
}

pub async fn load_messages_for_channel(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    channel_id: &str,
) -> bool {
    info!("Carregando mensagens do canal {}...", channel_id);
    emoji_cache::get_emoji_cache().set_active_channel(channel_id);
    let ch_id_for_emojis = channel_id.to_string();
    let app_weak_load = app_weak.clone();
    match http.get_channel_messages(channel_id).await {
        Ok(msgs_val) => {
            let has_more = msgs_val.len() >= 30;
            if let Some(oldest) = msgs_val.last() {
                if let Some(id_str) = oldest["id"].as_str() {
                    get_oldest_message_map().lock().unwrap().insert(channel_id.to_string(), id_str.to_string());
                }
            }

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_has_more_older_messages(has_more);
                    ui.set_is_loading_older_messages(false);

                    let ui_msgs: Vec<ChatMessage> = if msgs_val.is_empty() {
                        vec![ChatMessage {
                            id: "".into(),
                            author: "Litecord System".into(),
                            content: "Este canal está vazio ou não possui mensagens recentes.".into(),
                            commands: slint::ModelRc::default(),
                            content_lines: slint::ModelRc::default(),
                            embed_content: "".into(),
                            embed_lines: slint::ModelRc::default(),
                            embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                            embed_footer: "".into(),
                            code_block: "".into(),
                            reply_author: "".into(),
                            reply_content: "".into(),
                            reply_command: "".into(),
                            links: slint::ModelRc::default(),
                            buttons: slint::ModelRc::default(),
                            attachments: slint::ModelRc::default(),
                            timestamp: "Agora".into(),
                        }]
                    } else {
                        msgs_val.iter().rev().map(|m| {
                            let msg_id = m["id"].as_str().unwrap_or("");
                            let author = format_discord_author(m);
                            let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(m);
                            
                            let slint_cmds: Vec<slint::SharedString> = commands.into_iter().map(|c| c.into()).collect();
                            let commands_model = std::rc::Rc::new(slint::VecModel::from(slint_cmds));

                            let slint_links: Vec<LinkItem> = links.iter().map(|l| LinkItem {
                                label: l.label.clone().into(),
                                url: l.url.clone().into(),
                            }).collect();
                            let links_model = std::rc::Rc::new(slint::VecModel::from(slint_links));

                            ChatMessage {
                                id: msg_id.into(),
                                author: author.into(),
                                content: content.into(),
                                commands: slint::ModelRc::from(commands_model),
                                content_lines: map_message_lines(&content_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_content: embed_content.into(),
                                embed_lines: map_message_lines(&embed_lines, &ch_id_for_emojis, &app_weak_load),
                                embed_color: parse_hex_color(&embed_color),
                                embed_footer: embed_footer.into(),
                                code_block: code_block.into(),
                                reply_author: reply_author.into(),
                                reply_content: reply_content.into(),
                                reply_command: reply_command.into(),
                                links: slint::ModelRc::from(links_model),
                                buttons: map_message_buttons(&buttons, &ch_id_for_emojis, &app_weak_load),
                                attachments: map_message_attachments(&attachments, &app_weak_load),
                                timestamp: "Agora".into(),
                            }
                        }).collect()
                    };
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());

                    request_chat_scroll_to_bottom(app_weak.clone());
                }
            });
            true
        }
        Err(err) => {
            error!("Erro ao carregar mensagens do canal {}: {}", channel_id, err);
            let friendly_msg = if err.contains("403") || err.contains("Forbidden") {
                "🔒 Canal Privado\nEste canal é restrito e exige cargos específicos no servidor para visualizar as mensagens.".to_string()
            } else {
                format!("⚠️ Não foi possível carregar as mensagens ({})", err)
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_has_more_older_messages(false);
                    let ui_msgs = vec![ChatMessage {
                        id: "".into(),
                        author: "Litecord System".into(),
                        content: friendly_msg.into(),
                        commands: slint::ModelRc::default(),
                        content_lines: slint::ModelRc::default(),
                        embed_content: "".into(),
                        embed_lines: slint::ModelRc::default(),
                        embed_color: slint::Color::from_rgb_u8(88, 101, 242),
                        embed_footer: "".into(),
                        code_block: "".into(),
                        reply_author: "".into(),
                        reply_content: "".into(),
                        reply_command: "".into(),
                        links: slint::ModelRc::default(),
                        buttons: slint::ModelRc::default(),
                        attachments: slint::ModelRc::default(),
                        timestamp: "Agora".into(),
                    }];
                    let model = std::rc::Rc::new(slint::VecModel::from(ui_msgs));
                    ui.set_messages(model.into());
                }
            });
            false
        }
    }
}




pub static COLLAPSED_CATEGORIES: std::sync::OnceLock<Arc<Mutex<std::collections::HashSet<String>>>> = std::sync::OnceLock::new();
pub fn get_collapsed_categories() -> Arc<Mutex<std::collections::HashSet<String>>> {
    COLLAPSED_CATEGORIES.get_or_init(|| Arc::new(Mutex::new(std::collections::HashSet::new()))).clone()
}


pub async fn try_login_with_candidates(
    candidates: Vec<String>,
    app_weak: slint::Weak<AppWindow>,
    http_client: Arc<Mutex<Option<DiscordHttpClient>>>,
    last_token: Arc<Mutex<String>>,
    guilds_map: Arc<Mutex<HashMap<String, GuildData>>>,
    active_guild_id: Arc<Mutex<String>>,
    active_channel_id: Arc<Mutex<String>>,
    cmd_tx_store: Arc<Mutex<Option<mpsc::Sender<GatewayCommand>>>>,
    event_tx_gw: mpsc::Sender<GatewayEvent>,
) -> bool {
    for token_str in candidates {
        info!("Testando candidato a token via Discord REST API...");
        let http = DiscordHttpClient::new(token_str.clone());
        match http.get_current_user().await {
            Ok(user_info) => {
                let username = user_info["username"].as_str().unwrap_or("User");
                let user_id = user_info["id"].as_str().unwrap_or("");
                let global_name = user_info["global_name"].as_str().unwrap_or(username);
                let tag = if let Some(discrim) = user_info["discriminator"].as_str() {
                    if discrim != "0" {
                        format!("{}#{}", username, discrim)
                    } else {
                        format!("@{}", username)
                    }
                } else {
                    format!("@{}", username)
                };
                let display_tag = if !global_name.is_empty() && global_name != username {
                    format!("{} ({})", global_name, tag)
                } else {
                    tag.clone()
                };
                info!("Token VÁLIDO ENCONTRADO! Conectado como {} ({})", username, user_id);

                let vault = save_or_update_account(&token_str, user_id, global_name, &tag);
                sync_ui_saved_accounts(&app_weak, &vault);

                *last_token.lock().unwrap() = token_str.clone();
                *http_client.lock().unwrap() = Some(http.clone());

                let app_w = app_weak.clone();
                let display_tag_clone = display_tag.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_w.upgrade() {
                        ui.set_is_logged_in(true);
                        ui.set_user_tag(display_tag_clone.into());
                        ui.set_connection_status("Conectado com sucesso!".into());
                        ui.set_login_alert_message("".into());
                        ui.set_titlebar_error("".into());
                    }
                });

                trigger_update_check(app_weak.clone());

                let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);
                *cmd_tx_store.lock().unwrap() = Some(cmd_tx);

                fetch_and_populate_guilds(
                    &http,
                    app_weak.clone(),
                    guilds_map,
                    active_guild_id,
                    active_channel_id,
                    cmd_tx_store,
                ).await;

                let gw = Arc::new(GatewayClient::new(token_str, event_tx_gw));
                gw.start(cmd_rx).await;

                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    trim_process_memory();
                    info!("🧹 [MEMÓRIA] Faxina pós-login concluída — RAM compactada!");
                });

                return true;
            }
            Err(err_msg) => {
                info!("Candidato a token recusado pelo Discord ({}). Testando próximo candidato...", err_msg);
            }
        }
    }
    false
}


pub static PENDING_UPDATE_STORE: std::sync::OnceLock<Arc<Mutex<Option<updater::ReleaseInfo>>>> = std::sync::OnceLock::new();

pub fn get_pending_update_store() -> Arc<Mutex<Option<updater::ReleaseInfo>>> {
    PENDING_UPDATE_STORE.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

pub static OLDEST_MESSAGE_MAP: std::sync::OnceLock<Arc<Mutex<HashMap<String, String>>>> = std::sync::OnceLock::new();

pub fn get_oldest_message_map() -> Arc<Mutex<HashMap<String, String>>> {
    OLDEST_MESSAGE_MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}


pub fn trigger_update_check(app_weak: slint::Weak<AppWindow>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        if let Some(rel) = updater::check_for_updates().await {
            info!("🔔 Atualização v{} pronta para ser exibida ao usuário!", rel.version);
            *get_pending_update_store().lock().unwrap() = Some(rel.clone());
            let app_w = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_w.upgrade() {
                    ui.set_update_version(format!("v{}", rel.version).into());
                    ui.set_update_release_name(rel.release_name.into());
                    ui.set_show_update_dialog(true);
                }
            });
        }
    });
}

