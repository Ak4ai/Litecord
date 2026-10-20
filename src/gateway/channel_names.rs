//! Names of the channels loaded so far, so a `<#id>` mention can show the channel it
//! points to. Filled when a server's channel list is loaded; a channel of a server that
//! was not opened yet is simply unknown.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

fn names() -> &'static RwLock<HashMap<String, (String, String)>> {
    static NAMES: OnceLock<RwLock<HashMap<String, (String, String)>>> = OnceLock::new();
    NAMES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Records the channels of one server (`guild_id`) as (id, name) pairs.
pub fn remember<'a>(guild_id: &str, channels: impl IntoIterator<Item = (&'a str, &'a str)>) {
    if let Ok(mut map) = names().write() {
        for (id, name) in channels {
            map.insert(id.to_string(), (guild_id.to_string(), name.to_string()));
        }
    }
}

/// The server id and name of a known channel.
pub fn lookup(channel_id: &str) -> Option<(String, String)> {
    names().read().ok()?.get(channel_id).cloned()
}

/// Rewrites `<#id>` mentions as links to the channel, `[#name](https://discord.com/channels/guild/id)`,
/// so the line parser shows them as clickable links. Unknown channels read `#canal`, as before.
pub fn link_channel_mentions(input: &str) -> String {
    if !input.contains("<#") {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("<#") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('>') {
            Some(end) if end > 0 && after[..end].chars().all(|c| c.is_ascii_digit()) => {
                let id = &after[..end];
                match lookup(id) {
                    Some((guild_id, name)) => {
                        // `]` would end the label early.
                        let label = name.replace(']', ")");
                        out.push_str(&format!("[#{label}](https://discord.com/channels/{guild_id}/{id})"));
                    }
                    None => out.push_str("#canal"),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("<#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The channel id of a `https://discord.com/channels/<guild>/<channel>` link.
pub fn channel_link_target(url: &str) -> Option<(String, String)> {
    let path = url
        .strip_prefix("https://discord.com/channels/")
        .or_else(|| url.strip_prefix("https://discordapp.com/channels/"))
        .or_else(|| url.strip_prefix("https://ptb.discord.com/channels/"))
        .or_else(|| url.strip_prefix("https://canary.discord.com/channels/"))?;
    let mut parts = path.split(['/', '?', '#']);
    let guild = parts.next()?;
    let channel = parts.next()?;
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (numeric(channel) && (numeric(guild) || guild == "@me")).then(|| (guild.to_string(), channel.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_mentions_become_links_and_unknown_ones_read_canal() {
        remember("900", [("901", "🔗|links-úteis")]);
        assert_eq!(
            link_channel_mentions("<#901> Para encontrar <#999> e <#abc>"),
            "[#🔗|links-úteis](https://discord.com/channels/900/901) Para encontrar #canal e <#abc>"
        );
    }

    #[test]
    fn channel_links_name_their_server_and_channel() {
        assert_eq!(
            channel_link_target("https://discord.com/channels/900/901"),
            Some(("900".to_string(), "901".to_string()))
        );
        assert_eq!(channel_link_target("https://discord.com/channels/900/901/555").unwrap().1, "901");
        assert!(channel_link_target("https://discord.com/channels/900").is_none());
        assert!(channel_link_target("https://example.com/channels/900/901").is_none());
    }
}
