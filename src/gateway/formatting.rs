#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use super::types::*;

pub fn clean_discord_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            // Em space, En space, Non-breaking space, tabs, ideographic space -> single regular space
            '\u{00A0}' | '\u{2000}' | '\u{2001}' | '\u{2002}' | '\u{2003}' |
            '\u{2004}' | '\u{2005}' | '\u{2006}' | '\u{2007}' | '\u{2008}' |
            '\u{2009}' | '\u{200A}' | '\u{3000}' | '\t' => {
                out.push(' ');
            }
            // Zero-width spaces, soft hyphens -> strip
            '\u{200B}' | '\u{FEFF}' | '\u{200C}' | '\u{200D}' | '\u{200E}' | '\u{200F}' | '\u{00AD}' => {
                // skip
            }
            _ => {
                out.push(c);
            }
        }
    }
    out
}

/// Converts raw Discord Markdown syntax into clean readable plain text.
/// Applied in the correct order so that code blocks are parsed first (immune
/// to inner formatting) and inline formatting runs last.
pub fn parse_discord_markdown(input: &str) -> String {
    let cleaned_input = clean_discord_whitespace(input);
    let mut output_lines: Vec<String> = Vec::new();

    // ── Step 1: Protect code blocks (```) from inner parsing ──────────────
    // Split by triple-backtick blocks, mark odd-indexed segments as code.
    let triple_parts: Vec<&str> = cleaned_input.split("```").collect();
    let mut lines_to_process: Vec<(String, bool)> = Vec::new(); // (text, is_code_block)

    for (idx, part) in triple_parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside a code block
            // First "word" of the block may be a language hint (e.g. "python\ncode")
            let trimmed = part.trim_start_matches('\n');
            let (lang, code_body) = if let Some(nl) = trimmed.find('\n') {
                let potential_lang = &trimmed[..nl];
                // Language hints are short alphanumeric words
                if potential_lang.len() <= 20 && potential_lang.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '-' || c == '_') {
                    (potential_lang, &trimmed[nl+1..])
                } else {
                    ("", trimmed)
                }
            } else {
                ("", trimmed)
            };
            if lang.is_empty() {
                lines_to_process.push((format!("[Bloco de Código]\n{}", code_body.trim_end()), true));
            } else {
                lines_to_process.push((format!("[Bloco de Código ({})]\n{}", lang, code_body.trim_end()), true));
            }
        } else {
            for line in part.split('\n') {
                lines_to_process.push((line.to_string(), false));
            }
        }
    }

    // ── Step 2: Process each line ──────────────────────────────────────────
    for (text, is_code) in lines_to_process {
        if is_code {
            output_lines.push(text);
            continue;
        }

        let raw_line = text.as_str();
        let line = raw_line.trim_start();
        if line.is_empty() {
            output_lines.push(String::new());
            continue;
        }

        // Block-level: Headers (must be at start of line)
        if let Some(rest) = line.strip_prefix("### ") {
            output_lines.push(format!("▪ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            output_lines.push(format!("▌ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            output_lines.push(format!("▌ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Subtext
        if let Some(rest) = line.strip_prefix("-# ") {
            output_lines.push(apply_inline_markdown(rest.trim_start()));
            continue;
        }
        // Blockquote multi-line >>>
        if let Some(rest) = line.strip_prefix(">>> ") {
            for bq_line in rest.lines() {
                output_lines.push(format!("│ {}", apply_inline_markdown(bq_line.trim_start())));
            }
            continue;
        }
        // Blockquote single line >
        if let Some(rest) = line.strip_prefix("> ") {
            output_lines.push(format!("│ {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Unordered list (- or * at start)
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            output_lines.push(format!("• {}", apply_inline_markdown(rest.trim_start())));
            continue;
        }
        // Ordered list (number followed by ". ")
        {
            let mut chars = line.chars().peekable();
            let mut digits = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() { digits.push(c); chars.next(); } else { break; }
            }
            if !digits.is_empty() && line[digits.len()..].starts_with(". ") {
                let rest = &line[digits.len() + 2..];
                output_lines.push(format!("{}. {}", digits, apply_inline_markdown(rest.trim_start())));
                continue;
            }
        }

        // Normal line — apply inline markdown
        output_lines.push(apply_inline_markdown(line));
    }

    output_lines.join("\n")
}

/// Applies all inline Discord Markdown transforms to a single line of text.
/// Code blocks have already been extracted, so this only handles inline syntax.
fn apply_inline_markdown(input: &str) -> String {
    let s = input.to_string();

    // ── Inline code (backtick) — protect from further parsing ─────────────
    // We replace inline code with a placeholder, process the rest, then restore.
    let backtick_re_parts: Vec<&str> = s.split('`').collect();
    let mut reconstructed = String::new();
    let mut code_segments: Vec<String> = Vec::new();

    for (idx, part) in backtick_re_parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside inline code — protect it
            let placeholder = format!("\x00CODE{}\x00", code_segments.len());
            code_segments.push(format!("`{}`", part));
            reconstructed.push_str(&placeholder);
        } else {
            reconstructed.push_str(part);
        }
    }

    let mut s = reconstructed;

    // ── Markdown links [text](url) ─────────────────────────────────────────
    // Parse [label](url) → "label (url)"
    {
        let mut out = String::with_capacity(s.len());
        let mut rem = s.as_str();
        while let Some(bracket_start) = rem.find('[') {
            // Check it's not an escaped bracket
            out.push_str(&rem[..bracket_start]);
            let after_bracket = &rem[bracket_start + 1..];
            if let Some(bracket_end) = after_bracket.find(']') {
                let label = &after_bracket[..bracket_end];
                let after_label = &after_bracket[bracket_end + 1..];
                if after_label.starts_with('(') {
                    if let Some(paren_end) = after_label.find(')') {
                        let url = &after_label[1..paren_end];
                        if url.starts_with("http") {
                            if label.is_empty() {
                                out.push_str(url);
                            } else {
                                out.push_str(label);
                            }
                            rem = &after_label[paren_end + 1..];
                            continue;
                        }
                    }
                }
                // Not a valid link — emit literally
                out.push('[');
            } else {
                out.push('[');
            }
            rem = &rem[bracket_start + 1..];
        }
        out.push_str(rem);
        s = out;
    }

    // ── Spoilers ||text|| ──────────────────────────────────────────────────
    s = replace_between(&s, "||", "||", |_| "[SPOILER]".to_string());

    // ── Mentions and Discord entities ──────────────────────────────────────
    // <t:timestamp:format> — Discord timestamps
    s = regex_replace_simple(&s, "<t:", ">", |inner| {
        let parts: Vec<&str> = inner.splitn(2, ':').collect();
        let ts: i64 = parts[0].parse().unwrap_or(0);
        format!("[{}]", format_unix_timestamp(ts))
    });

    // <@!USER_ID> and <@USER_ID> — user mentions
    s = regex_replace_simple(&s, "<@!", ">", |_inner| "@usuário".to_string());
    s = regex_replace_simple(&s, "<@&", ">", |_inner| "@cargo".to_string());
    s = regex_replace_simple(&s, "<@", ">", |inner| {
        // Only if purely numeric (user ID)
        if inner.chars().all(|c| c.is_ascii_digit()) {
            "@usuário".to_string()
        } else {
            format!("<@{}>", inner)
        }
    });
    // <#CHANNEL_ID> — channel mentions
    s = regex_replace_simple(&s, "<#", ">", |_inner| "#canal".to_string());

    // <:name:ID> — custom emoji
    s = regex_replace_simple(&s, "<a:", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("emoji");
        format!(":{}:", name)
    });
    s = regex_replace_simple(&s, "<:", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("emoji");
        format!(":{}:", name)
    });

    // </name:ID> — command mentions
    s = regex_replace_simple(&s, "</", ">", |inner| {
        let name = inner.split(':').next().unwrap_or("comando");
        format!("/{}", name)
    });

    // ── Text formatting (order matters: most-specific first) ───────────────
    // Bold + Italic ***
    s = replace_between(&s, "***", "***", |inner| inner.to_string());
    // Bold **
    s = replace_between(&s, "**", "**", |inner| inner.to_string());
    // Italic * (single, but not at word boundaries where it would be a list)
    s = replace_between(&s, "*", "*", |inner| inner.to_string());
    // Underline __
    s = replace_between(&s, "__", "__", |inner| inner.to_string());
    // Italic _
    s = replace_between(&s, "_", "_", |inner| inner.to_string());
    // Strikethrough ~~
    s = replace_between(&s, "~~", "~~", |inner| inner.to_string());

    // ── Backslash escape removal ───────────────────────────────────────────
    let mut unescaped = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if matches!(next, '*' | '_' | '~' | '`' | '|' | '>' | '#' | '-' | '\\') {
                    unescaped.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        unescaped.push(c);
    }
    s = unescaped;

    // ── HTML entities ──────────────────────────────────────────────────────
    s = s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");

    // ── Restore inline code segments ──────────────────────────────────────
    for (idx, code) in code_segments.iter().enumerate() {
        let placeholder = format!("\x00CODE{}\x00", idx);
        s = s.replace(&placeholder, code);
    }

    s
}

/// Replaces all occurrences of text between `open` and `close` delimiters,
/// passing the inner content to `replacer`. Non-greedy (finds first closing).
fn replace_between<F>(input: &str, open: &str, close: &str, replacer: F) -> String
where
    F: Fn(&str) -> String,
{
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(start) = remaining.find(open) {
        let after_open = &remaining[start + open.len()..];
        if let Some(end) = after_open.find(close) {
            let inner = &after_open[..end];
            // Only replace if inner is non-empty
            if !inner.is_empty() {
                result.push_str(&remaining[..start]);
                result.push_str(&replacer(inner));
                remaining = &after_open[end + close.len()..];
                continue;
            }
        }
        // No matching close found — emit the open delimiter literally
        result.push_str(&remaining[..start + open.len()]);
        remaining = &remaining[start + open.len()..];
    }
    result.push_str(remaining);
    result
}

/// Replaces `prefix...suffix` patterns (single-pass, non-overlapping).
fn regex_replace_simple<F>(input: &str, prefix: &str, suffix: &str, replacer: F) -> String
where
    F: Fn(&str) -> String,
{
    replace_between(input, prefix, suffix, replacer)
}

/// Formats a Unix timestamp (seconds since epoch) into a human-readable string.
fn format_unix_timestamp(unix_secs: i64) -> String {
    // Simple calculation: days since 1970-01-01
    if unix_secs <= 0 {
        return "data desconhecida".to_string();
    }
    let secs_per_day = 86400i64;
    let total_days = unix_secs / secs_per_day;
    let time_secs = unix_secs % secs_per_day;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;

    // Compute year/month/day from days since epoch (Gregorian)
    let days_remaining = total_days + 719468; // offset to March 1, year 0
    let era = if days_remaining >= 0 { days_remaining } else { days_remaining - 146096 } / 146097;
    let doe = days_remaining - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:02}/{:02}/{} {:02}:{:02}", d, m, y, hours, minutes)
}

