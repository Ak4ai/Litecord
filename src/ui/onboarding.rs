//! Community onboarding: the questionnaire Discord shows after joining a server
//! with onboarding enabled. Answers become roles and unlock channels.
//!
//! The gateway never says "show this"; the official client asks for
//! `GET /guilds/{id}/onboarding` when the member has not gone through it yet and
//! then posts the answers to `POST /guilds/{id}/onboarding-responses`.
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{info, warn};
use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::http::DiscordHttpClient;
use crate::{AppWindow, OnboardingOptionItem, OnboardingPromptItem};

struct PromptData {
    id: String,
    title: String,
    single_select: bool,
    required: bool,
    options: Vec<OnboardingOptionItem>,
}

#[derive(Clone, Default)]
struct CachedOnboarding {
    prompt_ids: Vec<String>,
    option_ids: Vec<Vec<String>>,
}

pub struct Submission {
    pub option_ids: Vec<String>,
    pub prompts_seen: Value,
    pub responses_seen: Value,
}

static CACHE: OnceLock<Mutex<HashMap<String, CachedOnboarding>>> = OnceLock::new();
/// Guilds whose questionnaire was already offered (shown, skipped or answered).
static HANDLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
/// Guilds whose questionnaire the user skipped during this session.
static DISMISSED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CachedOnboarding>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn handled() -> &'static Mutex<HashSet<String>> {
    HANDLED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn set_contains(set: &'static OnceLock<Mutex<HashSet<String>>>, guild_id: &str) -> bool {
    set.get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|s| s.contains(guild_id))
        .unwrap_or(false)
}

fn set_insert(set: &'static OnceLock<Mutex<HashSet<String>>>, guild_id: &str) {
    if let Ok(mut s) = set.get_or_init(|| Mutex::new(HashSet::new())).lock() {
        s.insert(guild_id.to_string());
    }
}

/// The user pressed "Skip for now" (or closed the modal): do not ask again now.
pub fn mark_dismissed(guild_id: &str) {
    set_insert(&DISMISSED, guild_id);
}

/// Answers were saved; nothing else to offer for this guild in this session.
pub fn mark_done(guild_id: &str) {
    set_insert(&DISMISSED, guild_id);
    set_insert(&HANDLED, guild_id);
}

/// Offers the questionnaire once for a guild, if it still has one pending.
pub fn maybe_show(
    http: &DiscordHttpClient,
    app_weak: slint::Weak<AppWindow>,
    guild_id: &str,
    guild_name: &str,
    permissions: &crate::channel_permissions::ChannelPermissions,
) {
    if guild_id.is_empty() || guild_id == "@me" || !permissions.has_feature("GUILD_ONBOARDING") {
        return;
    }
    if permissions.is_pending() {
        // Membership screening comes first: Discord rejects onboarding answers while
        // the member is pending, so asking here would only end in an error.
        info!("[onboarding] {} aguarda aceite das regras do servidor.", guild_id);
        return;
    }
    if set_contains(&HANDLED, guild_id) || set_contains(&DISMISSED, guild_id) {
        return;
    }
    set_insert(&HANDLED, guild_id);

    let http = http.clone();
    let guild_id = guild_id.to_string();
    let guild_name = guild_name.to_string();
    tokio::spawn(async move {
        let onboarding = match http.get_guild_onboarding(&guild_id).await {
            Ok(value) => value,
            Err(err) => {
                // Let a later visit try again instead of hiding the prompt forever.
                if let Ok(mut s) = handled().lock() {
                    s.remove(&guild_id);
                }
                warn!("Onboarding do servidor {} indisponível: {}", guild_id, err);
                return;
            }
        };
        let Some((rows, cached)) = build(&onboarding) else {
            info!("Servidor {} não tem perguntas de onboarding pendentes.", guild_id);
            return;
        };
        if let Ok(mut map) = cache().lock() {
            map.insert(guild_id.clone(), cached);
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_weak.upgrade() {
                let prompts: Vec<OnboardingPromptItem> = rows
                    .into_iter()
                    .map(|prompt| OnboardingPromptItem {
                        id: prompt.id.as_str().into(),
                        title: prompt.title.as_str().into(),
                        single_select: prompt.single_select,
                        required: prompt.required,
                        options: ModelRc::new(VecModel::from(prompt.options)),
                    })
                    .collect();
                ui.set_onboarding_guild_id(guild_id.into());
                ui.set_onboarding_guild_name(guild_name.into());
                ui.set_onboarding_prompts(ModelRc::new(VecModel::from(prompts)));
                ui.set_onboarding_submitting(false);
                ui.set_onboarding_error("".into());
                ui.set_show_onboarding_modal(true);
            }
        });
    });
}

