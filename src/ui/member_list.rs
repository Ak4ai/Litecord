//! Server member list (optional, off by default): a thin column of avatars on
//! the right that widens on hover to show names. Members come from the gateway's
//! member list subscription (op 14 / GUILD_MEMBER_LIST_UPDATE), the same one the
//! official client uses, asked only for the part of the list on screen and only
//! while the column is enabled.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;
use slint::{Model, ModelRc, VecModel};

use crate::http::DiscordHttpClient;

/// Rows per gateway range, as Discord pages the list.
const PAGE: usize = 100;
/// The list keeps at most this many rows (Discord only lists offline members of
/// small servers anyway).
const MAX_ROWS: usize = 2000;

#[derive(Clone, Default)]
enum Row {
    #[default]
    Unknown,
    Group { id: String, count: usize },
    Member { id: String, name: String, tag: String, avatar_url: String, status: String },
}

#[derive(Default)]
struct State {
    guild: String,
    channel: String,
    rows: Vec<Row>,
    /// Role id -> name, for the group headers.
    roles: HashMap<String, String>,
    /// First page asked for besides the first one.
    page: usize,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static GUILD: Mutex<String> = Mutex::new(String::new());
/// Lists of the last servers left, shown at once when coming back while the
/// gateway sends the fresh one. A few thousand small rows at most.
static CACHE: Mutex<Vec<(String, Vec<Row>, HashMap<String, String>)>> = Mutex::new(Vec::new());
const CACHED_SERVERS: usize = 4;
static APP: std::sync::OnceLock<slint::Weak<crate::AppWindow>> = std::sync::OnceLock::new();

pub fn set_app(app_weak: slint::Weak<crate::AppWindow>) {
    let _ = APP.set(app_weak);
}

fn enabled() -> bool {
    crate::utils::video_settings::get_show_member_list()
}

/// The server now open ("@me" for direct messages).
pub fn set_guild(guild_id: &str) {
    *GUILD.lock().unwrap_or_else(|e| e.into_inner()) = guild_id.to_string();
}

/// A text channel was opened: show its members (when the column is on).
pub fn open_channel(http: &DiscordHttpClient, app_weak: &slint::Weak<crate::AppWindow>, channel_id: &str) {
    let guild = GUILD.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !enabled() || guild.is_empty() || guild == "@me" {
        return;
    }
    let roles_known = {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Channels of one server nearly always share its list: keep showing it
        // until the new one arrives, instead of blank rows.
        let (rows, roles) = match st.take() {
            Some(s) if s.guild == guild => (s.rows, s.roles),
            other => {
                let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(old) = other {
                    cache.retain(|(g, _, _)| *g != old.guild);
                    cache.push((old.guild, old.rows, old.roles));
                    if cache.len() > CACHED_SERVERS {
                        cache.remove(0);
                    }
                }
                cache
                    .iter()
                    .position(|(g, _, _)| *g == guild)
                    .map(|i| {
                        let (_, rows, roles) = cache.remove(i);
                        (rows, roles)
                    })
                    .unwrap_or_default()
            }
        };
        let known = !roles.is_empty();
        *st = Some(State { guild: guild.clone(), channel: channel_id.to_string(), rows, roles, page: 0 });
        known
    };
    push_ui(app_weak);
    subscribe(&guild, channel_id, 0);
    if !roles_known {
        let (http, w) = (http.clone(), app_weak.clone());
        tokio::spawn(async move {
            let Ok(list) = http.get_json(&format!("/guilds/{}/roles", guild)).await else { return };
            let roles: HashMap<String, String> = list
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|r| Some((r["id"].as_str()?.to_string(), r["name"].as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                if st.guild == guild {
                    st.roles = roles;
                }
            }
            push_ui(&w);
        });
    }
}

/// Asks again for the open list (after the gateway (re)connects).
pub fn resubscribe() {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = st.as_ref() {
        subscribe(&s.guild, &s.channel, s.page);
    }
}

/// Turned off in settings: drop everything.
/// The list stays in memory, so turning the column back on shows it at once.
pub fn clear(app_weak: &slint::Weak<crate::AppWindow>) {
    push_ui(app_weak);
}

fn subscribe(guild: &str, channel: &str, page: usize) {
    // The top of the list plus the page on screen and the next one, so rows
    // across a page boundary are never left out (Discord takes 3 ranges).
    let mut ranges = vec![[0, PAGE - 1]];
    for p in [page, page + 1] {
        if p > 0 {
            ranges.push([p * PAGE, p * PAGE + PAGE - 1]);
        }
    }
    crate::gateway::client::send_gateway(serde_json::json!({
        "op": 14,
        "d": { "guild_id": guild, "typing": true, "activities": false, "threads": false,
               "channels": { channel: ranges } }
    }));
}

/// The list was scrolled to `first_row`: ask for that part of it.
pub fn scrolled(first_row: i32) {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = st.as_mut() else { return };
    let page = first_row.max(0) as usize / PAGE;
    if page != s.page {
        s.page = page;
        subscribe(&s.guild, &s.channel, page);
    }
}

fn parse_item(item: &Value) -> Row {
    if let Some(g) = item.get("group") {
        return Row::Group { id: g["id"].as_str().unwrap_or("").to_string(), count: g["count"].as_u64().unwrap_or(0) as usize };
    }
    let Some(m) = item.get("member") else { return Row::Unknown };
    let user = &m["user"];
    let id = user["id"].as_str().unwrap_or("").to_string();
    let username = user["username"].as_str().unwrap_or("");
    let name = m["nick"]
        .as_str()
        .or_else(|| user["global_name"].as_str())
        .unwrap_or(username)
        .to_string();
    let avatar = user["avatar"].as_str().unwrap_or("");
    let avatar_url = if crate::ui::message_grouping::valid_avatar_hash(avatar) {
        format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.webp?size=64")
    } else {
        String::new()
    };
    let status = m["presence"]["status"].as_str().unwrap_or("offline").to_string();
    Row::Member { id, name, tag: username.to_string(), avatar_url, status }
}

/// Applies a GUILD_MEMBER_LIST_UPDATE payload.
pub fn apply(d: &Value) {
    let Some(app_weak) = APP.get() else { return };
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = st.as_mut() else { return };
        if d["guild_id"].as_str() != Some(s.guild.as_str()) {
            return;
        }
        if !enabled() {
            // Kept up to date (cheap) but not drawn while the column is off.
            apply_ops(s, d);
            return;
        }
        if !apply_ops(s, d) {
            return;
        }
        if d["ops"].as_array().is_some_and(|o| o.iter().any(|op| op["op"] == "SYNC")) {
            log::info!("👥 [MEMBROS] Lista recebida: {} linhas", s.rows.len());
        }
    }
    push_ui(app_weak);
}

/// Returns whether anything changed.
fn apply_ops(s: &mut State, d: &Value) -> bool {
    // The list length: one header per group plus its members.
    if let Some(groups) = d["groups"].as_array() {
        let total: usize = groups.iter().map(|g| 1 + g["count"].as_u64().unwrap_or(0) as usize).sum();
        s.rows.resize(total.min(MAX_ROWS), Row::Unknown);
    }
    let mut changed = false;
    for op in d["ops"].as_array().into_iter().flatten() {
        match op["op"].as_str().unwrap_or("") {
            "SYNC" => {
                let start = op["range"][0].as_u64().unwrap_or(0) as usize;
                for (i, item) in op["items"].as_array().into_iter().flatten().enumerate() {
                    let at = start + i;
                    if at >= MAX_ROWS {
                        break;
                    }
                    if at >= s.rows.len() {
                        s.rows.resize(at + 1, Row::Unknown);
                    }
                    s.rows[at] = parse_item(item);
                }
                changed = true;
            }
            "INSERT" => {
                let at = op["index"].as_u64().unwrap_or(0) as usize;
                if at <= s.rows.len() && at < MAX_ROWS {
                    s.rows.insert(at, parse_item(&op["item"]));
                    s.rows.truncate(MAX_ROWS);
                    changed = true;
                }
            }
            "UPDATE" => {
                let at = op["index"].as_u64().unwrap_or(0) as usize;
                if let Some(r) = s.rows.get_mut(at) {
                    *r = parse_item(&op["item"]);
                    changed = true;
                }
            }
            "DELETE" => {
                let at = op["index"].as_u64().unwrap_or(0) as usize;
                if at < s.rows.len() {
                    s.rows.remove(at);
                    changed = true;
                }
            }
            // A range no longer followed: keep showing what it had (better
            // than blank rows); it is synced again when scrolled back to.
            "INVALIDATE" => {}
            _ => {}
        }
    }
    changed
}

fn group_name(id: &str, roles: &HashMap<String, String>) -> String {
    match id {
        "online" => "Online".to_string(),
        "offline" => "Offline".to_string(),
        _ => roles.get(id).cloned().unwrap_or_else(|| "Online".to_string()),
    }
}

/// Sends the rows to the UI (rebuilding only what differs).
fn push_ui(app_weak: &slint::Weak<crate::AppWindow>) {
    let rows: Vec<(String, String, String, String, String, bool)> = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        match st.as_ref().filter(|_| enabled()) {
            None => Vec::new(),
            Some(s) => s
                .rows
                .iter()
                .map(|r| match r {
                    Row::Unknown => (String::new(), String::new(), String::new(), String::new(), String::new(), false),
                    Row::Group { id, count } => {
                        (format!("{} — {}", group_name(id, &s.roles), count), String::new(), String::new(), String::new(), String::new(), true)
                    }
                    Row::Member { id, name, tag, avatar_url, status } => {
                        (String::new(), id.clone(), name.clone(), tag.clone(), format!("{}|{}", status, avatar_url), false)
                    }
                })
                .collect(),
        }
    };
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = w.upgrade() else { return };
        let avatars = crate::utils::avatar_cache::get_avatar_cache();
        let items: Vec<crate::MemberItem> = rows
            .into_iter()
            .map(|(header, id, name, tag, sa, is_group)| {
                let (status, url) = sa.split_once('|').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
                let slot = if url.is_empty() { 0 } else { avatars.slot_for(&url) };
                crate::MemberItem {
                    header: header.into(),
                    known: !id.is_empty() || is_group,
                    initials: crate::ui::helpers::get_guild_initials(&name).into(),
                    name: name.into(),
                    tag: tag.into(),
                    status: status.into(),
                    avatar_url: url.into(),
                    avatar_slot: slot,
                }
            })
            .collect();
        let model = ui.get_member_rows();
        if let Some(vm) = model.as_any().downcast_ref::<VecModel<crate::MemberItem>>() {
            // Update in place so the list keeps its scroll position.
            let old = vm.row_count();
            for (i, it) in items.iter().enumerate() {
                if i < old {
                    if vm.row_data(i).as_ref() != Some(it) {
                        vm.set_row_data(i, it.clone());
                    }
                } else {
                    vm.push(it.clone());
                }
            }
            for _ in items.len()..old {
                vm.remove(vm.row_count() - 1);
            }
        } else {
            ui.set_member_rows(ModelRc::new(VecModel::from(items)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_sync_insert_update_and_delete() {
        let mut s = State::default();
        let d: Value = serde_json::from_str(r#"{"groups":[{"id":"online","count":2},{"id":"offline","count":1}],"ops":[
            {"op":"SYNC","range":[0,99],"items":[
                {"group":{"id":"online","count":2}},
                {"member":{"user":{"id":"1","username":"ana","global_name":"Ana","avatar":null},"presence":{"status":"online"}}},
                {"member":{"user":{"id":"2","username":"bia"},"nick":"Bia!","presence":{"status":"dnd"}}},
                {"group":{"id":"offline","count":1}},
                {"member":{"user":{"id":"3","username":"caio"},"presence":{"status":"offline"}}}]}]}"#).unwrap();
        assert!(apply_ops(&mut s, &d));
        assert_eq!(s.rows.len(), 5);
        assert!(matches!(&s.rows[2], Row::Member { name, status, .. } if name == "Bia!" && status == "dnd"));
        let d: Value = serde_json::from_str(r#"{"ops":[
            {"op":"DELETE","index":4},
            {"op":"UPDATE","index":1,"item":{"member":{"user":{"id":"1","username":"ana"},"presence":{"status":"idle"}}}},
            {"op":"INSERT","index":3,"item":{"member":{"user":{"id":"3","username":"caio"},"presence":{"status":"online"}}}}]}"#).unwrap();
        assert!(apply_ops(&mut s, &d));
        assert_eq!(s.rows.len(), 5);
        assert!(matches!(&s.rows[1], Row::Member { status, .. } if status == "idle"));
        assert!(matches!(&s.rows[3], Row::Member { id, .. } if id == "3"));
        assert!(matches!(&s.rows[4], Row::Group { .. }));
    }
}
