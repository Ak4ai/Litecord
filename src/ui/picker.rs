//! Emoji and sticker picker above the message bar.
//!
//! Only what is on screen costs anything: images are loaded for the rows in
//! view (from the emoji disk cache when there, else downloaded), shrunk to
//! picker size, and dropped when the panel closes. The server's emoji and
//! stickers are fetched once per server when the panel first opens there.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::http::DiscordHttpClient;
use crate::ui::emoji_data::{EMOJIS, GROUPS};
use crate::utils::i18n_ui::en_pt;
use crate::{AppWindow, PickerCat, PickerItem, PickerRow};

const PER_ROW: usize = 8;
const STICKERS_PER_ROW: usize = 4;
/// Row heights; must match picker.slint.
const HEADER_H: f32 = 28.0;
const EMOJI_H: f32 = 40.0;
const STICKER_H: f32 = 96.0;
const EMOJI_PX: u32 = 32;
const STICKER_PX: u32 = 88;
const MAX_RECENTS: usize = 16;

#[derive(Clone, Default)]
struct GuildAssets {
    /// (id, name) of the server's static emoji.
    emojis: Vec<(String, String)>,
    /// (id, name, is_gif) of the server's PNG/APNG/GIF stickers.
    stickers: Vec<(String, String, bool)>,
}

static ASSETS: Mutex<Option<(String, GuildAssets)>> = Mutex::new(None);
/// (key, name, text to insert) of the last picked emoji, newest first.
static RECENTS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

#[derive(Default)]
struct View {
    generation: u64,
    rows: Option<Rc<VecModel<PickerRow>>>,
    cats: Option<Rc<VecModel<PickerCat>>>,
    heights: Vec<f32>,
    /// Where each image key sits: (row, column); column usize::MAX = category bar.
    places: HashMap<String, Vec<(usize, usize)>>,
    requested: HashSet<String>,
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View::default());
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn group_name(i: usize) -> &'static str {
    match i {
        0 => en_pt("Smileys & Emotion", "Carinhas e emoções"),
        1 => en_pt("People & Body", "Pessoas e corpo"),
        2 => en_pt("Animals & Nature", "Animais e natureza"),
        3 => en_pt("Food & Drink", "Comidas e bebidas"),
        4 => en_pt("Travel & Places", "Viagens e lugares"),
        5 => en_pt("Activities", "Atividades"),
        6 => en_pt("Objects", "Objetos"),
        7 => en_pt("Symbols", "Símbolos"),
        _ => en_pt("Flags", "Bandeiras"),
    }
}

/// The Twemoji key the chat uses for a Unicode emoji ("u_1f600").
fn unicode_key(text: &str) -> String {
    crate::gateway::formatting::find_next_unicode_emoji(text)
        .map(|(_, _, _, key)| key)
        .unwrap_or_default()
}

struct Entry {
    key: String,
    name: String,
    insert: String,
}

/// Opens the panel on a tab (0 = emoji, 1 = stickers), or closes it when
/// that tab is already open.
pub fn toggle(ui: &AppWindow, tab: i32, http: Option<DiscordHttpClient>, guild_id: String) {
    if ui.get_picker_open() && ui.get_picker_tab() == tab {
        close(ui);
        return;
    }
    ui.set_picker_tab(tab);
    ui.set_picker_query("".into());
    ui.set_picker_tr_emoji(en_pt("Emoji", "Emojis").into());
    ui.set_picker_tr_stickers(en_pt("Stickers", "Figurinhas").into());
    ui.set_picker_tr_search(en_pt("Search", "Buscar").into());
    ui.set_picker_open(true);
    rebuild(ui);

    let have = lock(&ASSETS).as_ref().is_some_and(|(g, _)| *g == guild_id);
    if have || guild_id.is_empty() || guild_id == "@me" {
        return;
    }
    let Some(http) = http else { return };
    let app_weak = ui.as_weak();
    tokio::spawn(async move {
        let (emoji_path, sticker_path) = (format!("/guilds/{}/emojis", guild_id), format!("/guilds/{}/stickers", guild_id));
        let (emojis, stickers) = tokio::join!(http.get_json(&emoji_path), http.get_json(&sticker_path));
        let mut assets = GuildAssets::default();
        for e in emojis.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default() {
            let usable = e["available"].as_bool().unwrap_or(true) && !e["animated"].as_bool().unwrap_or(false);
            if let (Some(id), Some(name), true) = (e["id"].as_str(), e["name"].as_str(), usable) {
                assets.emojis.push((id.to_string(), name.to_string()));
            }
        }
        for s in stickers.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default() {
            // 1 PNG, 2 APNG, 4 GIF; 3 is Lottie, which this picker does not draw.
            let format = s["format_type"].as_u64().unwrap_or(0);
            if let (Some(id), Some(name), true) = (s["id"].as_str(), s["name"].as_str(), matches!(format, 1 | 2 | 4)) {
                assets.stickers.push((id.to_string(), name.to_string(), format == 4));
            }
        }
        *lock(&ASSETS) = Some((guild_id, assets));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_weak.upgrade() {
                if ui.get_picker_open() {
                    rebuild(&ui);
                }
            }
        });
    });
}

