//! Posting a squad's news to a Discord channel through a webhook: reading the webhook URL people
//! paste, and writing the messages.
//!
//! Only Discord's own webhook URLs are accepted, so a squad can't make the server send requests
//! anywhere else. What's kept is the webhook's path (`/api/webhooks/<id>/<token>`, with an
//! optional `?thread_id=`), and messages go to Discord's API host, or a fake in tests.

use std::fmt;

/// Hosts Discord serves webhooks on.
const HOSTS: [&str; 4] = [
    "discord.com",
    "discordapp.com",
    "canary.discord.com",
    "ptb.discord.com",
];

/// A Discord webhook: its id, its secret token, and optionally a thread to post in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Webhook {
    id: String,
    token: String,
    thread: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error(
        "that isn't a Discord webhook URL: copy it from the channel's settings, Integrations, Webhooks"
    )]
    NotAWebhook,
}

impl Webhook {
    /// Reads a webhook URL as Discord shows it, e.g.
    /// `https://discord.com/api/webhooks/123456789012345678/AbC-dEf_...`, optionally with an API
    /// version (`/api/v10/webhooks/...`) or `?thread_id=<id>`.
    ///
    /// # Errors
    ///
    /// When it isn't an `https` URL on a Discord host with a webhook path.
    pub fn parse(url: &str) -> Result<Self, WebhookError> {
        let rest = url
            .trim()
            .strip_prefix("https://")
            .ok_or(WebhookError::NotAWebhook)?;
        let (host, path) = rest.split_once('/').ok_or(WebhookError::NotAWebhook)?;
        if !HOSTS.contains(&host.to_ascii_lowercase().as_str()) {
            return Err(WebhookError::NotAWebhook);
        }
        Self::from_path(&format!("/{path}"))
    }

    /// Reads what [`Webhook::path`] wrote.
    ///
    /// # Errors
    ///
    /// When it isn't a webhook path.
    pub fn from_path(path: &str) -> Result<Self, WebhookError> {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let path = path
            .strip_prefix("/api/")
            .ok_or(WebhookError::NotAWebhook)?;
        // An optional API version, `v10/`.
        let path = match path.split_once('/') {
            Some((v, rest)) if v.len() > 1 && v.starts_with('v') && is_digits(&v[1..]) => rest,
            _ => path,
        };
        let mut parts = path
            .strip_prefix("webhooks/")
            .ok_or(WebhookError::NotAWebhook)?
            .trim_end_matches('/')
            .split('/');
        let (Some(id), Some(token), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(WebhookError::NotAWebhook);
        };
        let token_ok = (1..=200).contains(&token.len())
            && token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
        if !is_snowflake(id) || !token_ok {
            return Err(WebhookError::NotAWebhook);
        }
        // Other query parameters (e.g. `wait`) are ours to choose, so they're dropped.
        let thread = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("thread_id="))
            .map(str::to_owned);
        if thread.as_deref().is_some_and(|t| !is_snowflake(t)) {
            return Err(WebhookError::NotAWebhook);
        }
        Ok(Self {
            id: id.to_owned(),
            token: token.to_owned(),
            thread,
        })
    }

    /// `/api/webhooks/<id>/<token>`, with `?thread_id=` for a thread: what's stored, and where
    /// messages are posted on Discord's host.
    #[must_use]
    pub fn path(&self) -> String {
        let thread = self
            .thread
            .as_deref()
            .map_or_else(String::new, |t| format!("?thread_id={t}"));
        format!("/api/webhooks/{}/{}{thread}", self.id, self.token)
    }

    /// Enough to recognise the webhook without giving away its token: the end of its id.
    #[must_use]
    pub fn hint(&self) -> String {
        let start = self.id.len().saturating_sub(4);
        format!("…{}", &self.id[start..])
    }
}

/// Not `Display`ed by accident: the token is a secret, so only the hint shows.
impl fmt::Display for Webhook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "webhook {}", self.hint())
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}

/// A Discord id: a number of up to 20 digits.
fn is_snowflake(s: &str) -> bool {
    is_digits(s) && s.len() <= 20
}

/// The new level, when a character went up: both known and the new one higher.
#[must_use]
pub fn level_up(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    match (before, after) {
        (Some(before), Some(after)) if after > before => Some(after),
        _ => None,
    }
}

/// Text that Discord shows as typed: Markdown's special characters escaped. Mentions are
/// switched off when posting, so `@everyone` in a squad name can't ping anyone.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*'
                | '_'
                | '~'
                | '`'
                | '|'
                | '>'
                | '#'
                | '-'
                | '['
                | ']'
                | '('
                | ')'
                | '<'
                | ':'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The squad's name, linked to its page when the server knows its own address. The `<>` stop
/// Discord from adding a preview of the page under every message.
fn squad(name: &str, link: Option<&str>) -> String {
    match link {
        Some(link) => format!("[{}](<{link}>)", escape(name)),
        None => escape(name),
    }
}

/// Posted when a squad starts using a webhook, so whoever set it up sees it works.
#[must_use]
pub fn hello(squad_name: &str, link: Option<&str>) -> String {
    format!(
        "zesquad will post here when someone joins {} or goes up a level.",
        squad(squad_name, link)
    )
}

