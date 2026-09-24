use chrono::{DateTime, Datelike, Local, NaiveDate};
use serde_json::Value;

use crate::ChatMessage;

const GROUP_WINDOW_SECONDS: i64 = 5 * 60;

/// Keep the Discord metadata on each independent message, including messages
/// loaded from history and messages received through the gateway.
pub fn apply_message_metadata(message: &mut ChatMessage, source: &Value) {
    let author_id = if matches!(source["type"].as_i64().unwrap_or(0), 0 | 19) {
        source["author"]["id"]
            .as_str()
            .or_else(|| source["webhook_id"].as_str())
            .unwrap_or("")
    } else {
        ""
    };
    let created_at = source["timestamp"].as_str().unwrap_or("");
    set_message_metadata(message, author_id, created_at);
    message.avatar_url = avatar_url_for_message(source).into();
    message.avatar_initial = message
        .author
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
        .into();
    message.is_bot = source["author"]["bot"].as_bool().unwrap_or(false);
}

/// Only construct CDN paths from validated Discord ids and hashes, never a
/// user-provided URL. Member avatars take precedence over account avatars.
pub fn avatar_url_for_message(source: &Value) -> String {
    let user_id = source["author"]["id"].as_str().unwrap_or("");
    if user_id.is_empty() || !user_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return String::new();
    }
    let member_hash = source["member"]["avatar"].as_str().unwrap_or("");
    let guild_id = source["guild_id"].as_str().unwrap_or("");
    if !member_hash.is_empty()
        && guild_id.bytes().all(|byte| byte.is_ascii_digit())
        && !guild_id.is_empty()
        && valid_avatar_hash(member_hash)
    {
        return format!("https://cdn.discordapp.com/guilds/{guild_id}/users/{user_id}/avatars/{member_hash}.webp?size=64");
    }
    let hash = source["author"]["avatar"].as_str().unwrap_or("");
    if valid_avatar_hash(hash) {
        format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.webp?size=64")
    } else {
        String::new()
    }
}

fn valid_avatar_hash(hash: &str) -> bool {
    let hex = hash.strip_prefix("a_").unwrap_or(hash);
    !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn set_message_metadata(message: &mut ChatMessage, author_id: &str, created_at: &str) {
    message.author_id = author_id.into();
    message.created_at = created_at.into();

    if let Some(sent_at) = parse_local_time(message.created_at.as_str()) {
        message.timestamp = sent_at.format("%d/%m/%Y %H:%M").to_string().into();
        message.hover_timestamp = sent_at.format("%H:%M").to_string().into();
    }
}

/// Recompute the visual boundaries after a history prepend, live append, edit,
/// or deletion. This never combines message records or changes their IDs.
pub fn regroup_messages(messages: &mut [ChatMessage]) {
    let mut previous: Option<(String, String, DateTime<Local>)> = None;
    let mut previous_day: Option<NaiveDate> = None;

    for message in messages {
        let sent_at = parse_local_time(message.created_at.as_str());
        let is_regular_message =
            !message.id.is_empty() && !message.author_id.is_empty() && sent_at.is_some();

        message.day_label = if is_regular_message {
            let day = sent_at
                .as_ref()
                .expect("regular messages have a timestamp")
                .date_naive();
            let label = if previous_day != Some(day) {
                format_day(day)
            } else {
                String::new()
            };
            previous_day = Some(day);
            label.into()
        } else {
            "".into()
        };

        message.show_header = !is_regular_message
            || !previous
                .as_ref()
                .is_some_and(|(author_id, author, previous_time)| {
                    let time = sent_at.as_ref().expect("regular messages have a timestamp");
                    author_id == message.author_id.as_str()
                        && author == message.author.as_str()
                        && previous_time.date_naive() == time.date_naive()
                        && (0..=GROUP_WINDOW_SECONDS)
                            .contains(&time.signed_duration_since(*previous_time).num_seconds())
                });

        previous = if is_regular_message {
            Some((
                message.author_id.to_string(),
                message.author.to_string(),
                sent_at.expect("regular messages have a timestamp"),
            ))
        } else {
            None
        };
    }
}

fn format_day(day: NaiveDate) -> String {
    const MONTHS: [&str; 12] = [
        "janeiro",
        "fevereiro",
        "março",
        "abril",
        "maio",
        "junho",
        "julho",
        "agosto",
        "setembro",
        "outubro",
        "novembro",
        "dezembro",
    ];
    format!(
        "{} de {} de {}",
        day.day(),
        MONTHS[day.month0() as usize],
        day.year()
    )
}

fn parse_local_time(timestamp: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|time| time.with_timezone(&Local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_uses_discord_timestamp_and_author_id() {
        let mut message = ChatMessage::default();
        let source = serde_json::json!({
            "author": { "id": "42" },
            "timestamp": "2026-09-24T12:34:56.000Z"
        });
        apply_message_metadata(&mut message, &source);

        assert_eq!(message.author_id.as_str(), "42");
        assert_eq!(message.created_at.as_str(), "2026-09-24T12:34:56.000Z");
        assert!(!message.timestamp.is_empty());
        assert!(!message.hover_timestamp.is_empty());
    }

    #[test]
    fn avatar_urls_are_bounded_to_discord_cdn_paths() {
        let source = serde_json::json!({"author": {"id": "42", "avatar": "a_abcdef"}});
        assert_eq!(
            avatar_url_for_message(&source),
            "https://cdn.discordapp.com/avatars/42/a_abcdef.webp?size=64"
        );
        let invalid = serde_json::json!({"author": {"id": "42/other", "avatar": "abc"}});
        assert!(avatar_url_for_message(&invalid).is_empty());
    }

    #[test]
    fn grouping_respects_author_time_and_system_boundaries() {
        fn message(author_id: &str, time: &str) -> ChatMessage {
            ChatMessage {
                id: "message".into(),
                author: author_id.into(),
                author_id: author_id.into(),
                created_at: time.into(),
                ..Default::default()
            }
        }

        let mut messages = vec![
            message("1", "2026-09-24T12:00:00Z"),
            message("1", "2026-09-24T12:04:00Z"),
            message("2", "2026-09-24T12:05:00Z"),
            message("1", "2026-09-24T12:06:00Z"),
            message("1", "2026-09-24T12:12:00Z"),
            message("1", "2026-09-25T12:00:00Z"),
            ChatMessage::default(),
            message("1", "2026-09-25T12:01:00Z"),
        ];
        regroup_messages(&mut messages);

        let headers: Vec<bool> = messages.iter().map(|message| message.show_header).collect();
        assert_eq!(headers, [true, false, true, true, true, true, true, true]);
        assert_eq!(messages[0].day_label.as_str(), "24 de setembro de 2026");
        assert_eq!(messages[1].day_label.as_str(), "");
        assert_eq!(messages[5].day_label.as_str(), "25 de setembro de 2026");
        assert_eq!(messages[7].day_label.as_str(), "");
    }
}
