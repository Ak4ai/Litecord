//! The one place that says how Litecord identifies itself to Discord.
//!
//! The REST client, the Gateway IDENTIFY payload and the QR login used to carry
//! three different descriptions (an Electron desktop user agent on REST, Chrome
//! 127 on the Gateway, Chrome 130 on the QR login), so one process reported
//! contradicting environments depending on the connection (#30).
//!
//! Everything here describes a single profile, the web client in Chrome:
//!   * `user_agent()` goes in the `User-Agent` of every request to Discord;
//!   * `properties()` is the `properties` object of the Gateway IDENTIFY;
//!   * `super_properties_header()` is the same object, base64-encoded, sent as
//!     `X-Super-Properties` on REST requests.
//! The three are derived from the same constants, so they cannot drift apart:
//! `browser_user_agent` always equals the `User-Agent` header.
//!
//! The operating system and locale are the real ones. Optional fields that the
//! official clients generate per launch (`client_launch_id`,
//! `client_heartbeat_session_id`, `launch_signature`) are deliberately left out:
//! they are not required, and a made-up value would be worse than none.
//!
//! Maintenance: `CHROME_VERSION` and `CLIENT_BUILD_NUMBER` are the only values
//! that age. Refreshing them when cutting a Litecord release is enough.

use base64::Engine;
use serde_json::{json, Value};

use super::i18n::{self, Language};

const CHROME_VERSION: &str = "127.0.0.0";
const CLIENT_BUILD_NUMBER: u32 = 320000;

#[cfg(target_os = "windows")]
const OS_NAME: &str = "Windows";
#[cfg(target_os = "windows")]
const OS_VERSION: &str = "10";
#[cfg(target_os = "windows")]
const UA_PLATFORM: &str = "Windows NT 10.0; Win64; x64";

#[cfg(target_os = "macos")]
const OS_NAME: &str = "Mac OS X";
#[cfg(target_os = "macos")]
const OS_VERSION: &str = "10.15.7";
#[cfg(target_os = "macos")]
const UA_PLATFORM: &str = "Macintosh; Intel Mac OS X 10_15_7";

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const OS_NAME: &str = "Linux";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const OS_VERSION: &str = "";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UA_PLATFORM: &str = "X11; Linux x86_64";

/// `User-Agent` for every HTTP and WebSocket request made to Discord.
pub fn user_agent() -> String {
    format!(
        "Mozilla/5.0 ({UA_PLATFORM}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{CHROME_VERSION} Safari/537.36"
    )
}

/// Locale of the operating system, in the form the web client reports it.
fn system_locale() -> &'static str {
    match i18n::detect_os_language() {
        Language::Portuguese => "pt-BR",
        Language::Spanish => "es-ES",
        Language::German => "de",
        Language::French => "fr",
        Language::Russian => "ru",
        Language::Japanese => "ja",
        _ => "en-US",
    }
}

/// Client properties, as sent in the Gateway IDENTIFY payload.
pub fn properties() -> Value {
    json!({
        "os": OS_NAME,
        "browser": "Chrome",
        "device": "",
        "system_locale": system_locale(),
        "browser_user_agent": user_agent(),
        "browser_version": CHROME_VERSION,
        "os_version": OS_VERSION,
        "referrer": "",
        "referring_domain": "",
        "referrer_current": "",
        "referring_domain_current": "",
        "release_channel": "stable",
        "client_build_number": CLIENT_BUILD_NUMBER,
        "client_event_source": null
    })
}

/// Value of the `X-Super-Properties` header: `properties()` as base64 JSON.
pub fn super_properties_header() -> String {
    base64::engine::general_purpose::STANDARD.encode(properties().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_carries_exactly_the_gateway_properties() {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(super_properties_header())
            .expect("header is valid base64");
        let decoded: Value = serde_json::from_slice(&raw).expect("header is valid JSON");
        assert_eq!(decoded, properties());
    }

    #[test]
    fn declared_user_agent_is_the_one_sent() {
        let props = properties();
        assert_eq!(props["browser_user_agent"], user_agent());
        assert!(user_agent().contains(&format!("Chrome/{}", props["browser_version"].as_str().unwrap())));
    }

    #[test]
    fn profile_is_a_browser_not_the_desktop_app() {
        let ua = user_agent();
        assert!(!ua.contains("Electron") && !ua.contains("Discord/"), "{ua}");
        assert_eq!(properties()["browser"], "Chrome");
    }

    #[test]
    fn operating_system_matches_the_build_target() {
        let props = properties();
        let ua = user_agent();
        if cfg!(target_os = "windows") {
            assert_eq!(props["os"], "Windows");
            assert!(ua.contains("Windows NT"));
        } else if cfg!(target_os = "linux") {
            assert_eq!(props["os"], "Linux");
            assert!(ua.contains("X11; Linux"));
        }
    }

    #[test]
    fn header_is_a_valid_http_header_value() {
        assert!(reqwest::header::HeaderValue::from_str(&super_properties_header()).is_ok());
    }
}
