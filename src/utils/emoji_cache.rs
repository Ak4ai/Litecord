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
    last_used: u64,
}

/// Decoded emojis kept in RAM. The cache evicts the least recently used entry
/// instead of dropping everything at once: a hard clear made a busy channel
/// re-read and re-decode dozens of PNGs from disk in a burst.
#[derive(Default)]
struct MemoryStore {
    entries: HashMap<String, DecodedEmoji>,
    clock: u64,
}

/// Decoded emoji pixels are tiny (a 48x48 RGBA square is ~9 KB), so this holds
/// far more than the previous 64 entries while staying well under a megabyte.
const MAX_MEMORY_EMOJIS: usize = 512;

/// PNGs kept on disk. The oldest ones are dropped, like the avatar cache does.
const MAX_DISK_EMOJIS: usize = 2048;

impl MemoryStore {
    fn get(&mut self, emoji_id: &str) -> Option<slint::Image> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(emoji_id)?;
        entry.last_used = self.clock;
        Some(slint::Image::from_rgba8(entry.buffer.clone()))
    }

    fn insert(&mut self, emoji_id: String, decoded: DecodedEmoji) {
        self.clock = self.clock.wrapping_add(1);
        let last_used = self.clock;
        if !self.entries.contains_key(&emoji_id) && self.entries.len() >= MAX_MEMORY_EMOJIS {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(emoji_id, DecodedEmoji { last_used, ..decoded });
    }

    fn contains(&self, emoji_id: &str) -> bool {
        self.entries.contains_key(emoji_id)
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

pub struct EmojiCache {
    memory_cache: Mutex<MemoryStore>,
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
            memory_cache: Mutex::new(MemoryStore::default()),
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
        if let Ok(mut guard) = self.memory_cache.lock() {
            if let Some(image) = guard.get(emoji_id) {
                return Some(image);
            }
        }

        // 2. Check disk cache
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let image = slint::Image::from_rgba8(dec.buffer.clone());
                    if let Ok(mut guard) = self.memory_cache.lock() {
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
        let mut guard = self.memory_cache.lock().ok()?;
        guard.get(emoji_id)
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
            if guard.contains(emoji_id) {
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
                let buffer = dec.buffer.clone();
                if let Ok(mut guard) = cache_arc.memory_cache.lock() {
                    guard.insert(id_str.clone(), dec);
                }
                let id_ui = id_str.clone();
                let cache_ui = cache_arc.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_weak.upgrade() {
                        if let Some(img) = cache_ui.get_cached(&id_ui) {
                            crate::ui::helpers::refresh_channel_emoji(&ui, &id_ui, &img);
                            crate::ui::helpers::queue_emoji_refresh(&app_weak, &id_ui, &buffer);
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
            if guard.contains(emoji_id) {
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
                            // Save to disk and decode on a blocking worker: the image
                            // crate is CPU heavy, the file write is blocking I/O, and
                            // both run inside the async runtime.
                            let disk_path = file_path.clone();
                            let disk_dir = cache_arc.disk_dir.clone();
                            let decoded = tokio::task::spawn_blocking(move || {
                                let _ = std::fs::write(&disk_path, &bytes);
                                prune_disk_emojis(&disk_dir, MAX_DISK_EMOJIS);
                                decode_bytes_to_rgba(&bytes)
                            })
                            .await
                            .ok()
                            .flatten();
                            if let Some(dec) = decoded {
                                // The pixels travel as a raw buffer: a `slint::Image`
                                // cannot be moved between threads, and building it
                                // again on the UI thread could hit the disk when the
                                // cache entry was evicted in between.
                                let buffer = dec.buffer.clone();
                                if let Ok(mut guard) = cache_arc.memory_cache.lock() {
                                    guard.insert(id_str.clone(), dec);
                                }

                                // Update Slint UI in place: sidebar slot and painted rows.
                                let id_clone = id_str.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        let image = slint::Image::from_rgba8(buffer.clone());
                                        crate::ui::helpers::refresh_channel_emoji(&ui, &id_clone, &image);
                                        crate::ui::helpers::queue_emoji_refresh(&app_weak, &id_clone, &buffer);
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

/// Keeps the emoji PNG cache bounded: without this every emoji ever seen left a
/// file behind forever (the avatar cache already prunes its own directory).
fn prune_disk_emojis(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.ends_with(".png") || name.len() < 5 {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((entry.path(), metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)))
        })
        .collect();
    if files.len() > keep {
        files.sort_by_key(|(_, modified)| *modified);
        let excess = files.len() - keep;
        for (path, _) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
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
        Some(DecodedEmoji { buffer, last_used: 0 })
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

/// Custom emojis registered from the gateway payloads.
///
/// A map instead of a list: registering one emoji used to scan every known
/// emoji, so a big account paid an O(n^2) cost on every `READY`.
static REGISTERED_CUSTOM_EMOJIS: OnceLock<Mutex<HashMap<String, CustomEmojiEntry>>> = OnceLock::new();
/// Cap of the registry: a large account sees thousands of custom emojis and the
/// picker shows a fixed list, so the map is emptied when it grows too much.
const MAX_REGISTERED_CUSTOM_EMOJIS: usize = 4096;

pub fn get_custom_emojis_store() -> &'static Mutex<HashMap<String, CustomEmojiEntry>> {
    REGISTERED_CUSTOM_EMOJIS.get_or_init(|| Mutex::new(HashMap::with_capacity(256)))
}

pub fn register_custom_emoji(id: &str, name: &str, animated: bool) {
    if id.is_empty() {
        return;
    }
    if let Ok(mut store) = get_custom_emojis_store().lock() {
        if store.len() >= MAX_REGISTERED_CUSTOM_EMOJIS && !store.contains_key(id) {
            store.clear();
        }
        store.insert(
            id.to_string(),
            CustomEmojiEntry {
                id: id.to_string(),
                name: name.to_string(),
                animated,
            },
        );
    }
}

pub fn get_all_custom_emojis() -> Vec<CustomEmojiEntry> {
    get_custom_emojis_store()
        .lock()
        .map(|s| s.values().cloned().collect())
        .unwrap_or_default()
}

/// Forgets the registered custom emojis: they belong to the signed in account.
pub fn clear_registered_custom_emojis() {
    if let Ok(mut store) = get_custom_emojis_store().lock() {
        store.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emoji() -> DecodedEmoji {
        DecodedEmoji {
            buffer: slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(2, 2),
            last_used: 0,
        }
    }

    #[test]
    fn memory_cache_evicts_only_the_least_recently_used_emoji() {
        let mut store = MemoryStore::default();
        for index in 0..MAX_MEMORY_EMOJIS {
            store.insert(format!("e{index}"), emoji());
        }
        assert_eq!(store.entries.len(), MAX_MEMORY_EMOJIS);

        // Using one emoji makes it the most recent one, so the next insert drops
        // another entry instead of the whole cache.
        assert!(store.get("e0").is_some());
        store.insert("new".into(), emoji());
        assert_eq!(store.entries.len(), MAX_MEMORY_EMOJIS);
        assert!(store.entries.contains_key("e0"));
        assert!(!store.entries.contains_key("e1"));
    }

    #[test]
    fn disk_cache_prunes_the_oldest_png_files_beyond_the_limit() {
        let dir = std::env::temp_dir().join(format!(
            "litecord-emoji-prune-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..5 {
            let path = dir.join(format!("{index}.png"));
            std::fs::write(&path, b"x").unwrap();
            // Distinct modification times so the order is deterministic.
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
                let _ = file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(index));
            }
        }
        prune_disk_emojis(&dir, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["2.png", "3.png", "4.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_emoji_registry_is_bounded_and_cleared_with_the_session() {
        for index in 0..(MAX_REGISTERED_CUSTOM_EMOJIS + 10) {
            register_custom_emoji(&format!("{index}"), "emoji", false);
        }
        assert!(get_custom_emojis_store().lock().unwrap().len() <= MAX_REGISTERED_CUSTOM_EMOJIS);
        clear_registered_custom_emojis();
        assert!(get_custom_emojis_store().lock().unwrap().is_empty());
    }
}
