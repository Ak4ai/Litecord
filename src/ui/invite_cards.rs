//! Server invite cards for `discord.gg/<code>` links, like the official client.
//!
//! The message only carries the link; the server's name, icon and member counts come
//! from `GET /invites/<code>`. Each code is fetched once per session and cached, only
//! for messages that are being shown, and the rows holding that code are updated in
//! place when the answer arrives. A code that fails is remembered and its message keeps
//! the plain link.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::utils::i18n::Language;
use crate::{AppWindow, ChatMessage, InviteCard};

const MAX_CARDS_PER_MESSAGE: usize = 3;
const ICON_LOGICAL_PX: f32 = 48.0;
const INVITE_HOSTS: [&str; 3] = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"];

#[derive(Clone)]
struct InviteInfo {
    guild_name: String,
    description: String,
    online: Option<u64>,
    members: Option<u64>,
    /// Square RGBA icon at its downloaded size; shaped per window scale when shown.
    icon: Option<(Vec<u8>, u32)>,
}

#[derive(Clone)]
enum Entry {
    Loading,
    Ready(InviteInfo),
    Failed,
}

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Invite codes in the order they appear, without repeats.
pub fn invite_codes(text: &str) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for host in INVITE_HOSTS {
        let mut rest = text;
        while let Some(at) = rest.find(host) {
            let after = &rest[at + host.len()..];
            let code: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if (2..=32).contains(&code.len()) && !codes.contains(&code) {
                codes.push(code);
            }
            rest = after;
        }
    }
    codes.truncate(MAX_CARDS_PER_MESSAGE);
    codes
}

/// Gives each message the cards for its invite links and starts fetching the codes
/// not seen yet. Runs on the UI thread, before the rows are handed to the model.
pub fn hydrate(messages: &mut [ChatMessage], app: &slint::Weak<AppWindow>) {
    let scale = app.upgrade().map(|ui| ui.window().scale_factor()).unwrap_or(1.0);
    for message in messages {
        // Links are often taken out of the text into `links`, so both are searched.
        let mut text = message.content.to_string();
        for link in message.links.iter() {
            text.push(' ');
            text.push_str(&link.url);
        }
        let codes = invite_codes(&text);
        if codes.is_empty() {
            continue;
        }
        let mut to_fetch = Vec::new();
        let cards: Vec<InviteCard> = {
            let mut entries = cache().lock().unwrap();
            codes
                .iter()
                .filter_map(|code| match entries.get(code) {
                    Some(Entry::Ready(info)) => Some(ready_card(code, info, scale)),
                    Some(Entry::Loading) => Some(loading_card(code)),
                    Some(Entry::Failed) => None,
                    None => {
                        entries.insert(code.clone(), Entry::Loading);
                        to_fetch.push(code.clone());
                        Some(loading_card(code))
                    }
                })
                .collect()
        };
        message.invites = ModelRc::new(VecModel::from(cards));
        for code in to_fetch {
            fetch(code, app.clone());
        }
    }
}

fn invite_url(code: &str) -> String {
    format!("https://discord.gg/{code}")
}

fn loading_card(code: &str) -> InviteCard {
    InviteCard {
        code: code.into(),
        url: invite_url(code).into(),
        loaded: false,
        ..Default::default()
    }
}

fn ready_card(code: &str, info: &InviteInfo, scale: f32) -> InviteCard {
    let lang = crate::utils::i18n_ui::current_language();
    let t = crate::utils::i18n_ui::ui_text();
    let mut counts = Vec::new();
    if let Some(n) = info.online {
        counts.push(t.invite_online.replace("{}", &group_digits(n, lang)));
    }
    if let Some(n) = info.members {
        counts.push(t.invite_members.replace("{}", &group_digits(n, lang)));
    }
    let icon = info.icon.as_ref().map(|(rgba, side)| shaped_icon(rgba, *side, scale));
    InviteCard {
        code: code.into(),
        url: invite_url(code).into(),
        loaded: true,
        guild_name: info.guild_name.clone().into(),
        description: info.description.clone().into(),
        counts: counts.join(" · ").into(),
        has_icon: icon.is_some(),
        icon_img: icon.unwrap_or_default(),
    }
}

