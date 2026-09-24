use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use slint::Model;
use tokio::sync::Semaphore;

use crate::{AppWindow, ChatMessage};

struct AvatarPixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub struct AvatarCache {
    images: Mutex<HashMap<String, AvatarPixels>>,
    pending: Mutex<HashSet<String>>,
    failed: Mutex<HashSet<String>>,
    requests: Arc<Semaphore>,
}

static AVATAR_CACHE: OnceLock<Arc<AvatarCache>> = OnceLock::new();

pub fn get_avatar_cache() -> Arc<AvatarCache> {
    AVATAR_CACHE
        .get_or_init(|| {
            Arc::new(AvatarCache {
                images: Mutex::new(HashMap::with_capacity(64)),
                pending: Mutex::new(HashSet::new()),
                failed: Mutex::new(HashSet::new()),
                requests: Arc::new(Semaphore::new(4)),
            })
        })
        .clone()
}

impl AvatarCache {
    pub fn hydrate(&self, messages: &mut [ChatMessage], app: &slint::Weak<AppWindow>) {
        for message in messages {
            if !message.show_header || message.author_id.is_empty() || message.avatar_url.is_empty()
            {
                continue;
            }
            let url = message.avatar_url.as_str();
            if let Some(image) = self.get(url) {
                message.avatar_img = image;
            } else {
                self.fetch(url, app.clone());
            }
        }
    }

    fn get(&self, url: &str) -> Option<slint::Image> {
        let cache = self.images.lock().ok()?;
        let pixels = cache.get(url)?;
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &pixels.rgba,
            pixels.width,
            pixels.height,
        );
        Some(slint::Image::from_rgba8(buffer))
    }

    fn fetch(&self, url: &str, app: slint::Weak<AppWindow>) {
        if self.failed.lock().is_ok_and(|failed| failed.contains(url)) {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            if !pending.insert(url.to_owned()) {
                return;
            }
        } else {
            return;
        }

        let url = url.to_owned();
        let cache = get_avatar_cache();
        tokio::spawn(async move {
            let _permit = match cache.requests.acquire().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let client = crate::utils::apply_proxy_to_builder(
                reqwest::Client::builder().timeout(Duration::from_secs(6)),
            )
            .build();
            let pixels = if let Ok(client) = client {
                if let Ok(response) = client.get(&url).send().await {
                    if response.status().is_success()
                        && response.content_length().unwrap_or(0) <= 512 * 1024
                    {
                        response.bytes().await.ok().and_then(|bytes| {
                            if bytes.len() > 512 * 1024 {
                                return None;
                            }
                            let decoded = image::load_from_memory(&bytes).ok()?;
                            if decoded.width() > 1024 || decoded.height() > 1024 {
                                return None;
                            }
                            let thumbnail = decoded
                                .resize_to_fill(40, 40, image::imageops::FilterType::Triangle)
                                .to_rgba8();
                            Some(AvatarPixels {
                                width: 40,
                                height: 40,
                                rgba: thumbnail.into_raw(),
                            })
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(pixels) = pixels {
                if let Ok(mut images) = cache.images.lock() {
                    if images.len() >= 128 {
                        images.clear();
                    }
                    images.insert(url.clone(), pixels);
                }
                let ui_cache = cache.clone();
                let ui_url = url.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let (Some(ui), Some(image)) = (app.upgrade(), ui_cache.get(&ui_url)) {
                        let mut messages: Vec<ChatMessage> = ui.get_messages().iter().collect();
                        let mut changed = false;
                        for message in &mut messages {
                            if message.avatar_url == ui_url.as_str() {
                                message.avatar_img = image.clone();
                                changed = true;
                            }
                        }
                        if changed {
                            ui.set_messages(
                                std::rc::Rc::new(slint::VecModel::from(messages)).into(),
                            );
                        }
                    }
                });
            } else if let Ok(mut failed) = cache.failed.lock() {
                if failed.len() >= 128 {
                    failed.clear();
                }
                failed.insert(url.clone());
            }
            if let Ok(mut pending) = cache.pending.lock() {
                pending.remove(&url);
            }
        });
    }
}