/// Closes the panel and lets go of its images.
pub fn close(ui: &AppWindow) {
    ui.set_picker_open(false);
    ui.set_picker_rows(ModelRc::default());
    ui.set_picker_cats(ModelRc::default());
    VIEW.with(|v| {
        let gen = v.borrow().generation + 1;
        *v.borrow_mut() = View { generation: gen, ..View::default() };
        VIEW_GEN.store(gen, std::sync::atomic::Ordering::Relaxed);
    });
}

pub fn search(ui: &AppWindow) {
    rebuild(ui);
}

pub fn set_tab(ui: &AppWindow, tab: i32) {
    ui.set_picker_tab(tab);
    rebuild(ui);
}

/// Builds the rows for the current tab and search text.
fn rebuild(ui: &AppWindow) {
    let query = ui.get_picker_query().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let matches = |name: &str| {
        let name = name.to_lowercase();
        words.iter().all(|w| name.contains(w))
    };
    let assets = lock(&ASSETS).as_ref().map(|(_, a)| a.clone()).unwrap_or_default();

    // (header, entries, sticker section)
    let mut sections: Vec<(String, Vec<Entry>, bool)> = Vec::new();
    if ui.get_picker_tab() == 1 {
        let stickers: Vec<Entry> = assets
            .stickers
            .iter()
            .filter(|(_, n, _)| matches(n))
            .map(|(id, name, gif)| Entry {
                key: format!("{}_{}", if *gif { "g" } else { "s" }, id),
                name: name.clone(),
                insert: id.clone(),
            })
            .collect();
        let header = if stickers.is_empty() {
            en_pt("This server has no stickers you can use", "Este servidor não tem figurinhas que você possa usar")
        } else {
            en_pt("Server stickers", "Figurinhas do servidor")
        };
        sections.push((header.to_string(), stickers, true));
    } else {
        let custom: Vec<Entry> = assets
            .emojis
            .iter()
            .filter(|(_, n)| matches(n))
            .map(|(id, name)| Entry { key: format!("c_{}", id), name: name.clone(), insert: format!("<:{}:{}>", name, id) })
            .collect();
        if words.is_empty() {
            let recents: Vec<Entry> = lock(&RECENTS)
                .iter()
                .map(|(k, n, i)| Entry { key: k.clone(), name: n.clone(), insert: i.clone() })
                .collect();
            if !recents.is_empty() {
                sections.push((en_pt("Recent", "Recentes").to_string(), recents, false));
            }
            if !custom.is_empty() {
                sections.push((en_pt("Server", "Servidor").to_string(), custom, false));
            }
            for (gi, _) in GROUPS.iter().enumerate() {
                let entries = EMOJIS
                    .iter()
                    .filter(|(_, _, g)| *g as usize == gi)
                    .map(|(t, n, _)| Entry { key: unicode_key(t), name: n.to_string(), insert: t.to_string() })
                    .collect();
                sections.push((group_name(gi).to_string(), entries, false));
            }
        } else {
            let mut found = custom;
            found.extend(
                EMOJIS
                    .iter()
                    .filter(|(_, n, _)| matches(n))
                    .map(|(t, n, _)| Entry { key: unicode_key(t), name: n.to_string(), insert: t.to_string() }),
            );
            let header = if found.is_empty() { en_pt("No emoji found", "Nenhum emoji encontrado") } else { en_pt("Results", "Resultados") };
            sections.push((header.to_string(), found, false));
        }
    }

    let mut rows = Vec::new();
    let mut heights = Vec::new();
    let mut cats = Vec::new();
    let mut places: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    let mut y = 0.0f32;
    for (header, entries, sticker) in sections {
        if ui.get_picker_tab() == 0 && words.is_empty() {
            if let Some(first) = entries.first() {
                places.entry(first.key.clone()).or_default().push((cats.len(), usize::MAX));
                cats.push(PickerCat { key: first.key.clone().into(), name: header.clone().into(), img: slint::Image::default(), loaded: false, y });
            }
        }
        rows.push(PickerRow { header: header.into(), items: ModelRc::default(), sticker });
        heights.push(HEADER_H);
        y += HEADER_H;
        let per_row = if sticker { STICKERS_PER_ROW } else { PER_ROW };
        for chunk in entries.chunks(per_row) {
            let row_index = rows.len();
            let items: Vec<PickerItem> = chunk
                .iter()
                .enumerate()
                .map(|(col, e)| {
                    places.entry(e.key.clone()).or_default().push((row_index, col));
                    PickerItem {
                        key: e.key.clone().into(),
                        name: e.name.clone().into(),
                        insert: e.insert.clone().into(),
                        img: slint::Image::default(),
                        loaded: false,
                    }
                })
                .collect();
            rows.push(PickerRow { header: "".into(), items: ModelRc::new(VecModel::from(items)), sticker });
            let h = if sticker { STICKER_H } else { EMOJI_H };
            heights.push(h);
            y += h;
        }
    }

    let rows_model = Rc::new(VecModel::from(rows));
    let cats_model = Rc::new(VecModel::from(cats));
    ui.set_picker_rows(ModelRc::from(rows_model.clone()));
    ui.set_picker_cats(ModelRc::from(cats_model.clone()));
    ui.set_picker_scroll_reset(ui.get_picker_scroll_reset() + 1);
    let keys: Vec<String> = cats_model.iter().map(|c| c.key.to_string()).collect();
    let gen = VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.generation += 1;
        v.rows = Some(rows_model);
        v.cats = Some(cats_model);
        v.heights = heights;
        v.places = places;
        v.requested.clear();
        VIEW_GEN.store(v.generation, std::sync::atomic::Ordering::Relaxed);
        v.generation
    });
    for key in keys {
        request(ui, gen, key);
    }
    scrolled(ui, 0.0, 420.0);
}