/// Smooth resize to the tile's physical size, then rounded corners: the software
/// renderer neither smooths a shrinking image nor clips to a radius.
fn shaped_icon(rgba: &[u8], side: u32, scale: f32) -> slint::Image {
    let target = (ICON_LOGICAL_PX * scale.clamp(1.0, 4.0)).round() as u32;
    let mut pixels = match image::RgbaImage::from_raw(side, side, rgba.to_vec()) {
        Some(img) if side > target => {
            image::imageops::resize(&img, target, target, image::imageops::FilterType::Lanczos3)
        }
        Some(img) => img,
        None => return slint::Image::default(),
    };
    let (w, h) = pixels.dimensions();
    crate::utils::image_shape::round_corners(&mut pixels, w, h, w as f32 / 3.0);
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(pixels.as_raw(), w, h);
    slint::Image::from_rgba8(buffer)
}

fn group_digits(n: u64, lang: Language) -> String {
    let sep = match lang {
        Language::English | Language::Japanese => ",",
        Language::French => "\u{202f}",
        _ => ".",
    };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let builder = reqwest::Client::builder()
            .user_agent(crate::utils::client_identity::user_agent())
            .timeout(std::time::Duration::from_secs(10));
        crate::utils::apply_proxy_to_builder(builder).build().unwrap_or_default()
    })
}

fn fetch(code: String, app: slint::Weak<AppWindow>) {
    tokio::spawn(async move {
        let entry = match fetch_info(&code).await {
            Some(info) => Entry::Ready(info),
            None => Entry::Failed,
        };
        cache().lock().unwrap().insert(code.clone(), entry);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app.upgrade() {
                refresh_rows(&ui, &code);
            }
        });
    });
}

async fn fetch_info(code: &str) -> Option<InviteInfo> {
    // Invite lookups need no token; the user's session is not sent for them.
    let url = format!("https://discord.com/api/v10/invites/{code}?with_counts=true");
    let json: serde_json::Value = http_client().get(&url).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    let guild = json.get("guild")?;
    let guild_id = guild["id"].as_str()?;
    let icon = match guild["icon"].as_str() {
        Some(hash) if hash.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            download_icon(&format!("https://cdn.discordapp.com/icons/{guild_id}/{hash}.png?size=128")).await
        }
        _ => None,
    };
    Some(InviteInfo {
        guild_name: guild["name"].as_str().unwrap_or_default().to_string(),
        description: guild["description"].as_str().unwrap_or_default().to_string(),
        online: json["approximate_presence_count"].as_u64(),
        members: json["approximate_member_count"].as_u64(),
        icon,
    })
}

async fn download_icon(url: &str) -> Option<(Vec<u8>, u32)> {
    let bytes = http_client().get(url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        let side = img.width().min(img.height()).min(256);
        let square = img.resize_to_fill(side, side, image::imageops::FilterType::Lanczos3).to_rgba8();
        Some((square.into_raw(), side))
    })
    .await
    .ok()
    .flatten()
}

/// Rebuilds the cards of the rows that link to `code`, and only those rows.
fn refresh_rows(ui: &AppWindow, code: &str) {
    let messages = ui.get_messages();
    for index in 0..messages.row_count() {
        let Some(mut message) = messages.row_data(index) else { continue };
        if !message.invites.iter().any(|card| card.code == code) {
            continue;
        }
        let mut rows = [message];
        hydrate(&mut rows, &ui.as_weak());
        let [updated] = rows;
        message = updated;
        messages.set_row_data(index, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_invite_codes_in_every_link_form() {
        let text = "Join https://discord.gg/discord-developers or discord.com/invite/AbC12 \
                    and again https://discord.gg/discord-developers, plus discordapp.com/invite/x";
        assert_eq!(invite_codes(text), vec!["discord-developers", "AbC12"]);
    }

    #[test]
    fn ignores_text_without_invites() {
        assert!(invite_codes("see https://discord.com/channels/1/2").is_empty());
    }

    #[test]
    fn member_counts_are_grouped_per_language() {
        assert_eq!(group_digits(308679, Language::Portuguese), "308.679");
        assert_eq!(group_digits(308679, Language::English), "308,679");
        assert_eq!(group_digits(999, Language::German), "999");
        assert_eq!(group_digits(1234567, Language::Spanish), "1.234.567");
    }
}
