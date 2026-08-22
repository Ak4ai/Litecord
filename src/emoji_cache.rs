use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use log::warn;

#[derive(Clone)]
struct DecodedEmoji {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub struct EmojiCache {
    memory_cache: Mutex<HashMap<String, DecodedEmoji>>,
    disk_dir: PathBuf,
}

static EMOJI_CACHE: OnceLock<Arc<EmojiCache>> = OnceLock::new();

pub fn get_emoji_cache() -> Arc<EmojiCache> {
    EMOJI_CACHE.get_or_init(|| {
        let mut disk_dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        disk_dir.push("Litecord");
        disk_dir.push("cache");
        disk_dir.push("emojis");

        if let Err(e) = std::fs::create_dir_all(&disk_dir) {
            warn!("Não foi possível criar diretório de cache de emojis: {}", e);
        }

        Arc::new(EmojiCache {
            memory_cache: Mutex::new(HashMap::with_capacity(128)),
            disk_dir,
        })
    }).clone()
}

impl EmojiCache {
    /// Returns cached Slint Image if already in RAM, otherwise checks disk cache.
    pub fn get(&self, emoji_id: &str) -> Option<slint::Image> {
        if emoji_id.is_empty() {
            return None;
        }

        // 1. Check in-memory cache
        if let Ok(guard) = self.memory_cache.lock() {
            if let Some(dec) = guard.get(emoji_id) {
                let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &dec.rgba,
                    dec.width,
                    dec.height,
                );
                return Some(slint::Image::from_rgba8(pixel_buffer));
            }
        }

        // 2. Check disk cache
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &dec.rgba,
                        dec.width,
                        dec.height,
                    );
                    if let Ok(mut guard) = self.memory_cache.lock() {
                        if guard.len() >= 200 {
                            guard.clear();
                        }
                        guard.insert(emoji_id.to_string(), dec);
                    }
                    return Some(slint::Image::from_rgba8(pixel_buffer));
                }
            }
        }

        None
    }

    /// Background async downloader for uncached Discord CDN emojis
    pub fn fetch_async(&self, emoji_id: &str, app_weak: slint::Weak<crate::AppWindow>) {
        if emoji_id.is_empty() {
            return;
        }

        if let Ok(guard) = self.memory_cache.lock() {
            if guard.contains_key(emoji_id) {
                return;
            }
        }

        let cache_arc = get_emoji_cache();
        let id_str = emoji_id.to_string();
        let file_path = self.disk_dir.join(format!("{}.png", emoji_id));

        tokio::spawn(async move {
            let url = format!("https://cdn.discordapp.com/emojis/{}.png?size=48&quality=lossless", id_str);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build();

            if let Ok(client) = client {
                if let Ok(resp) = client.get(&url).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            // Save to disk cache
                            let _ = std::fs::write(&file_path, &bytes);

                            // Decode in background thread
                            if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                                if let Ok(mut guard) = cache_arc.memory_cache.lock() {
                                    if guard.len() >= 200 {
                                        guard.clear();
                                    }
                                    guard.insert(id_str.clone(), dec);
                                }

                                // Trigger light UI redraw
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        ui.set_is_loading_older_messages(ui.get_is_loading_older_messages());
                                    }
                                });
                            }
                        }
                    }
                }
            }
        });
    }
}

fn decode_bytes_to_rgba(bytes: &[u8]) -> Option<DecodedEmoji> {
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        let rgba = dyn_img.to_rgba8();
        let (width, height) = rgba.dimensions();
        Some(DecodedEmoji {
            width,
            height,
            rgba: rgba.into_raw(),
        })
    } else {
        None
    }
}
