//! Server invite cards for `discord.gg/<code>` links, like the official client.
//!
//! The message only carries the link; the server's name, icon and member counts come
//! from `GET /invites/<code>`. Each code is fetched once, only for messages that are
//! being shown, and the rows holding that code are updated in place when the answer
//! arrives. A code that fails is remembered and its message keeps the plain link.
//!
//! Images are asked of the CDN near the card's size and kept already shaped for the
//! window, so showing a card again copies nothing. The cache keeps the last MAX_CACHED
//! codes (a cover is ~0.3-0.6 MB) and is emptied when the app goes to the tray.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::utils::i18n::Language;
use crate::{AppWindow, ChatMessage, InviteCard};

const MAX_CARDS_PER_MESSAGE: usize = 3;
/// Codes kept, the last ones shown.
const MAX_CACHED: usize = 32;
const ICON_LOGICAL_PX: f32 = 48.0;
const INVITE_HOSTS: [&str; 3] = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"];

#[derive(Clone)]
struct InviteInfo {
    guild_name: String,
    description: String,
    online: Option<u64>,
    members: Option<u64>,
    /// Rounded icon at the tile's physical size.
    icon: Option<Pixels>,
    event: Option<EventInfo>,
}

/// A scheduled event (`discord.com/events/<guild>/<event>`). Its card shows the
/// event's name in `guild_name` and its description in `description`.
#[derive(Clone)]
struct EventInfo {
    starts_at: Option<chrono::DateTime<chrono::Local>>,
    live: bool,
    interested: Option<u64>,
    /// Cover cropped to the card's frame at its physical size.
    cover: Option<Pixels>,
}

/// Shared, reference counted pixels: a card shown again reuses them.
type Pixels = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

const EVENT_KEY_PREFIX: &str = "event:";
const INVITE_EVENT_QUERY: &str = "?event=";

fn is_event_key(key: &str) -> bool {
    key.starts_with(EVENT_KEY_PREFIX) || key.contains(INVITE_EVENT_QUERY)
}
const EVENT_COVER_LOGICAL: (f32, f32) = (400.0, 160.0);

/// Event lookups need the user's session (only members see a server's events).
static SESSION_CLIENT: OnceLock<Mutex<Option<crate::http::DiscordHttpClient>>> = OnceLock::new();

pub fn set_session_client(client: crate::http::DiscordHttpClient) {
    *SESSION_CLIENT.get_or_init(Default::default).lock().unwrap() = Some(client);
}

/// Event links in the order they appear, as cache keys `event:<guild>:<event>`.
pub fn event_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for host in ["discord.com/events/", "discordapp.com/events/"] {
        let mut rest = text;
        while let Some(at) = rest.find(host) {
            let after = &rest[at + host.len()..];
            let mut ids = after.split(|c: char| !c.is_ascii_digit());
            if let (Some(guild), Some(event)) = (ids.next(), ids.next()) {
                let key = format!("{EVENT_KEY_PREFIX}{guild}:{event}");
                if !guild.is_empty() && !event.is_empty() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
            rest = after;
        }
    }
    keys
}

#[derive(Clone)]
enum Entry {
    Loading,
    Ready(InviteInfo),
    Failed,
}

#[derive(Default)]
struct Cache {
    /// Each entry with the turn it was last shown on.
    entries: HashMap<String, (Entry, u64)>,
    turn: u64,
}

impl Cache {
    fn get(&mut self, code: &str) -> Option<Entry> {
        self.turn += 1;
        let turn = self.turn;
        self.entries.get_mut(code).map(|(entry, used)| {
            *used = turn;
            entry.clone()
        })
    }

