use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use log::warn;
use slint::Model;
use tokio::sync::Semaphore;

#[derive(Clone)]
struct DecodedEmoji {
    buffer: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
}

pub struct EmojiCache {
    memory_cache: Mutex<HashMap<String, DecodedEmoji>>,
    in_flight: Mutex<HashSet<String>>,
    active_channel: Mutex<String>,
    active_generation: AtomicU64,
    semaphore: Arc<Semaphore>,
    disk_dir: PathBuf,
}

static EMOJI_CACHE: OnceLock<Arc<EmojiCache>> = OnceLock::new();

pub fn get_emoji_cache() -> Arc<EmojiCache> {
    EMOJI_CACHE.get_or_init(|| {
        let mut disk_dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|_| {
                std::env::var("XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
            })
            .unwrap_or_else(|_| PathBuf::from("."));
        disk_dir.push("Litecord");
        disk_dir.push("cache");
        disk_dir.push("emojis");

        if let Err(e) = std::fs::create_dir_all(&disk_dir) {
            warn!("Não foi possível criar diretório de cache de emojis: {}", e);
        }

        Arc::new(EmojiCache {
            memory_cache: Mutex::new(HashMap::with_capacity(64)),
            in_flight: Mutex::new(HashSet::new()),
            active_channel: Mutex::new(String::new()),
            active_generation: AtomicU64::new(1),
            semaphore: Arc::new(Semaphore::new(8)), // Up to 8 parallel downloads
            disk_dir,
        })
    }).clone()
}

