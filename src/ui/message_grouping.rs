use chrono::{DateTime, Datelike, Local, NaiveDate};
use serde_json::Value;

use crate::utils::i18n::Language;
use crate::ChatMessage;

const GROUP_WINDOW_SECONDS: i64 = 5 * 60;

/// Rows kept in the chat model while the reader is at the bottom.
///
/// The chat view is not virtualized yet, so every row stays instantiated. Live
/// appends used to grow it without limit for the whole visit; keeping a window
/// bounds the model while the rows that leave are far above the viewport.
pub const MAX_CHAT_ROWS: usize = 300;

/// Hard cap while the user explicitly walks back through the history.
///
/// Live appends keep the smaller [`MAX_CHAT_ROWS`] window. "Load older" may
/// grow the model up to this limit; after that the button is hidden so browsing
/// history cannot grow the scene without bound. Returning to the present is
/// unaffected because only live appends trim from the oldest end.
pub const MAX_CHAT_ROWS_WITH_HISTORY: usize = 600;

/// Oldest row after a trim, so the pagination cursor can move back to it.
pub struct TrimmedWindow {
    pub oldest_id: Option<String>,
}

/// Drops the oldest rows when `model` grew past `max_rows`.
///
/// Returns `Some` only when rows were actually removed. The caller must move the
/// `oldest_message_map` cursor to `oldest_id` (when present) and regroup row 0,
/// otherwise "load older" would skip the trimmed window.
pub fn trim_chat_window(
    model: &slint::VecModel<ChatMessage>,
    max_rows: usize,
) -> Option<TrimmedWindow> {
    use slint::Model;

    let row_count = model.row_count();
    if row_count <= max_rows {
        return None;
    }
    for _ in 0..(row_count - max_rows) {
        model.remove(0);
    }
    let oldest_id = (0..model.row_count()).find_map(|index| {
        let row = model.row_data(index)?;
        (!row.id.is_empty()).then(|| row.id.to_string())
    });
    Some(TrimmedWindow { oldest_id })
}

/// Trims the window and keeps pagination coherent with what is left.
///
/// Moves the "oldest message" cursor to the new first row and re-groups it, so
/// "load older" refetches the dropped window instead of skipping it. Returns
/// `true` when rows were removed.
pub fn trim_chat_window_and_rebase(
    messages: &slint::ModelRc<ChatMessage>,
    model: &slint::VecModel<ChatMessage>,
    channel_id: &str,
) -> bool {
    let Some(trimmed) = trim_chat_window(model, MAX_CHAT_ROWS) else {
        return false;
    };
    if let Some(oldest_id) = trimmed.oldest_id {
        if let Ok(mut map) = crate::ui::data_loader::get_oldest_message_map().lock() {
            map.insert(channel_id.to_string(), oldest_id);
        }
    }
    regroup_message_at(messages, 0);
    true
}

/// Keep the Discord metadata on each independent message, including messages
/// loaded from history and messages received through the gateway.
pub fn apply_message_metadata(message: &mut ChatMessage, source: &Value) {
    let author_id = source["author"]["id"]
        .as_str()
        .or_else(|| source["webhook_id"].as_str())
        .unwrap_or("");
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

/// Group DM icon, built only from a numeric channel id and a validated hash.
pub fn group_icon_url(channel_id: &str, hash: &str) -> String {
    if channel_id.is_empty()
        || !channel_id.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_avatar_hash(hash)
    {
        return String::new();
    }
    format!("https://cdn.discordapp.com/channel-icons/{channel_id}/{hash}.webp?size=64")
}

pub(crate) fn valid_avatar_hash(hash: &str) -> bool {
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
        let (next_previous, next_day) = group_row(message, previous.as_ref(), previous_day);
        previous = next_previous;
        previous_day = next_day;
    }
}

type PreviousMessage = (String, String, DateTime<Local>);