pub fn format_discord_author(m: &Value) -> String {
    let author_obj = &m["author"];
    let name = author_obj["global_name"].as_str()
        .unwrap_or_else(|| author_obj["username"].as_str().unwrap_or("Unknown"));
    let is_bot = author_obj["bot"].as_bool().unwrap_or(false);

    if is_bot {
        format!("{} [BOT]", name)
    } else {
        name.to_string()
    }
}

pub fn is_emoji_char(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1F600..=0x1F64F | // Emoticons
        0x1F300..=0x1F5FF | // Misc Symbols and Pictographs
        0x1F680..=0x1F6FF | // Transport and Map
        0x1F1E6..=0x1F1FF | // Flags
        0x2600..=0x26FF   | // Misc symbols (e.g. ⚡, ⚠️, ⚽, ☕)
        0x2700..=0x27BF   | // Dingbats (e.g. ✨, ❌, ❓, ❗)
        0x1F900..=0x1F9FF | // Supplemental Symbols
        0x1FA70..=0x1FAFF | // Symbols Extended-A
        0x2300..=0x23FF   | // Media & Tech (⏮, ⏭, ⏯, ⏸, ⏹, ⏺, ⏩, ⏪, ⏫, ⏬, ⏰, ⏳)
        0x25A0..=0x25FF   | // Geometric Shapes (▶, ◀, ◼, ◻)
        0x2B00..=0x2BFF   | // Misc Symbols and Arrows (⬅, ⬆, ⬇, ⭐, ⭕)
        0x2934..=0x2935   | // Arrows
        0x3030 | 0x303D | 0x3297 | 0x3299 | 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x2199 | 0x21A9..=0x21AA
    )
}