#[must_use]
pub fn joined(character: &str, squad_name: &str, link: Option<&str>) -> String {
    format!(
        "**{}** joined {}.",
        escape(character),
        squad(squad_name, link)
    )
}

#[must_use]
pub fn levelled(character: &str, level: u64, squad_name: &str, link: Option<&str>) -> String {
    format!(
        "**{}** reached level {level} · {}",
        escape(character),
        squad(squad_name, link)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ID: &str = "123456789012345678";
    const TOKEN: &str = "AbC-dEf_123";

    #[test]
    fn reads_webhook_urls() {
        let hook =
            Webhook::parse(&format!("https://discord.com/api/webhooks/{ID}/{TOKEN}")).unwrap();
        assert_eq!(hook.path(), format!("/api/webhooks/{ID}/{TOKEN}"));
        assert_eq!(hook.hint(), "…5678");
        assert!(!hook.to_string().contains(TOKEN));
        for url in [
            format!("  https://discordapp.com/api/webhooks/{ID}/{TOKEN}/ "),
            format!("https://canary.discord.com/api/v10/webhooks/{ID}/{TOKEN}"),
            format!("https://Discord.com/api/webhooks/{ID}/{TOKEN}?wait=true"),
        ] {
            assert_eq!(Webhook::parse(&url), Ok(hook.clone()), "{url}");
        }
        let thread = Webhook::parse(&format!(
            "https://discord.com/api/webhooks/{ID}/{TOKEN}?thread_id=42&wait=1"
        ))
        .unwrap();
        assert_eq!(
            thread.path(),
            format!("/api/webhooks/{ID}/{TOKEN}?thread_id=42")
        );
        assert_eq!(Webhook::from_path(&thread.path()), Ok(thread));
    }

    #[test]
    fn refuses_anything_else() {
        for url in [
            "",
            "discord.com/api/webhooks/1/x",
            "http://discord.com/api/webhooks/1/x",
            "https://evil.example/api/webhooks/1/x",
            "https://discord.com.evil.example/api/webhooks/1/x",
            "https://evil.example/https://discord.com/api/webhooks/1/x",
            "https://discord.com@evil.example/api/webhooks/1/x",
            "https://discord.com/api/webhooks/1",
            "https://discord.com/api/webhooks/1/x/extra",
            "https://discord.com/api/webhooks/abc/x",
            "https://discord.com/api/webhooks/1/x y",
            "https://discord.com/api/webhooks/1/../../users/@me",
            "https://discord.com/api/channels/1/messages",
            "https://discord.com/api/webhooks/1/x?thread_id=abc",
            "https://discord.com/api/webhooks/123456789012345678901/x",
        ] {
            assert_eq!(Webhook::parse(url), Err(WebhookError::NotAWebhook), "{url}");
        }
    }

    #[test]
    fn levels_up_only_upwards() {
        assert_eq!(level_up(Some(26), Some(27)), Some(27));
        assert_eq!(level_up(Some(26), Some(29)), Some(29));
        assert_eq!(level_up(Some(27), Some(27)), None);
        assert_eq!(level_up(Some(27), Some(26)), None);
        assert_eq!(level_up(None, Some(27)), None);
        assert_eq!(level_up(Some(27), None), None);
    }

    #[test]
    fn messages() {
        let link = Some("https://zesquad.example/s/abc");
        assert_eq!(
            joined("Aldric Thornwood", "Thursday *raiders*", link),
            "**Aldric Thornwood** joined [Thursday \\*raiders\\*](<https://zesquad.example/s/abc>)."
        );
        assert_eq!(
            levelled("Aldric Thornwood", 27, "Raiders", None),
            "**Aldric Thornwood** reached level 27 · Raiders"
        );
        assert!(hello("Raiders", None).contains("Raiders"));
        // A name can't close the link early or start a new one.
        assert_eq!(
            squad("a](<https://evil.example>) [b", link),
            "[a\\]\\(\\<https\\://evil.example\\>\\) \\[b](<https://zesquad.example/s/abc>)"
        );
    }

    proptest! {
        #[test]
        fn accepted_urls_keep_only_a_discord_path(url in ".{0,120}") {
            if let Ok(hook) = Webhook::parse(&url) {
                let path = hook.path();
                prop_assert!(path.starts_with("/api/webhooks/"));
                prop_assert!(!path.contains(".."));
                prop_assert_eq!(Webhook::from_path(&path), Ok(hook));
            }
        }

        #[test]
        fn real_looking_urls_read_back(id in "[0-9]{17,20}", token in "[A-Za-z0-9_-]{60,68}") {
            let hook = Webhook::parse(&format!("https://discord.com/api/webhooks/{id}/{token}")).unwrap();
            prop_assert_eq!(hook.path(), format!("/api/webhooks/{id}/{token}"));
        }

        #[test]
        fn escaped_text_has_no_bare_markdown(text in ".{0,60}") {
            let escaped = escape(&text);
            let mut chars = escaped.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    prop_assert!(chars.next().is_some());
                } else {
                    prop_assert!(!"*_~`|>#[]()<:".contains(c), "{escaped}");
                }
            }
        }
    }
}