/// Loads the images of the rows between `top` and `top + height` (pixels).
pub fn scrolled(ui: &AppWindow, top: f32, height: f32) {
    let (gen, keys) = VIEW.with(|v| {
        let v = v.borrow();
        let Some(rows) = v.rows.as_ref() else { return (v.generation, Vec::new()) };
        let mut keys = Vec::new();
        let mut y = 0.0f32;
        // One screen of margin below, so scrolling down finds them ready.
        let bottom = top + height * 2.0;
        for (i, h) in v.heights.iter().enumerate() {
            if y + h >= top && y <= bottom {
                if let Some(row) = rows.row_data(i) {
                    keys.extend(row.items.iter().filter(|it| !it.loaded).map(|it| it.key.to_string()));
                }
            }
            y += h;
            if y > bottom {
                break;
            }
        }
        (v.generation, keys)
    });
    for key in keys {
        request(ui, gen, key);
    }
}

fn semaphore() -> &'static Arc<tokio::sync::Semaphore> {
    static SEM: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(6)))
}

/// Where an image key comes from and where it is kept on disk.
fn source_of(key: &str) -> Option<(String, String, u32)> {
    let (kind, id) = key.split_once('_')?;
    Some(match kind {
        "u" => (format!("https://cdnjs.cloudflare.com/ajax/libs/twemoji/14.0.2/72x72/{}.png", id), format!("u_{}.png", id), EMOJI_PX),
        "c" => (format!("https://cdn.discordapp.com/emojis/{}.png?size=48&quality=lossless", id), format!("{}.png", id), EMOJI_PX),
        "s" => (format!("https://media.discordapp.net/stickers/{}.png?size=160", id), format!("sticker_{}.png", id), STICKER_PX),
        "g" => (format!("https://media.discordapp.net/stickers/{}.gif?size=160", id), format!("sticker_{}.gif", id), STICKER_PX),
        _ => return None,
    })
}