pub fn find_next_unicode_emoji(s: &str) -> Option<(usize, usize, String, String)> {
    let mut char_indices = s.char_indices().peekable();
    while let Some((idx, c)) = char_indices.next() {
        if is_emoji_char(c) {
            let mut end_idx = idx + c.len_utf8();
            let mut emoji_chars = vec![c];

            while let Some(&(next_idx, next_c)) = char_indices.peek() {
                if next_c == '\u{fe0f}' || next_c == '\u{fe0e}' || next_c == '\u{200d}' || (0x1f3fb..=0x1f3ff).contains(&(next_c as u32)) || (is_emoji_char(next_c) && emoji_chars.last() == Some(&'\u{200d}')) {
                    char_indices.next();
                    end_idx = next_idx + next_c.len_utf8();
                    emoji_chars.push(next_c);
                } else {
                    break;
                }
            }

            let hex_parts: Vec<String> = emoji_chars.iter()
                .filter(|&&ch| ch != '\u{fe0f}' && ch != '\u{fe0e}')
                .map(|&ch| format!("{:x}", ch as u32))
                .collect();
            let twemoji_hex = hex_parts.join("-");
            let emoji_text = s[idx..end_idx].to_string();
            return Some((idx, end_idx - idx, emoji_text, format!("u_{}", twemoji_hex)));
        }
    }
    None
}

fn push_text_before_cmd(before: &str, line_blocks: &mut Vec<MessageBlockData>, lines: &mut Vec<MessageLineData>) {
    let trimmed = before.trim();
    if trimmed.is_empty() {
        return;
    }
    
    // If before is long (> 40 chars), find the best natural breaking point (punctuation or space)
    // so the text immediately preceding the command chip is short (< 35 chars) and stays on the same line with the chip!
    if trimmed.chars().count() > 40 {
        let chars: Vec<char> = trimmed.chars().collect();
        let total = chars.len();
        
        let mut split_idx = None;
        for i in (15..total.saturating_sub(12)).rev() {
            let c = chars[i];
            if c == '.' || c == '!' || c == '?' || c == ':' || c == ';' || c == ',' {
                split_idx = Some(i + 1);
                break;
            }
        }
        if split_idx.is_none() {
            for i in (15..total.saturating_sub(12)).rev() {
                if chars[i] == ' ' {
                    split_idx = Some(i);
                    break;
                }
            }
        }

        if let Some(idx) = split_idx {
            let p1: String = chars[..idx].iter().collect();
            let p2: String = chars[idx..].iter().collect();
            if !p1.trim().is_empty() {
                line_blocks.push(MessageBlockData {
                    text: parse_discord_markdown(p1.trim()),
                    is_link: false,
                    is_command: false,
                    is_emoji: false,
                    emoji_id: String::new(),
                    url: String::new(),
                    command_name: String::new(),
                });
                lines.push(MessageLineData { blocks: std::mem::take(line_blocks) });
            }
            let p2_clean = p2.trim_start();
            if !p2_clean.is_empty() {
                line_blocks.push(MessageBlockData {
                    text: parse_discord_markdown(p2_clean),
                    is_link: false,
                    is_command: false,
                    is_emoji: false,
                    emoji_id: String::new(),
                    url: String::new(),
                    command_name: String::new(),
                });
            }
            return;
        }
    }

    line_blocks.push(MessageBlockData {
        text: parse_discord_markdown(before),
        is_link: false,
        is_command: false,
        is_emoji: false,
        emoji_id: String::new(),
        url: String::new(),
        command_name: String::new(),
    });
}