    fn insert(&mut self, code: String, entry: Entry) {
        self.turn += 1;
        if !self.entries.contains_key(&code) && self.entries.len() >= MAX_CACHED {
            let oldest = self.entries.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(code, (entry, self.turn));
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The app went to the tray: cards already on screen keep their images, the rest goes.
pub fn clear() {
    if let Ok(mut cache) = cache().lock() {
        cache.entries.retain(|_, (entry, _)| matches!(entry, Entry::Loading));
    }
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
            // `discord.gg/<code>?event=<id>` invites to one of the server's events,
            // and is shown as that event's card.
            let event: String = after[code.len()..]
                .strip_prefix(INVITE_EVENT_QUERY)
                .map(|ids| ids.chars().take_while(|c| c.is_ascii_digit()).collect())
                .unwrap_or_default();
            let key = if event.is_empty() { code.clone() } else { format!("{code}{INVITE_EVENT_QUERY}{event}") };
            if (2..=32).contains(&code.len()) && !codes.contains(&key) {
                codes.push(key);
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
        let mut codes = invite_codes(&text);
        codes.extend(event_keys(&text));
        codes.truncate(MAX_CARDS_PER_MESSAGE);
        if codes.is_empty() {
            continue;
        }
        let mut to_fetch = Vec::new();
        let cards: Vec<InviteCard> = {
            let mut entries = cache().lock().unwrap();
            codes
                .iter()
                .filter_map(|code| match entries.get(code) {
                    Some(Entry::Ready(info)) => Some(ready_card(code, &info)),
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
            fetch(code, scale, app.clone());
        }
    }
}

fn invite_url(code: &str) -> String {
    match code.strip_prefix(EVENT_KEY_PREFIX).and_then(|ids| ids.split_once(':')) {
        Some((guild, event)) => format!("https://discord.com/events/{guild}/{event}"),
        None => format!("https://discord.gg/{code}"),
    }
}

fn loading_card(code: &str) -> InviteCard {
    InviteCard {
        code: code.into(),
        url: invite_url(code).into(),
        loaded: false,
        is_event: is_event_key(code),
        ..Default::default()
    }
}

fn event_card(code: &str, info: &InviteInfo, event: &EventInfo) -> InviteCard {
    let t = crate::utils::i18n_ui::ui_text();
    let lang = crate::utils::i18n_ui::current_language();
    let event_when = if event.live {
        t.event_live.to_string()
    } else {
        event.starts_at.map(|at| at.format("%d/%m/%Y %H:%M").to_string()).unwrap_or_default()
    };
    let cover = event.cover.clone().map(slint::Image::from_rgba8);
    InviteCard {
        code: code.into(),
        url: invite_url(code).into(),
        loaded: true,
        is_event: true,
        guild_name: info.guild_name.clone().into(),
        description: info.description.clone().into(),
        counts: event
            .interested
            .map(|n| t.event_interested.replace("{}", &group_digits(n, lang)))
            .unwrap_or_default()
            .into(),
        event_when: event_when.into(),
        event_live: event.live,
        has_cover: cover.is_some(),
        cover_img: cover.unwrap_or_default(),
        ..Default::default()
    }
}

fn ready_card(code: &str, info: &InviteInfo) -> InviteCard {
    if let Some(event) = &info.event {
        return event_card(code, info, event);
    }
    let lang = crate::utils::i18n_ui::current_language();
    let t = crate::utils::i18n_ui::ui_text();
    let mut counts = Vec::new();
    if let Some(n) = info.online {
        counts.push(t.invite_online.replace("{}", &group_digits(n, lang)));
    }
    if let Some(n) = info.members {
        counts.push(t.invite_members.replace("{}", &group_digits(n, lang)));
    }
    let icon = info.icon.clone().map(slint::Image::from_rgba8);
    InviteCard {
        code: code.into(),
        url: invite_url(code).into(),
        loaded: true,
        guild_name: info.guild_name.clone().into(),
        description: info.description.clone().into(),
        counts: counts.join(" · ").into(),
        has_icon: icon.is_some(),
        icon_img: icon.unwrap_or_default(),
        ..Default::default()
    }
}

/// Physical pixels for a logical size on this window.
fn physical(logical: f32, scale: f32) -> u32 {
    (logical * scale.clamp(1.0, 4.0)).round() as u32
}

/// The CDN's size step (a power of two) at or above `px`.
fn cdn_size(px: u32) -> u32 {
    px.clamp(16, 4096).next_power_of_two()
}

fn to_pixels(img: image::RgbaImage) -> Pixels {
    let (w, h) = img.dimensions();
    Pixels::clone_from_slice(img.as_raw(), w, h)
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

/// Built per lookup, like the avatar downloads: a proxy changed in the settings applies
/// at once, and no connection pool stays open between the rare lookups.
fn http_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .user_agent(crate::utils::client_identity::user_agent())
        .timeout(std::time::Duration::from_secs(10));
    crate::utils::apply_proxy_to_builder(builder).build().unwrap_or_default()
}

fn fetch(code: String, scale: f32, app: slint::Weak<AppWindow>) {
    tokio::spawn(async move {
        let entry = match fetch_info(&code, scale).await {
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

async fn fetch_info(code: &str, scale: f32) -> Option<InviteInfo> {
    if let Some((guild_id, event_id)) = code.strip_prefix(EVENT_KEY_PREFIX).and_then(|ids| ids.split_once(':')) {
        return fetch_event(guild_id, event_id, scale).await;
    }
    fetch_invite(code, scale).await
}

async fn fetch_event(guild_id: &str, event_id: &str, scale: f32) -> Option<InviteInfo> {
    let client = SESSION_CLIENT.get()?.lock().ok()?.clone()?;
    let json = client.get_scheduled_event(guild_id, event_id).await.ok()?;
    event_from_json(&json, event_id, scale).await
}

/// Card data of a scheduled event object, from the events API or an invite.
async fn event_from_json(json: &serde_json::Value, event_id: &str, scale: f32) -> Option<InviteInfo> {
    json.get("name")?;
    let starts_at = json["scheduled_start_time"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|at| at.with_timezone(&chrono::Local));
    // Status 2 = active.
    let live = json["status"].as_u64() == Some(2);
    let cover = match json["image"].as_str() {
        Some(hash) if hash.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            let (w, h) = (physical(EVENT_COVER_LOGICAL.0, scale), physical(EVENT_COVER_LOGICAL.1, scale));
            let url = format!("https://cdn.discordapp.com/guild-events/{event_id}/{hash}.png?size={}", cdn_size(w));
            download_cover(&url, w, h).await
        }
        _ => None,
    };
    Some(InviteInfo {
        guild_name: json["name"].as_str().unwrap_or_default().to_string(),
        description: json["description"].as_str().unwrap_or_default().to_string(),
        online: None,
        members: None,
        icon: None,
        event: Some(EventInfo { starts_at, live, interested: json["user_count"].as_u64(), cover }),
    })
}

/// Cover cropped to the card's 2.5:1 frame at `w` x `h` physical pixels.
async fn download_cover(url: &str, w: u32, h: u32) -> Option<Pixels> {
    let bytes = http_client().get(url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
    if bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        Some(to_pixels(img.resize_to_fill(w, h, image::imageops::FilterType::Lanczos3).to_rgba8()))
    })
    .await
    .ok()
    .flatten()
}

async fn fetch_invite(key: &str, scale: f32) -> Option<InviteInfo> {
    let (code, event_id) = match key.split_once(INVITE_EVENT_QUERY) {
        Some((code, event)) => (code, Some(event)),
        None => (key, None),
    };
    // Invite lookups need no token; the user's session is not sent for them.
    let mut url = format!("https://discord.com/api/v10/invites/{code}?with_counts=true");
    if let Some(event) = event_id {
        url.push_str(&format!("&guild_scheduled_event_id={event}"));
    }
    let json: serde_json::Value = http_client().get(&url).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    // An event that has ended is no longer returned: the plain invite card is shown.
    if let (Some(event_id), Some(event)) = (event_id, json.get("guild_scheduled_event")) {
        if let Some(info) = event_from_json(event, event_id, scale).await {
            return Some(info);
        }
    }
    let guild = json.get("guild")?;
    let guild_id = guild["id"].as_str()?;
    let icon = match guild["icon"].as_str() {
        Some(hash) if hash.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            let side = physical(ICON_LOGICAL_PX, scale);
            download_icon(&format!("https://cdn.discordapp.com/icons/{guild_id}/{hash}.png?size={}", cdn_size(side)), side).await
        }
        _ => None,
    };
    Some(InviteInfo {
        guild_name: guild["name"].as_str().unwrap_or_default().to_string(),
        description: guild["description"].as_str().unwrap_or_default().to_string(),
        online: json["approximate_presence_count"].as_u64(),
        members: json["approximate_member_count"].as_u64(),
        icon,
        event: None,
    })
}

/// Smooth resize to the tile's physical size, then rounded corners: the software
/// renderer neither smooths a shrinking image nor clips to a radius.
async fn download_icon(url: &str, side: u32) -> Option<Pixels> {
    let bytes = http_client().get(url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        let mut square = img.resize_to_fill(side, side, image::imageops::FilterType::Lanczos3).to_rgba8();
        crate::utils::image_shape::round_corners(&mut square, side, side, side as f32 / 3.0);
        Some(to_pixels(square))
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
    fn finds_event_links_as_cache_keys() {
        let text = "RSVP: https://discord.com/events/613425648685547541/1425901390633250877 \
                    again https://discord.com/events/613425648685547541/1425901390633250877";
        assert_eq!(event_keys(text), vec!["event:613425648685547541:1425901390633250877"]);
        assert_eq!(
            invite_url("event:1:2"),
            "https://discord.com/events/1/2"
        );
        assert!(event_keys("https://discord.com/events/").is_empty());
    }

    #[test]
    fn invites_to_an_event_keep_the_event_in_their_key() {
        let text = "@cargo https://discord.gg/seafight-269116793526812673?event=1560218812681289748";
        let keys = invite_codes(text);
        assert_eq!(keys, vec!["seafight-269116793526812673?event=1560218812681289748"]);
        assert!(is_event_key(&keys[0]));
        assert_eq!(invite_url(&keys[0]), "https://discord.gg/seafight-269116793526812673?event=1560218812681289748");
    }

    #[test]
    fn ignores_text_without_invites() {
        assert!(invite_codes("see https://discord.com/channels/1/2").is_empty());
    }

    #[test]
    fn cache_keeps_the_codes_shown_last() {
        let mut cache = Cache::default();
        for i in 0..MAX_CACHED {
            cache.insert(format!("c{i}"), Entry::Failed);
        }
        assert!(cache.get("c0").is_some(), "shown again: now the newest");
        cache.insert("new".into(), Entry::Failed);
        assert_eq!(cache.entries.len(), MAX_CACHED);
        assert!(cache.get("c1").is_none(), "the least recently shown went");
        assert!(cache.get("c0").is_some() && cache.get("new").is_some());
    }

    #[test]
    fn images_are_asked_near_the_card_size() {
        assert_eq!(cdn_size(physical(48.0, 1.0)), 64);
        assert_eq!(cdn_size(physical(48.0, 1.5)), 128);
        assert_eq!(cdn_size(physical(400.0, 1.0)), 512);
        assert_eq!(cdn_size(physical(400.0, 1.5)), 1024);
    }

    #[test]
    fn member_counts_are_grouped_per_language() {
        assert_eq!(group_digits(308679, Language::Portuguese), "308.679");
        assert_eq!(group_digits(308679, Language::English), "308,679");
        assert_eq!(group_digits(999, Language::German), "999");
        assert_eq!(group_digits(1234567, Language::Spanish), "1.234.567");
    }
}