fn request(ui: &AppWindow, gen: u64, key: String) {
    if key.is_empty() || !VIEW.with(|v| v.borrow_mut().requested.insert(key.clone())) {
        return;
    }
    let Some((url, file, px)) = source_of(&key) else { return };
    let path = crate::utils::emoji_cache::get_emoji_cache().disk_dir().join(file);
    let app_weak = ui.as_weak();
    tokio::spawn(async move {
        let _permit = semaphore().acquire().await;
        if VIEW_GEN.load(std::sync::atomic::Ordering::Relaxed) != gen {
            return;
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => {
                let builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8));
                let Ok(client) = crate::utils::apply_proxy_to_builder(builder).build() else { return };
                let Ok(resp) = client.get(&url).send().await else { return };
                if !resp.status().is_success() {
                    return;
                }
                let Ok(b) = resp.bytes().await else { return };
                let _ = tokio::fs::write(&path, &b).await;
                b.to_vec()
            }
        };
        let Some(buf) = shrink(&bytes, px) else { return };
        let _ = slint::invoke_from_event_loop(move || {
            if app_weak.upgrade().is_some() {
                apply(gen, &key, buf);
            }
        });
    });
}

/// Mirror of the view generation for worker threads (VIEW is UI-thread only).
static VIEW_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn shrink(bytes: &[u8], px: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(px, px).to_rgba8();
    let (w, h) = small.dimensions();
    Some(SharedPixelBuffer::clone_from_slice(small.as_raw(), w, h))
}

fn apply(gen: u64, key: &str, buf: SharedPixelBuffer<Rgba8Pixel>) {
    VIEW.with(|v| {
        let v = v.borrow();
        if v.generation != gen {
            return;
        }
        let img = slint::Image::from_rgba8(buf);
        for &(row, col) in v.places.get(key).into_iter().flatten() {
            if col == usize::MAX {
                if let Some(cats) = v.cats.as_ref() {
                    if let Some(mut c) = cats.row_data(row) {
                        c.img = img.clone();
                        c.loaded = true;
                        cats.set_row_data(row, c);
                    }
                }
                continue;
            }
            if let Some(r) = v.rows.as_ref().and_then(|rows| rows.row_data(row)) {
                if let Some(mut item) = r.items.row_data(col) {
                    item.img = img.clone();
                    item.loaded = true;
                    r.items.set_row_data(col, item);
                }
            }
        }
    });
}

/// An emoji goes into the message box; a sticker is sent right away.
pub fn pick(ui: &AppWindow, row: i32, col: i32, http: Option<DiscordHttpClient>, channel_id: String) {
    let item = VIEW.with(|v| {
        let v = v.borrow();
        let r = v.rows.as_ref()?.row_data(row.max(0) as usize)?;
        Some((r.sticker, r.items.row_data(col.max(0) as usize)?))
    });
    let Some((sticker, item)) = item else { return };
    if sticker {
        close(ui);
        if let (Some(http), false) = (http, channel_id.is_empty()) {
            let id = item.insert.to_string();
            tokio::spawn(async move {
                let _ = http.send_sticker(&channel_id, &id).await;
            });
        }
        return;
    }
    let mut text = ui.get_chat_input_text().to_string();
    text.push_str(&item.insert);
    ui.set_chat_input_text(text.into());
    let mut recents = lock(&RECENTS);
    recents.retain(|(k, _, _)| k.as_str() != item.key.as_str());
    recents.insert(0, (item.key.to_string(), item.name.to_string(), item.insert.to_string()));
    recents.truncate(MAX_RECENTS);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_table_and_keys() {
        assert!(EMOJIS.len() > 1500);
        assert_eq!(unicode_key("😀"), "u_1f600");
        // Variation selectors are dropped, as in the chat's Twemoji names.
        assert_eq!(unicode_key("❤️"), "u_2764");
        assert!(source_of("s_123").unwrap().0.contains("/stickers/123.png"));
        assert!(source_of("c_9").unwrap().1 == "9.png");
    }
}