/// Applies the grouping flags of one row from the state left by the rows above it.
///
/// Returns the state the next row must start from: `previous` only carries a
/// regular message (a system row resets it) while `previous_day` survives system
/// rows so a day separator is not repeated around them.
fn group_row(
    message: &mut ChatMessage,
    previous: Option<&PreviousMessage>,
    previous_day: Option<NaiveDate>,
) -> (Option<PreviousMessage>, Option<NaiveDate>) {
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
        label.into()
    } else {
        "".into()
    };

    message.show_header = !is_regular_message
        || !previous.is_some_and(|(author_id, author, previous_time)| {
            let time = sent_at.as_ref().expect("regular messages have a timestamp");
            author_id == message.author_id.as_str()
                && author == message.author.as_str()
                && previous_time.date_naive() == time.date_naive()
                && (0..=GROUP_WINDOW_SECONDS)
                    .contains(&time.signed_duration_since(*previous_time).num_seconds())
        });

    if is_regular_message {
        let time = sent_at.expect("regular messages have a timestamp");
        (
            Some((
                message.author_id.to_string(),
                message.author.to_string(),
                time,
            )),
            Some(time.date_naive()),
        )
    } else {
        (None, previous_day)
    }
}

/// Reads the grouping state produced by the rows above `index`.
fn context_before(
    messages: &slint::ModelRc<ChatMessage>,
    index: usize,
) -> (Option<PreviousMessage>, Option<NaiveDate>) {
    use slint::Model;

    let previous = index
        .checked_sub(1)
        .and_then(|previous_index| messages.row_data(previous_index))
        .and_then(regular_message_info);

    // `previous_day` survives non-regular rows, so scan back to the last regular
    // message instead of stopping at the first system row.
    let mut previous_day = None;
    for row_index in (0..index).rev() {
        let Some(info) = messages.row_data(row_index).and_then(regular_message_info) else {
            continue;
        };
        previous_day = Some(info.2.date_naive());
        break;
    }

    (previous, previous_day)
}

fn regular_message_info(message: ChatMessage) -> Option<PreviousMessage> {
    if message.id.is_empty() || message.author_id.is_empty() {
        return None;
    }
    let sent_at = parse_local_time(message.created_at.as_str())?;
    Some((
        message.author_id.to_string(),
        message.author.to_string(),
        sent_at,
    ))
}

/// Re-groups the row at `index` and the one right after it.
///
/// Used after an edit or a deletion (where the row that moved into `index` now
/// has a new predecessor). Rows above keep their grouping, so a delete costs two
/// row updates instead of rebuilding the whole chat model.
pub fn regroup_message_at(messages: &slint::ModelRc<ChatMessage>, index: usize) {
    use slint::Model;

    let row_count = messages.row_count();
    if index >= row_count {
        return;
    }

    let (mut previous, mut previous_day) = context_before(messages, index);
    let last = (index + 1).min(row_count - 1);
    for row_index in index..=last {
        let Some(mut message) = messages.row_data(row_index) else { break };
        let (next_previous, next_day) = group_row(&mut message, previous.as_ref(), previous_day);
        messages.set_row_data(row_index, message);
        previous = next_previous;
        previous_day = next_day;
    }
}

/// Groups only the message being appended, using the last regular row already
/// on screen as context. Equivalent to the last iteration of `regroup_messages`
/// but without re-reading the whole model, so a live message costs O(1) even
/// with thousands of rows loaded.
pub fn group_appended_message(messages: &slint::ModelRc<ChatMessage>, message: &mut ChatMessage) {
    use slint::Model;

    let (previous, previous_day) = context_before(messages, messages.row_count());
    let _ = group_row(message, previous.as_ref(), previous_day);
}

/// Rewrites the day separators already on screen in the current language.
/// Only rows that carry a separator are touched, so a language switch costs one
/// row update per visible day instead of rebuilding the message list.
pub fn relabel_day_separators(messages: &slint::ModelRc<ChatMessage>) {
    relabel_day_separators_for(crate::utils::i18n_ui::current_language(), messages);
}