pub fn parse_text_into_lines(input: &str, links: &mut Vec<LinkData>) -> Vec<MessageLineData> {
    let mut lines = Vec::new();
    if input.is_empty() {
        return lines;
    }

    let cleaned = clean_discord_whitespace(input);

    for raw_line in cleaned.split('\n') {
        let trimmed_line = raw_line.trim_start();
        if trimmed_line.is_empty() {
            continue;
        }
        let mut line_blocks = Vec::new();
        let mut rem = trimmed_line;

        while !rem.is_empty() {
            let next_bracket = rem.find('[');
            let next_cmd = rem.find("</");
            let next_emoji = match (rem.find("<:"), rem.find("<a:")) {
                (Some(s), Some(a)) => Some((s.min(a), if s < a { "<:" } else { "<a:" })),
                (Some(s), None) => Some((s, "<:")),
                (None, Some(a)) => Some((a, "<a:")),
                (None, None) => None,
            };
            let next_http = rem.find("http://");
            let next_https = rem.find("https://");

            let next_raw_url = match (next_http, next_https) {
                (Some(h), Some(hs)) => Some(h.min(hs)),
                (Some(h), None) => Some(h),
                (None, Some(hs)) => Some(hs),
                (None, None) => None,
            };
            let next_unicode = find_next_unicode_emoji(rem);

            let mut candidates: Vec<(usize, &str)> = Vec::new();
            if let Some(idx) = next_bracket { candidates.push((idx, "bracket")); }
            if let Some(idx) = next_cmd { candidates.push((idx, "cmd")); }
            if let Some((idx, _)) = next_emoji { candidates.push((idx, "emoji")); }
            if let Some(idx) = next_raw_url { candidates.push((idx, "raw_url")); }
            if let Some((idx, _, _, _)) = next_unicode { candidates.push((idx, "unicode_emoji")); }

            if candidates.is_empty() {
                let parsed = parse_discord_markdown(rem);
                if !parsed.is_empty() {
                    line_blocks.push(MessageBlockData {
                        text: parsed,
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                }
                break;
            }

            candidates.sort_by_key(|c| c.0);
            let (first_idx, first_type) = candidates[0];

            match first_type {
                "bracket" => {
                    let before = &rem[..first_idx];
                    let after_bracket = &rem[first_idx + 1..];
                    if let Some(bracket_end) = after_bracket.find(']') {
                        let label = &after_bracket[..bracket_end];
                        let after_label = &after_bracket[bracket_end + 1..];
                        if after_label.starts_with('(') {
                            if let Some(paren_end) = after_label.find(')') {
                                let url = &after_label[1..paren_end];
                                if url.starts_with("http") {
                                    if !before.is_empty() {
                                        line_blocks.push(MessageBlockData {
                                            text: parse_discord_markdown(before),
                                            is_link: false,
                                            is_command: false,
                                            is_emoji: false,
                                            emoji_id: String::new(),
                                            url: String::new(),
                                            command_name: String::new(),
                                        });
                                    }
                                    let display_label = if label.is_empty() { url.to_string() } else { parse_discord_markdown(label) };
                                    line_blocks.push(MessageBlockData {
                                        text: display_label.clone(),
                                        is_link: true,
                                        is_command: false,
                                        is_emoji: false,
                                        emoji_id: String::new(),
                                        url: url.to_string(),
                                        command_name: String::new(),
                                    });
                                    if !links.iter().any(|l| l.url == url) {
                                        links.push(LinkData {
                                            label: display_label,
                                            url: url.to_string(),
                                        });
                                    }
                                    rem = &after_label[paren_end + 1..];
                                    continue;
                                }
                            }
                        }
                    }
                    let slice_len = first_idx + 1;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "cmd" => {
                    let before = &rem[..first_idx];
                    let after_tag = &rem[first_idx + 2..];
                    if let Some(gt_idx) = after_tag.find('>') {
                        let inside = &after_tag[..gt_idx];
                        let (cmd_name, _cmd_id) = if let Some(colon) = inside.find(':') {
                            (&inside[..colon], &inside[colon + 1..])
                        } else {
                            (inside, "")
                        };
                        let clean_cmd = cmd_name.trim().trim_start_matches('/');
                        if !clean_cmd.is_empty() {
                            if !before.is_empty() {
                                push_text_before_cmd(before, &mut line_blocks, &mut lines);
                            }
                            line_blocks.push(MessageBlockData {
                                text: format!("/{}", clean_cmd),
                                is_link: false,
                                is_command: true,
                                is_emoji: false,
                                emoji_id: String::new(),
                                url: String::new(),
                                command_name: format!("/{}", clean_cmd),
                            });
                            rem = &after_tag[gt_idx + 1..];
                            continue;
                        }
                    }
                    let slice_len = first_idx + 2;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "emoji" => {
                    let before = &rem[..first_idx];
                    let tag_len = if rem[first_idx..].starts_with("<a:") { 3 } else { 2 };
                    let after_tag = &rem[first_idx + tag_len..];
                    if let Some(gt_idx) = after_tag.find('>') {
                        let inside = &after_tag[..gt_idx];
                        if let Some(colon) = inside.find(':') {
                            let emoji_name = &inside[..colon];
                            let emoji_id = &inside[colon + 1..];
                            if !emoji_id.is_empty() && emoji_id.chars().all(|c| c.is_ascii_digit()) {
                                if !before.is_empty() {
                                    line_blocks.push(MessageBlockData {
                                        text: parse_discord_markdown(before),
                                        is_link: false,
                                        is_command: false,
                                        is_emoji: false,
                                        emoji_id: String::new(),
                                        url: String::new(),
                                        command_name: String::new(),
                                    });
                                }
                                line_blocks.push(MessageBlockData {
                                    text: format!(":{}:", emoji_name),
                                    is_link: false,
                                    is_command: false,
                                    is_emoji: true,
                                    emoji_id: emoji_id.to_string(),
                                    url: String::new(),
                                    command_name: String::new(),
                                });
                                rem = &after_tag[gt_idx + 1..];
                                continue;
                            }
                        }
                    }
                    let slice_len = first_idx + tag_len;
                    line_blocks.push(MessageBlockData {
                        text: parse_discord_markdown(&rem[..slice_len]),
                        is_link: false,
                        is_command: false,
                        is_emoji: false,
                        emoji_id: String::new(),
                        url: String::new(),
                        command_name: String::new(),
                    });
                    rem = &rem[slice_len..];
                }
                "raw_url" => {
                    let before = &rem[..first_idx];
                    let url_candidate = &rem[first_idx..];
                    
                    let end_pos = url_candidate.find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == ')' || c == '"' || c == '\'')
                        .unwrap_or(url_candidate.len());
                    
                    let mut raw_url = &url_candidate[..end_pos];
                    while raw_url.ends_with('.') || raw_url.ends_with(',') || raw_url.ends_with(';') {
                        raw_url = &raw_url[..raw_url.len() - 1];
                    }

                    if raw_url.len() > 8 {
                        if !before.is_empty() {
                            line_blocks.push(MessageBlockData {
                                text: parse_discord_markdown(before),
                                is_link: false,
                                is_command: false,
                                is_emoji: false,
                                emoji_id: String::new(),
                                url: String::new(),
                                command_name: String::new(),
                            });
                        }
                        line_blocks.push(MessageBlockData {
                            text: raw_url.to_string(),
                            is_link: true,
                            is_command: false,
                            is_emoji: false,
                            emoji_id: String::new(),
                            url: raw_url.to_string(),
                            command_name: String::new(),
                        });
                        if !links.iter().any(|l| l.url == raw_url) {
                            links.push(LinkData {
                                label: raw_url.to_string(),
                                url: raw_url.to_string(),
                            });
                        }
                        rem = &rem[first_idx + raw_url.len()..];
                    } else {
                        let slice_len = first_idx + 4;
                        line_blocks.push(MessageBlockData {
                            text: parse_discord_markdown(&rem[..slice_len]),
                            is_link: false,
                            is_command: false,
                            is_emoji: false,
                            emoji_id: String::new(),
                            url: String::new(),
                            command_name: String::new(),
                        });
                        rem = &rem[slice_len..];
                    }
                }
                "unicode_emoji" => {
                    if let Some((u_idx, u_len, u_text, u_id)) = find_next_unicode_emoji(rem) {
                        let before = &rem[..u_idx];
                        if !before.is_empty() {
                            line_blocks.push(MessageBlockData {
                                text: parse_discord_markdown(before),
                                is_link: false,
                                is_command: false,
                                is_emoji: false,
                                emoji_id: String::new(),
                                url: String::new(),
                                command_name: String::new(),
                            });
                        }
                        line_blocks.push(MessageBlockData {
                            text: u_text,
                            is_link: false,
                            is_command: false,
                            is_emoji: true,
                            emoji_id: u_id,
                            url: String::new(),
                            command_name: String::new(),
                        });
                        rem = &rem[u_idx + u_len..];
                    } else {
                        let slice_len = first_idx + 1;
                        rem = &rem[slice_len..];
                    }
                }
                _ => break,
            }
        }

        if !line_blocks.is_empty() {
            lines.push(MessageLineData { blocks: line_blocks });
        }
    }

    lines
}

pub fn format_decimal_color(color_val: &Value) -> String {
    if let Some(dec) = color_val.as_u64() {
        let r = (dec >> 16) & 0xFF;
        let g = (dec >> 8) & 0xFF;
        let b = dec & 0xFF;
        format!("#{:02X}{:02X}{:02X}", r, g, b)
    } else {
        "#5865f2".to_string() // Default blurple
    }
}

pub fn extract_and_clean_links(input: &str, links: &mut Vec<LinkData>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rem = input;
    while let Some(bracket_start) = rem.find('[') {
        out.push_str(&rem[..bracket_start]);
        let after_bracket = &rem[bracket_start + 1..];
        if let Some(bracket_end) = after_bracket.find(']') {
            let label = &after_bracket[..bracket_end];
            let after_label = &after_bracket[bracket_end + 1..];
            if after_label.starts_with('(') {
                if let Some(paren_end) = after_label.find(')') {
                    let url = &after_label[1..paren_end];
                    if url.starts_with("http") {
                        let link_label = if label.is_empty() { url.to_string() } else { label.to_string() };
                        if !links.iter().any(|l| l.url == url) {
                            links.push(LinkData {
                                label: link_label.clone(),
                                url: url.to_string(),
                            });
                        }
                        if label.is_empty() {
                            out.push_str(url);
                        } else {
                            out.push_str(&format!("🔗 {}", label));
                        }
                        rem = &after_label[paren_end + 1..];
                        continue;
                    }
                }
            }
            out.push('[');
        } else {
            out.push('[');
        }
        rem = &rem[bracket_start + 1..];
    }
    out.push_str(rem);

    // Also extract standalone raw URLs (http:// or https://) into links if not already present
    for word in out.split_whitespace() {
        let clean_word = word.trim_matches(|c| c == '<' || c == '>' || c == '(' || c == ')' || c == '"' || c == '\'' || c == '[' || c == ']');
        if (clean_word.starts_with("http://") || clean_word.starts_with("https://")) && clean_word.len() > 8 {
            if !links.iter().any(|l| l.url == clean_word) {
                links.push(LinkData {
                    label: clean_word.to_string(),
                    url: clean_word.to_string(),
                });
            }
        }
    }

    // Clean </name:id> to </name> for clean text representation
    while let Some(tag_start) = out.find("</") {
        let after = &out[tag_start + 2..];
        if let Some(tag_end) = after.find('>') {
            let inside = &after[..tag_end];
            if inside.contains(':') {
                let name = if let Some(colon) = inside.find(':') {
                    &inside[..colon]
                } else {
                    inside
                };
                let mut new_out = String::with_capacity(out.len());
                new_out.push_str(&out[..tag_start]);
                new_out.push_str(&format!("</{}>", name.trim()));
                new_out.push_str(&after[tag_end + 1..]);
                out = new_out;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    out
}

fn extract_triple_backtick_code_blocks(input: &str) -> (String, String) {
    let parts: Vec<&str> = input.split("```").collect();
    if parts.len() < 3 {
        return (input.to_string(), String::new());
    }

    let mut text_parts = Vec::new();
    let mut code_parts = Vec::new();

    for (idx, part) in parts.iter().enumerate() {
        if idx % 2 == 1 {
            // Inside code block
            let trimmed = part.trim_start_matches('\n');
            let (_, code_body) = if let Some(nl) = trimmed.find('\n') {
                let potential_lang = &trimmed[..nl];
                if potential_lang.len() <= 20 && potential_lang.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '-' || c == '_') {
                    (potential_lang, &trimmed[nl+1..])
                } else {
                    ("", trimmed)
                }
            } else {
                ("", trimmed)
            };
            code_parts.push(code_body.trim_end().to_string());
        } else {
            text_parts.push(*part);
        }
    }

    (text_parts.join(""), code_parts.join("\n\n"))
}

/// Returns (content, embed_content, embed_color, embed_footer, code_block, reply_author, reply_content, links) as separate components.
/// `content` has regular text, replies, interactions, stickers, components.
/// `embed_content` has all embed text (title, desc, fields etc.).
/// `embed_footer` has footer text.
/// `code_block` has extracted monospaced code blocks.
pub fn extract_commands_from_text(text: &str, commands: &mut Vec<String>) {
    let t = text.trim();
    if t.is_empty() { return; }

    // ONLY extract real Discord command mentions e.g. </name:id> or </name>
    let mut rem = t;
    while let Some(pos) = rem.find("</") {
        let after = &rem[pos + 2..];
        if let Some(end) = after.find('>') {
            let inside = &after[..end];
            let name = if let Some(colon) = inside.find(':') {
                &inside[..colon]
            } else {
                inside
            };
            let clean = name.trim().trim_start_matches('/');
            if !clean.is_empty() && clean.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                let cmd_str = format!("/{}", clean);
                if !commands.contains(&cmd_str) {
                    commands.push(cmd_str);
                }
            }
            rem = &after[end + 1..];
        } else {
            break;
        }
    }
}

pub fn format_discord_message_parts(m: &Value) -> (String, Vec<String>, Vec<MessageLineData>, String, Vec<MessageLineData>, String, String, String, String, String, String, Vec<LinkData>, Vec<MessageButtonData>, Vec<MessageAttachmentData>) {
    let mut content_parts: Vec<String> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    let mut content_lines: Vec<MessageLineData> = Vec::new();
    let mut embed_parts_all: Vec<String> = Vec::new();
    let mut embed_lines: Vec<MessageLineData> = Vec::new();
    let mut links: Vec<LinkData> = Vec::new();
    let mut buttons: Vec<MessageButtonData> = Vec::new();
    let mut attachments_list: Vec<MessageAttachmentData> = Vec::new();
    let mut embed_footer = String::new();
    let mut reply_author = String::new();
    let mut reply_content = String::new();
    let mut reply_command = String::new();
    let msg_type = m["type"].as_u64().unwrap_or(0);

    // Extract embed color (first embed's color if present)
    let embed_color = m["embeds"].as_array()
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("color"))
        .map(|c| format_decimal_color(c))
        .unwrap_or_else(|| "#5865f2".to_string());

    // Handle reply context (type 19)
    if msg_type == 19 {
        if let Some(ref_msg) = m["referenced_message"].as_object() {
            let ref_author = ref_msg.get("author")
                .and_then(|a| a["global_name"].as_str()
                    .or_else(|| a["username"].as_str()))
                .unwrap_or("alguem");
            let ref_content = ref_msg.get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let preview = if ref_content.chars().count() > 60 {
                format!("{}...", ref_content.chars().take(57).collect::<String>())
            } else if ref_content.is_empty() {
                "[midia/embed]".to_string()
            } else {
                ref_content.to_string()
            };
            reply_author = ref_author.to_string();
            reply_content = parse_discord_markdown(&preview);
        }
    }

    // Handle interaction context (type 20 = slash command, 23 = context menu, or message with interaction)
    if let Some(interaction) = m["interaction"].as_object()
        .or_else(|| m["interaction_metadata"].as_object()) {
        let cmd_name = interaction.get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("comando");
        let user_name = interaction.get("user")
            .and_then(|u| u["global_name"].as_str()
                .or_else(|| u["username"].as_str()))
            .unwrap_or("");
        let cmd_str = format!("/{}", cmd_name);
        if !commands.contains(&cmd_str) {
            commands.push(cmd_str.clone());
        }
        if !user_name.is_empty() {
            reply_author = user_name.to_string();
        }
        reply_command = cmd_str;
    }

    // 1. Raw Text Content — parsed into lines and regular text
    let mut code_block = String::new();
    if let Some(content) = m["content"].as_str() {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            extract_commands_from_text(trimmed, &mut commands);

            let (cleaned_text, extracted_code) = extract_triple_backtick_code_blocks(trimmed);
            if !extracted_code.is_empty() {
                code_block = extracted_code;
            }
            if !cleaned_text.trim().is_empty() {
                let parsed_lines = parse_text_into_lines(&cleaned_text, &mut links);
                if parsed_lines.iter().any(|l| l.blocks.iter().any(|b| b.is_link || b.is_command || b.is_emoji)) {
                    content_lines = parsed_lines;
                }
                let cleaned = extract_and_clean_links(&cleaned_text, &mut links);
                content_parts.push(parse_discord_markdown(&cleaned));
            }
        }
    }

    // 2. Embeds → go into embed_content & embed_lines
    if let Some(embeds) = m["embeds"].as_array() {
        for embed in embeds {
            let mut ep: Vec<String> = Vec::new();

            // Embed author
            if let Some(author_name) = embed["author"]["name"].as_str() {
                let a = author_name.trim();
                if !a.is_empty() {
                    extract_commands_from_text(a, &mut commands);
                    let cleaned = extract_and_clean_links(a, &mut links);
                    ep.push(format!("**{}**", parse_discord_markdown(&cleaned)));
                }
            }

            // Embed title (with optional URL)
            if let Some(title) = embed["title"].as_str() {
                let t = title.trim();
                if !t.is_empty() {
                    extract_commands_from_text(t, &mut commands);
                    let cleaned_title = extract_and_clean_links(t, &mut links);
                    let parsed_title = parse_discord_markdown(&cleaned_title);
                    if let Some(url) = embed["url"].as_str() {
                        if !url.is_empty() {
                            embed_lines.push(MessageLineData {
                                blocks: vec![MessageBlockData {
                                    text: parsed_title.clone(),
                                    is_link: true,
                                    is_command: false,
                                    is_emoji: false,
                                    emoji_id: String::new(),
                                    url: url.to_string(),
                                    command_name: String::new(),
                                }],
                            });
                            if !links.iter().any(|l| l.url == url) {
                                links.push(LinkData {
                                    label: format!("Titulo: {}", parsed_title),
                                    url: url.to_string(),
                                });
                            }
                        } else {
                            embed_lines.extend(parse_text_into_lines(t, &mut links));
                        }
                    } else {
                        embed_lines.extend(parse_text_into_lines(t, &mut links));
                    }
                    ep.push(parsed_title);
                }
            }

            // Embed description
            if let Some(desc) = embed["description"].as_str() {
                let d = desc.trim();
                if !d.is_empty() {
                    extract_commands_from_text(d, &mut commands);
                    embed_lines.extend(parse_text_into_lines(d, &mut links));
                    let cleaned = extract_and_clean_links(d, &mut links);
                    ep.push(parse_discord_markdown(&cleaned));
                }
            }

            // Embed fields (e.g. Duration, Artist, Track Number)
            if let Some(fields) = embed["fields"].as_array() {
                for field in fields {
                    let name = field["name"].as_str().unwrap_or("").trim();
                    let val  = field["value"].as_str().unwrap_or("").trim();
                    if !name.is_empty() && !val.is_empty() {
                        extract_commands_from_text(name, &mut commands);
                        extract_commands_from_text(val, &mut commands);

                        let field_line = format!("{}: {}", name, val);
                        embed_lines.extend(parse_text_into_lines(&field_line, &mut links));

                        let cleaned_name = extract_and_clean_links(name, &mut links);
                        let cleaned_val = extract_and_clean_links(val, &mut links);
                        ep.push(format!("{}: {}",
                            parse_discord_markdown(&cleaned_name),
                            parse_discord_markdown(&cleaned_val)));
                    } else if !val.is_empty() {
                        extract_commands_from_text(val, &mut commands);
                        embed_lines.extend(parse_text_into_lines(val, &mut links));
                        let cleaned_val = extract_and_clean_links(val, &mut links);
                        ep.push(parse_discord_markdown(&cleaned_val));
                    } else if !name.is_empty() {
                        extract_commands_from_text(name, &mut commands);
                        embed_lines.extend(parse_text_into_lines(name, &mut links));
                        let cleaned_name = extract_and_clean_links(name, &mut links);
                        ep.push(parse_discord_markdown(&cleaned_name));
                    }
                }
            }

            // Embed image
            if let Some(img_url) = embed["image"]["url"].as_str() {
                if !img_url.is_empty() {
                    links.push(LinkData {
                        label: "Ver Imagem do Embed".to_string(),
                        url: img_url.to_string(),
                    });
                }
            }

            // Embed thumbnail
            if let Some(thumb_url) = embed["thumbnail"]["url"].as_str() {
                if !thumb_url.is_empty() {
                    links.push(LinkData {
                        label: "Ver Miniatura do Embed".to_string(),
                        url: thumb_url.to_string(),
                    });
                }
            }

            // Embed timestamp
            if let Some(ts) = embed["timestamp"].as_str() {
                if !ts.is_empty() {
                    let display = if ts.chars().count() >= 10 { ts.chars().take(10).collect::<String>() } else { ts.to_string() };
                    ep.push(format!("[{}]", display));
                }
            }

            // Embed footer (accumulate footers)
            if let Some(footer) = embed["footer"]["text"].as_str() {
                let f = footer.trim();
                if !f.is_empty() {
                    extract_commands_from_text(f, &mut commands);
                    let cleaned = extract_and_clean_links(f, &mut links);
                    let footer_parsed = parse_discord_markdown(&cleaned);
                    if embed_footer.is_empty() {
                        embed_footer = footer_parsed;
                    } else {
                        embed_footer = format!("{}\n{}", embed_footer, footer_parsed);
                    }
                }
            }

            if !ep.is_empty() {
                embed_parts_all.push(ep.join("\n"));
            }
        }
    }

    // 3. Attachments → content & clickable links
    if let Some(attachments) = m["attachments"].as_array() {
        for att in attachments {
            let id = att["id"].as_str().unwrap_or("").to_string();
            let filename = att["filename"].as_str().unwrap_or("arquivo").to_string();
            let url = att["url"].as_str().unwrap_or("").to_string();
            let proxy_url = att["proxy_url"].as_str().unwrap_or(&url).to_string();
            let content_type = att["content_type"].as_str().unwrap_or("").to_string();
            let size_bytes = att["size"].as_u64().unwrap_or(0);
            let width = att["width"].as_i64().unwrap_or(0) as i32;
            let height = att["height"].as_i64().unwrap_or(0) as i32;

            let size_str = if size_bytes < 1024 {
                format!("{} B", size_bytes)
            } else if size_bytes < 1024 * 1024 {
                format!("{:.1} KB", size_bytes as f64 / 1024.0)
            } else {
                format!("{:.1} MB", size_bytes as f64 / (1024.0 * 1024.0))
            };

            let is_image = content_type.starts_with("image/") 
                || filename.ends_with(".png") 
                || filename.ends_with(".jpg") 
                || filename.ends_with(".jpeg") 
                || filename.ends_with(".webp") 
                || filename.ends_with(".gif");

            if !is_image {
                let label = if content_type.starts_with("video/") {
                    "Video"
                } else if content_type.starts_with("audio/") {
                    "Audio"
                } else {
                    "Anexo"
                };
                content_parts.push(format!("[{}: {}]", label, filename));
                if !url.is_empty() {
                    links.push(LinkData {
                        label: format!("Abrir {}: {}", label, filename),
                        url: url.clone(),
                    });
                }
            } else if !url.is_empty() {
                links.push(LinkData {
                    label: format!("Imagem: {}", filename),
                    url: url.clone(),
                });
            }

            attachments_list.push(MessageAttachmentData {
                id,
                filename,
                url,
                proxy_url,
                size_bytes,
                size_str,
                width,
                height,
                content_type,
                is_image,
            });
        }
    }

    // 4. Stickers → content
    if let Some(stickers) = m["sticker_items"].as_array() {
        for st in stickers {
            let st_name = st["name"].as_str().unwrap_or("Sticker");
            content_parts.push(format!("[Sticker: {}]", st_name));
        }
    }

    // 5. Components V2 → Rich interactive buttons (ActionRows)
    if let Some(components) = m["components"].as_array() {
        for row in components {
            if let Some(row_components) = row["components"].as_array() {
                for comp in row_components {
                    let comp_type = comp["type"].as_u64().unwrap_or(0);
                    match comp_type {
                        2 => {
                            let label = comp["label"].as_str().unwrap_or("").trim();
                            let url = comp["url"].as_str().unwrap_or("").trim();
                            let style = comp["style"].as_i64().unwrap_or(if !url.is_empty() { 5 } else { 2 }) as i32;
                            let is_disabled = comp["disabled"].as_bool().unwrap_or(false);
                            
                            let mut emoji_str = String::new();
                            let mut emoji_id_for_btn = String::new();
                            if let Some(emoji_obj) = comp.get("emoji") {
                                if let Some(id) = emoji_obj["id"].as_str() {
                                    if !id.is_empty() {
                                        emoji_id_for_btn = id.to_string();
                                        emoji_str = emoji_obj["name"].as_str().unwrap_or("").to_string();
                                    }
                                }
                                if emoji_id_for_btn.is_empty() {
                                    if let Some(emoji_name) = emoji_obj["name"].as_str() {
                                        emoji_str = emoji_name.to_string();
                                        if let Some((_, _, _, hex_id)) = find_next_unicode_emoji(emoji_name) {
                                            emoji_id_for_btn = hex_id;
                                        }
                                    }
                                }
                            }

                            if !label.is_empty() || !emoji_str.is_empty() {
                                buttons.push(MessageButtonData {
                                    label: label.to_string(),
                                    url: url.to_string(),
                                    emoji: emoji_str,
                                    emoji_id: emoji_id_for_btn,
                                    style_type: style,
                                    is_disabled,
                                });
                            }
                        }
                        3 | 5 | 6 | 7 | 8 => {
                            if let Some(placeholder) = comp["placeholder"].as_str() {
                                if !placeholder.is_empty() {
                                    buttons.push(MessageButtonData {
                                        label: placeholder.to_string(),
                                        url: String::new(),
                                        emoji: "📋".to_string(),
                                        emoji_id: "u_1f4cb".to_string(),
                                        style_type: 2,
                                        is_disabled: true,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Build content string
    let content = if content_parts.is_empty() && embed_parts_all.is_empty() && buttons.is_empty() && attachments_list.is_empty() {
        match msg_type {
            1 => "[Membro adicionado ao grupo]".to_string(),
            2 => "[Membro removido do grupo]".to_string(),
            3 => "[Chamada]".to_string(),
            4 => "[Nome do canal alterado]".to_string(),
            5 => "[Icone do canal alterado]".to_string(),
            6 => "[Mensagem Fixada]".to_string(),
            7 => "[Novo membro entrou no servidor!]".to_string(),
            8 => "[Servidor Impulsionado!]".to_string(),
            9 => "[Servidor alcancou nivel 1 de impulso!]".to_string(),
            10 => "[Servidor alcancou nivel 2 de impulso!]".to_string(),
            11 => "[Servidor alcancou nivel 3 de impulso!]".to_string(),
            12 => "[Canal seguido adicionado]".to_string(),
            14 => "[Servidor desqualificado da descoberta]".to_string(),
            15 => "[Servidor requalificado na descoberta]".to_string(),
            19 => String::new(),
            20 => String::new(),
            21 => "[Mensagem inicial de thread]".to_string(),
            22 => "[Lembrete de convite]".to_string(),
            23 => String::new(),
            24 => "[Acao do AutoMod]".to_string(),
            25 => "[Compra de assinatura de cargo]".to_string(),
            _ => "[Conteudo especial]".to_string(),
        }
    } else {
        content_parts.join("\n")
    };

    let embed_content = embed_parts_all.join("\n\n");

    (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments_list)
}

/// Legacy wrapper — returns combined content + embed as a single string.
#[allow(dead_code)]
pub fn format_discord_message(m: &Value) -> String {
    let (content, _, _, embed, _, _, _, _, _, _, _, _, _, _) = format_discord_message_parts(m);
    if embed.is_empty() {
        content
    } else if content.is_empty() {
        embed
    } else {
        format!("{}\n{}", content, embed)
    }
}