impl EmojiCache {
    /// Notify cache that user switched active channel (clears stale in-flight tracking & bumps generation)
    pub fn set_active_channel(&self, channel_id: &str) {
        if let Ok(mut ac) = self.active_channel.lock() {
            *ac = channel_id.to_string();
        }
        self.active_generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut inf) = self.in_flight.lock() {
            inf.clear();
        }
    }

    /// Check if a channel is currently active on user screen
    pub fn is_channel_active(&self, channel_id: &str) -> bool {
        if channel_id.is_empty() {
            return true;
        }
        if let Ok(ac) = self.active_channel.lock() {
            if ac.is_empty() {
                return true;
            }
            return *ac == channel_id;
        }
        true
    }

    /// Returns cached Slint Image if already in RAM, otherwise checks disk cache.
    pub fn get(&self, emoji_id: &str) -> Option<slint::Image> {
        if emoji_id.is_empty() {
            return None;
        }

        // 1. Check in-memory cache
        if let Ok(guard) = self.memory_cache.lock() {
            if let Some(dec) = guard.get(emoji_id) {
                return Some(slint::Image::from_rgba8(dec.buffer.clone()));
            }
        }

        // 2. Check disk cache
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let image = slint::Image::from_rgba8(dec.buffer.clone());
                    if let Ok(mut guard) = self.memory_cache.lock() {
                        if guard.len() >= 64 {
                            guard.clear();
                        }
                        guard.insert(emoji_id.to_string(), dec);
                    }
                    return Some(image);
                }
            }
        }

        None
    }

    /// Returns the image only if it is already decoded in RAM.
    ///
    /// Painting paths use this instead of `get`, so a cold cache never reads and
    /// decodes a PNG inside the UI thread; they call `hydrate_from_disk_async`.
    pub fn get_cached(&self, emoji_id: &str) -> Option<slint::Image> {
        if emoji_id.is_empty() {
            return None;
        }
        let guard = self.memory_cache.lock().ok()?;
        guard
            .get(emoji_id)
            .map(|dec| slint::Image::from_rgba8(dec.buffer.clone()))
    }

    /// Drops decoded emojis from RAM (the PNG disk cache is kept).
    pub fn clear_memory(&self) {
        if let Ok(mut guard) = self.memory_cache.lock() {
            guard.clear();
        }
    }

    /// Decodes an emoji already saved on disk on a blocking worker and refreshes
    /// the rows that use it.
    pub fn hydrate_from_disk_async(
        &self,
        emoji_id: &str,
        app_weak: slint::Weak<crate::AppWindow>,
    ) {
        if emoji_id.is_empty() {
            return;
        }
        if let Ok(guard) = self.memory_cache.lock() {
            if guard.contains_key(emoji_id) {
                return;
            }
        }
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));
        if !file_path.exists() {
            return;
        }
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if !in_flight.insert(emoji_id.to_string()) {
                return;
            }
        } else {
            return;
        }

        let cache_arc = get_emoji_cache();
        let id_str = emoji_id.to_string();
        tokio::spawn(async move {
            let path = file_path.clone();
            let decoded = tokio::task::spawn_blocking(move || {
                std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| decode_bytes_to_rgba(&bytes))
            })
            .await
            .ok()
            .flatten();
            if let Some(dec) = decoded {
                if let Ok(mut guard) = cache_arc.memory_cache.lock() {
                    if guard.len() >= 64 {
                        guard.clear();
                    }
                    guard.insert(id_str.clone(), dec);
                }
                let id_ui = id_str.clone();
                let cache_ui = cache_arc.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_weak.upgrade() {
                        if let Some(img) = cache_ui.get_cached(&id_ui) {
                            crate::ui::helpers::refresh_channel_emoji(&ui, &id_ui, &img);
                            crate::ui::helpers::refresh_emoji_in_messages(&ui, &id_ui, &img);
                        }
                    }
                });
            }
            if let Ok(mut in_flight) = cache_arc.in_flight.lock() {
                in_flight.remove(&id_str);
            }
        });
    }

    /// Prioritized async downloader: supports both Discord custom emojis and Unicode Twemoji
    pub fn fetch_priority_async(&self, emoji_id: &str, channel_id: &str, app_weak: slint::Weak<crate::AppWindow>) {
        if emoji_id.is_empty() {
            return;
        }

        // Only download if channel is currently active (or if channel_id not specified)
        if !channel_id.is_empty() && !self.is_channel_active(channel_id) {
            return;
        }

        // Check if already in memory
        if let Ok(guard) = self.memory_cache.lock() {
            if guard.contains_key(emoji_id) {
                return;
            }
        }

        // Check and mark in-flight to prevent duplicate simultaneous requests
        if let Ok(mut inf) = self.in_flight.lock() {
            if inf.contains(emoji_id) {
                return;
            }
            inf.insert(emoji_id.to_string());
        }

        let cache_arc = get_emoji_cache();
        let id_str = emoji_id.to_string();
        let ch_id_str = channel_id.to_string();
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));
        let expected_gen = self.active_generation.load(Ordering::SeqCst);
        let sem = self.semaphore.clone();

        tokio::spawn(async move {
            let _permit = sem.acquire().await;

            // Check again if still on the same channel / generation
            if !ch_id_str.is_empty() && (!cache_arc.is_channel_active(&ch_id_str) || expected_gen != cache_arc.active_generation.load(Ordering::SeqCst)) {
                if let Ok(mut inf) = cache_arc.in_flight.lock() {
                    inf.remove(&id_str);
                }
                return;
            }

            let url = if id_str.starts_with("u_") {
                let hex = &id_str[2..];
                format!("https://cdnjs.cloudflare.com/ajax/libs/twemoji/14.0.2/72x72/{}.png", hex)
            } else {
                format!("https://cdn.discordapp.com/emojis/{}.png?size=48&quality=lossless", id_str)
            };

            let client = crate::utils::network_settings::shared_http_client();
            if let Ok(resp) = client.get(&url).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            // Save to disk cache
                            let _ = std::fs::write(&file_path, &bytes);

                            // Decode in background thread
                            if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                                if let Ok(mut guard) = cache_arc.memory_cache.lock() {
                                    if guard.len() >= 64 {
                                        guard.clear();
                                    }
                                    guard.insert(id_str.clone(), dec);
                                }

                                // Update Slint UI in place: sidebar slot and painted rows.
                                let id_clone = id_str.clone();
                                let cache_arc_ui = cache_arc.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        if let Some(img) = cache_arc_ui.get(&id_clone) {
                                            crate::ui::helpers::refresh_channel_emoji(&ui, &id_clone, &img);
                                            crate::ui::helpers::refresh_emoji_in_messages(&ui, &id_clone, &img);
                                        }
                                    }
                                });
                            }
                        }
                    }
                }

            if let Ok(mut inf) = cache_arc.in_flight.lock() {
                inf.remove(&id_str);
            }
        });
    }
}

fn decode_bytes_to_rgba(bytes: &[u8]) -> Option<DecodedEmoji> {
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        let (w, h) = (dyn_img.width(), dyn_img.height());
        if w > 1024 || h > 1024 {
            log::warn!("Emoji com dimensões excessivas rejeitado: {}x{}", w, h);
            return None;
        }
        let rgba = dyn_img.to_rgba8();
        let (width, height) = rgba.dimensions();
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            width,
            height,
        );
        Some(DecodedEmoji { buffer })
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct CustomEmojiEntry {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

static REGISTERED_CUSTOM_EMOJIS: OnceLock<Mutex<Vec<CustomEmojiEntry>>> = OnceLock::new();

pub fn get_custom_emojis_store() -> &'static Mutex<Vec<CustomEmojiEntry>> {
    REGISTERED_CUSTOM_EMOJIS.get_or_init(|| Mutex::new(Vec::with_capacity(128)))
}

pub fn register_custom_emoji(id: &str, name: &str, animated: bool) {
    if let Ok(mut store) = get_custom_emojis_store().lock() {
        if !store.iter().any(|e| e.id == id) {
            store.push(CustomEmojiEntry {
                id: id.to_string(),
                name: name.to_string(),
                animated,
            });
        }
    }
}

pub fn get_all_custom_emojis() -> Vec<CustomEmojiEntry> {
    get_custom_emojis_store().lock().map(|s| s.clone()).unwrap_or_default()
}
