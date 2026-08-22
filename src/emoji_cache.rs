use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use log::warn;
use slint::Model;
use tokio::sync::Semaphore;

#[derive(Clone)]
struct DecodedEmoji {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
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
            .unwrap_or_else(|_| PathBuf::from("."));
        disk_dir.push("Litecord");
        disk_dir.push("cache");
        disk_dir.push("emojis");

        if let Err(e) = std::fs::create_dir_all(&disk_dir) {
            warn!("Não foi possível criar diretório de cache de emojis: {}", e);
        }

        Arc::new(EmojiCache {
            memory_cache: Mutex::new(HashMap::with_capacity(128)),
            in_flight: Mutex::new(HashSet::new()),
            active_channel: Mutex::new(String::new()),
            active_generation: AtomicU64::new(1),
            semaphore: Arc::new(Semaphore::new(6)), // Up to 6 parallel downloads
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
                        if guard.len() >= 250 {
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

    /// Prioritized async downloader: prioritizes emojis from the active screen/channel
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
                                    if guard.len() >= 250 {
                                        guard.clear();
                                    }
                                    guard.insert(id_str.clone(), dec);
                                }

                                // Update Slint UI messages in-place immediately!
                                let id_clone = id_str.clone();
                                let cache_arc_ui = cache_arc.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        if let Some(img) = cache_arc_ui.get(&id_clone) {
                                            let current_msgs: Vec<crate::ChatMessage> = ui.get_messages().iter().collect();
                                            let mut any_changed = false;
                                            let mut updated_msgs = Vec::with_capacity(current_msgs.len());

                                            for mut msg in current_msgs {
                                                let mut msg_changed = false;

                                                // Update content_lines
                                                let current_content_lines: Vec<crate::MessageLine> = msg.content_lines.iter().collect();
                                                let mut updated_content_lines = Vec::with_capacity(current_content_lines.len());
                                                for line in current_content_lines {
                                                    let blocks: Vec<crate::MessageBlock> = line.blocks.iter().collect();
                                                    let mut updated_blocks = Vec::with_capacity(blocks.len());
                                                    for mut b in blocks {
                                                        if b.is_emoji && b.emoji_id == id_clone.as_str() {
                                                            b.emoji_img = img.clone();
                                                            msg_changed = true;
                                                        }
                                                        updated_blocks.push(b);
                                                    }
                                                    updated_content_lines.push(crate::MessageLine {
                                                        blocks: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_blocks))),
                                                    });
                                                }

                                                // Update embed_lines
                                                let current_embed_lines: Vec<crate::MessageLine> = msg.embed_lines.iter().collect();
                                                let mut updated_embed_lines = Vec::with_capacity(current_embed_lines.len());
                                                for line in current_embed_lines {
                                                    let blocks: Vec<crate::MessageBlock> = line.blocks.iter().collect();
                                                    let mut updated_blocks = Vec::with_capacity(blocks.len());
                                                    for mut b in blocks {
                                                        if b.is_emoji && b.emoji_id == id_clone.as_str() {
                                                            b.emoji_img = img.clone();
                                                            msg_changed = true;
                                                        }
                                                        updated_blocks.push(b);
                                                    }
                                                    updated_embed_lines.push(crate::MessageLine {
                                                        blocks: slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_blocks))),
                                                    });
                                                }

                                                if msg_changed {
                                                    msg.content_lines = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_content_lines)));
                                                    msg.embed_lines = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_embed_lines)));
                                                    any_changed = true;
                                                }
                                                updated_msgs.push(msg);
                                            }

                                            if any_changed {
                                                ui.set_messages(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_msgs))));
                                            }
                                        }
                                    }
                                });
                            }
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