fn build(onboarding: &Value) -> Option<(Vec<PromptData>, CachedOnboarding)> {
    if !onboarding["enabled"].as_bool().unwrap_or(false) {
        return None;
    }
    // `onboarding_prompts_seen` is only written when the flow is finished, so an
    // empty map means this member still has to go through it.
    let already_finished = onboarding["onboarding_prompts_seen"]
        .as_object()
        .is_some_and(|seen| !seen.is_empty());
    if already_finished {
        return None;
    }
    let responses: HashSet<&str> = onboarding["responses"]
        .as_array()
        .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut rows = Vec::new();
    let mut cached = CachedOnboarding::default();
    for prompt in onboarding["prompts"].as_array()? {
        if !prompt["in_onboarding"].as_bool().unwrap_or(false) {
            continue;
        }
        let options_json = match prompt["options"].as_array() {
            Some(options) if !options.is_empty() => options,
            _ => continue,
        };
        let prompt_id = prompt["id"].as_str()?.to_string();
        let mut options = Vec::with_capacity(options_json.len());
        let mut option_ids = Vec::with_capacity(options_json.len());
        for option in options_json {
            let option_id = match option["id"].as_str() {
                Some(id) => id.to_string(),
                None => continue,
            };
            option_ids.push(option_id.clone());
            options.push(OnboardingOptionItem {
                id: option_id.as_str().into(),
                title: option["title"].as_str().unwrap_or("").into(),
                description: option["description"].as_str().unwrap_or("").into(),
                selected: responses.contains(option_id.as_str()),
            });
        }
        if options.is_empty() {
            continue;
        }
        rows.push(PromptData {
            id: prompt_id.clone(),
            title: prompt["title"].as_str().unwrap_or("").to_string(),
            single_select: prompt["single_select"].as_bool().unwrap_or(false),
            required: prompt["required"].as_bool().unwrap_or(false),
            options,
        });
        cached.prompt_ids.push(prompt_id);
        cached.option_ids.push(option_ids);
    }
    if rows.is_empty() {
        None
    } else {
        Some((rows, cached))
    }
}

/// Selects or clears one option; single-select prompts keep only one choice.
pub fn toggle(ui: &AppWindow, prompt_index: usize, option_index: usize) {
    let prompts = ui.get_onboarding_prompts();
    let Some(prompt) = prompts.row_data(prompt_index) else { return };
    let single_select = prompt.single_select;
    let mut options: Vec<OnboardingOptionItem> = (0..prompt.options.row_count())
        .filter_map(|i| prompt.options.row_data(i))
        .collect();
    let Some(option) = options.get_mut(option_index) else { return };
    let select = !option.selected;
    if select && single_select {
        for other in options.iter_mut() {
            other.selected = false;
        }
    }
    options[option_index].selected = select;

    let mut rows: Vec<OnboardingPromptItem> = (0..prompts.row_count())
        .filter_map(|i| prompts.row_data(i))
        .collect();
    rows[prompt_index].options = ModelRc::new(VecModel::from(options));
    ui.set_onboarding_prompts(ModelRc::new(VecModel::from(rows)));
    ui.set_onboarding_error("".into());
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reads the current selections and builds the body the official client sends.
pub fn take_submission(ui: &AppWindow) -> Result<Submission, String> {
    let guild_id = ui.get_onboarding_guild_id().to_string();
    let cached = cache()
        .lock()
        .ok()
        .and_then(|map| map.get(&guild_id).cloned())
        .ok_or_else(|| crate::utils::i18n_ui::ui_text().onboarding_error.to_string())?;
    let prompts = ui.get_onboarding_prompts();
    let now = now_ms();
    let mut option_ids = Vec::new();
    let mut prompts_seen = serde_json::Map::new();
    let mut responses_seen = serde_json::Map::new();

    for prompt_index in 0..prompts.row_count() {
        let Some(prompt) = prompts.row_data(prompt_index) else { continue };
        let Some(prompt_id) = cached.prompt_ids.get(prompt_index) else { continue };
        prompts_seen.insert(prompt_id.clone(), json!(now));
        let mut chosen = 0;
        for option_index in 0..prompt.options.row_count() {
            let Some(option) = prompt.options.row_data(option_index) else { continue };
            let Some(option_id) = cached.option_ids.get(prompt_index).and_then(|ids| ids.get(option_index)) else {
                continue;
            };
            responses_seen.insert(option_id.clone(), json!(now));
            if option.selected {
                option_ids.push(option_id.clone());
                chosen += 1;
            }
        }
        if prompt.required && chosen == 0 {
            return Err(crate::utils::i18n_ui::ui_text().onboarding_required_error.to_string());
        }
    }

    Ok(Submission {
        option_ids,
        prompts_seen: Value::Object(prompts_seen),
        responses_seen: Value::Object(responses_seen),
    })
}

pub fn request_body(submission: &Submission) -> Value {
    json!({
        "onboarding_responses": submission.option_ids,
        "onboarding_prompts_seen": submission.prompts_seen,
        "onboarding_responses_seen": submission.responses_seen,
    })
}