fn relabel_day_separators_for(lang: Language, messages: &slint::ModelRc<ChatMessage>) {
    use slint::Model;
    for index in 0..messages.row_count() {
        let Some(mut message) = messages.row_data(index) else { continue };
        if message.day_label.is_empty() {
            continue;
        }
        let Some(sent_at) = parse_local_time(message.created_at.as_str()) else { continue };
        let label = format_day_for(lang, sent_at.date_naive());
        if message.day_label.as_str() != label {
            message.day_label = label.into();
            messages.set_row_data(index, message);
        }
    }
}

fn format_day(day: NaiveDate) -> String {
    format_day_for(crate::utils::i18n_ui::current_language(), day)
}

/// Long date used by the day separators, written the way each language writes it.
fn format_day_for(lang: Language, day: NaiveDate) -> String {
    let m = day.month0() as usize;
    let (d, y) = (day.day(), day.year());
    match lang {
        Language::Portuguese => {
            const MONTHS: [&str; 12] = [
                "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto",
                "setembro", "outubro", "novembro", "dezembro",
            ];
            format!("{d} de {} de {y}", MONTHS[m])
        }
        Language::Spanish => {
            const MONTHS: [&str; 12] = [
                "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto",
                "septiembre", "octubre", "noviembre", "diciembre",
            ];
            format!("{d} de {} de {y}", MONTHS[m])
        }
        Language::German => {
            const MONTHS: [&str; 12] = [
                "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August",
                "September", "Oktober", "November", "Dezember",
            ];
            format!("{d}. {} {y}", MONTHS[m])
        }
        Language::French => {
            const MONTHS: [&str; 12] = [
                "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août",
                "septembre", "octobre", "novembre", "décembre",
            ];
            if d == 1 {
                format!("1er {} {y}", MONTHS[m])
            } else {
                format!("{d} {} {y}", MONTHS[m])
            }
        }
        Language::Russian => {
            const MONTHS: [&str; 12] = [
                "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа",
                "сентября", "октября", "ноября", "декабря",
            ];
            format!("{d} {} {y} г.", MONTHS[m])
        }
        Language::Japanese => format!("{y}年{}月{d}日", m + 1),
        Language::English | Language::Auto => {
            const MONTHS: [&str; 12] = [
                "January", "February", "March", "April", "May", "June", "July", "August",
                "September", "October", "November", "December",
            ];
            format!("{} {d}, {y}", MONTHS[m])
        }
    }
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
        // The text depends on the UI language (shared global); only the layout is checked here.
        assert!(!messages[0].day_label.is_empty());
        assert_eq!(messages[1].day_label.as_str(), "");
        assert!(!messages[5].day_label.is_empty());
        assert_eq!(messages[7].day_label.as_str(), "");
    }

    #[test]
    fn day_labels_follow_each_language() {
        let day = NaiveDate::from_ymd_opt(2026, 9, 24).unwrap();
        assert_eq!(format_day_for(Language::Portuguese, day), "24 de setembro de 2026");
        assert_eq!(format_day_for(Language::English, day), "September 24, 2026");
        assert_eq!(format_day_for(Language::Spanish, day), "24 de septiembre de 2026");
        assert_eq!(format_day_for(Language::German, day), "24. September 2026");
        assert_eq!(format_day_for(Language::French, day), "24 septembre 2026");
        assert_eq!(format_day_for(Language::Russian, day), "24 сентября 2026 г.");
        assert_eq!(format_day_for(Language::Japanese, day), "2026年9月24日");
    }

    #[test]
    fn language_switch_rewrites_only_the_separators_on_screen() {
        use slint::Model;
        let mut first = ChatMessage::default();
        set_message_metadata(&mut first, "1", "2026-09-24T12:00:00+00:00");
        first.id = "10".into();
        first.day_label = "24 de setembro de 2026".into();
        let mut grouped = ChatMessage::default();
        set_message_metadata(&mut grouped, "1", "2026-09-24T12:01:00+00:00");
        grouped.id = "11".into();

        let model = slint::ModelRc::new(slint::VecModel::from(vec![first, grouped]));
        relabel_day_separators_for(Language::English, &model);

        let day = parse_local_time("2026-09-24T12:00:00+00:00").unwrap().date_naive();
        assert_eq!(model.row_data(0).unwrap().day_label.as_str(), format_day_for(Language::English, day));
        assert_eq!(model.row_data(1).unwrap().day_label.as_str(), "");
    }

    #[test]
    fn incremental_append_matches_full_regroup() {
        fn message(id: &str, author_id: &str, time: &str) -> ChatMessage {
            ChatMessage {
                id: id.into(),
                author: author_id.into(),
                author_id: author_id.into(),
                created_at: time.into(),
                ..Default::default()
            }
        }

        let scenarios: Vec<(Vec<ChatMessage>, ChatMessage)> = vec![
            // Same author inside the five minute grouping window.
            (
                vec![message("1", "1", "2026-09-24T12:00:00Z")],
                message("2", "1", "2026-09-24T12:04:00Z"),
            ),
            // Same author after the window.
            (
                vec![message("1", "1", "2026-09-24T12:00:00Z")],
                message("2", "1", "2026-09-24T12:06:00Z"),
            ),
            // Different author.
            (
                vec![message("1", "1", "2026-09-24T12:00:00Z")],
                message("2", "2", "2026-09-24T12:01:00Z"),
            ),
            // Day boundary.
            (
                vec![message("1", "1", "2026-09-24T12:00:00Z")],
                message("2", "1", "2026-09-25T12:00:00Z"),
            ),
            // A system row between the regular messages breaks header grouping but
            // still suppresses a duplicate day separator.
            (
                vec![
                    message("1", "1", "2026-09-24T12:00:00Z"),
                    ChatMessage::default(),
                ],
                message("2", "1", "2026-09-24T12:01:00Z"),
            ),
            // Empty model: the first row carries a day label and a header.
            (
                Vec::new(),
                message("1", "1", "2026-09-24T12:00:00Z"),
            ),
            // Appended system row.
            (
                vec![message("1", "1", "2026-09-24T12:00:00Z")],
                ChatMessage::default(),
            ),
        ];

        for (previous, appended) in scenarios {
            let mut expected_rows = previous.clone();
            expected_rows.push(appended.clone());
            regroup_messages(&mut expected_rows);
            let expected = expected_rows.pop().unwrap();

            let model = slint::ModelRc::new(slint::VecModel::from(previous));
            let mut actual = appended;
            group_appended_message(&model, &mut actual);

            assert_eq!(actual.show_header, expected.show_header);
            assert_eq!(actual.day_label.as_str(), expected.day_label.as_str());
        }
    }

    #[test]
    fn chat_window_drops_the_oldest_rows_and_reports_the_new_cursor() {
        use slint::Model;

        fn message(id: &str) -> ChatMessage {
            ChatMessage {
                id: id.into(),
                author_id: "1".into(),
                author: "Ana".into(),
                created_at: "2026-09-24T12:00:00Z".into(),
                ..Default::default()
            }
        }

        let model = slint::VecModel::from((0..10).map(|index| message(&index.to_string())).collect::<Vec<_>>());
        assert!(trim_chat_window(&model, 10).is_none(), "at the cap nothing moves");

        let trimmed = trim_chat_window(&model, 6).expect("five rows are dropped");
        assert_eq!(model.row_count(), 6);
        assert_eq!(trimmed.oldest_id.as_deref(), Some("4"));
        assert_eq!(model.row_data(0).unwrap().id.as_str(), "4");
    }

    #[test]
    fn incremental_delete_matches_full_regroup() {
        use slint::Model;

        fn message(id: &str, author_id: &str, time: &str) -> ChatMessage {
            ChatMessage {
                id: id.into(),
                author: author_id.into(),
                author_id: author_id.into(),
                created_at: time.into(),
                ..Default::default()
            }
        }

        // Same-author runs, an author switch, a day boundary and a system row in
        // the middle so every index has an interesting predecessor.
        let mut rows = vec![
            message("1", "1", "2026-09-24T12:00:00Z"),
            message("2", "1", "2026-09-24T12:03:00Z"),
            message("3", "2", "2026-09-24T12:04:00Z"),
            ChatMessage::default(),
            message("4", "2", "2026-09-25T12:00:00Z"),
            message("5", "2", "2026-09-25T12:02:00Z"),
        ];
        regroup_messages(&mut rows);

        for removed in 0..rows.len() {
            let mut expected: Vec<ChatMessage> = rows
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != removed)
                .map(|(_, row)| row.clone())
                .collect();
            regroup_messages(&mut expected);

            let vec_model = std::rc::Rc::new(slint::VecModel::from(rows.clone()));
            let model: slint::ModelRc<ChatMessage> = vec_model.clone().into();
            vec_model.remove(removed);
            regroup_message_at(&model, removed);

            assert_eq!(model.row_count(), expected.len());
            for (index, expected_row) in expected.iter().enumerate() {
                let actual = model.row_data(index).unwrap();
                assert_eq!(
                    actual.show_header, expected_row.show_header,
                    "removed {removed}, row {index}"
                );
                assert_eq!(
                    actual.day_label.as_str(),
                    expected_row.day_label.as_str(),
                    "removed {removed}, row {index}"
                );
            }
        }
    }

    #[test]
    fn incremental_regroup_after_content_change_matches_full_regroup() {
        use slint::Model;

        let mut rows = vec![
            ChatMessage {
                id: "1".into(),
                author: "Ana".into(),
                author_id: "1".into(),
                created_at: "2026-09-24T12:00:00Z".into(),
                ..Default::default()
            },
            ChatMessage {
                id: "2".into(),
                author: "Ana".into(),
                author_id: "1".into(),
                created_at: "2026-09-24T12:02:00Z".into(),
                ..Default::default()
            },
            ChatMessage {
                id: "3".into(),
                author: "Bia".into(),
                author_id: "2".into(),
                created_at: "2026-09-24T12:03:00Z".into(),
                ..Default::default()
            },
            ChatMessage {
                id: "4".into(),
                author: "Bia".into(),
                author_id: "2".into(),
                created_at: "2026-09-24T12:04:00Z".into(),
                ..Default::default()
            },
        ];
        regroup_messages(&mut rows);

        let model = slint::ModelRc::new(slint::VecModel::from(rows.clone()));
        // Simulate an edit that moves the second row inside its own five minute
        // window: only that row and the next one need to be re-evaluated.
        let mut edited = model.row_data(1).unwrap();
        edited.created_at = "2026-09-24T12:06:00Z".into();
        model.set_row_data(1, edited);
        regroup_message_at(&model, 1);

        let mut expected = rows;
        expected[1].created_at = "2026-09-24T12:06:00Z".into();
        regroup_messages(&mut expected);

        for index in 0..expected.len() {
            let actual = model.row_data(index).unwrap();
            assert_eq!(actual.show_header, expected[index].show_header, "row {index}");
            assert_eq!(
                actual.day_label.as_str(),
                expected[index].day_label.as_str(),
                "row {index}"
            );
        }
    }

    #[test]
    fn group_icon_urls_require_numeric_ids_and_hex_hashes() {
        assert_eq!(
            group_icon_url("123", "a_0f1e"),
            "https://cdn.discordapp.com/channel-icons/123/a_0f1e.webp?size=64"
        );
        assert!(group_icon_url("12/../3", "abcd").is_empty());
        assert!(group_icon_url("123", "../evil").is_empty());
        assert!(group_icon_url("123", "").is_empty());
    }
}
