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

/// `**Name** joined Squad.`, and a line of welcome under it.
#[must_use]
pub fn joined(character: &str, squad_name: &str, link: Option<&str>) -> String {
    let flavour = pick(JOINED, &[character]);
    format!(
        "**{}** joined {}.\n-# {flavour}",
        escape(character),
        squad(squad_name, link)
    )
}

/// `**Name** reached level N · Squad`, and a line of flavour under it: one for the milestone
/// when the level is one, otherwise one for the class or anyone.
#[must_use]
pub fn levelled(
    character: &str,
    level: u64,
    class: Option<&str>,
    squad_name: &str,
    link: Option<&str>,
) -> String {
    let flavour = level_flavour(character, level, class);
    format!(
        "**{}** reached level {level} · {}\n-# {flavour}",
        escape(character),
        squad(squad_name, link)
    )
}

/// The same character reaching the same level always gets the same line, so a message is
/// reproducible, but different characters and levels get a mix.
fn level_flavour(character: &str, level: u64, class: Option<&str>) -> &'static str {
    let level_text = level.to_string();
    let seed = [character, level_text.as_str()];
    if let Some(line) = class.and_then(|class| class_milestone(class, level)) {
        return line;
    }
    if let Some(lines) = milestone(level) {
        return pick(lines, &seed);
    }
    let class_lines = class.map_or(&[][..], class_lines);
    let pool: Vec<&'static str> = class_lines.iter().chain(LEVELLED).copied().collect();
    pick(&pool, &seed)
}

/// One of `lines`, chosen by hashing `seed` (FNV-1a), so no randomness is needed.
fn pick(lines: &[&'static str], seed: &[&str]) -> &'static str {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in seed {
        for byte in part.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let len = u64::try_from(lines.len()).unwrap_or(u64::MAX);
    lines[usize::try_from(hash % len).unwrap_or(0)]
}

const JOINED: &[&str] = &[
    "Fresh recruit spotted. Someone show them where the mailbox is.",
    "The squad grows. Group quests just got easier.",
    "Welcome aboard! Mind the murlocs.",
    "Another adventurer answers the call.",
    "LFM no more: one more joins the party.",
    "Grab a hearthstone and pull up a campfire.",
    "Rumour has it they brought snacks. And bandages.",
    "The inn has another regular.",
];

const LEVELLED: &[&str] = &[
    "Ding! Somewhere, a chat channel fills up with grats.",
    "The quest log is lighter. The bags are heavier.",
    "Rested XP, well spent.",
    "The class trainer will see you now. Bring gold.",
    "Grey mobs tremble. Green mobs nervously reconsider.",
    "Corpse runs survived: several. Levels gained: one more.",
    "Off to find out why that one quest is still red.",
    "One step closer to the next bag. Or the next mount.",
    "Nobody tell the flight master, but it was mostly walking.",
    "Somewhere a vendor is already eyeing the grey items.",
];

/// Lines for the levels everyone remembers.
fn milestone(level: u64) -> Option<&'static [&'static str]> {
    Some(match level {
        10 => &[
            "Talent points unlocked. Time to agonise over the first one.",
            "Double digits! The first talent point is burning a hole in the pocket.",
        ],
        20 => &[
            "Level 20. Dungeons are calling, and so is the repair bill.",
            "Twenty levels in. The world map suddenly looks a lot bigger.",
        ],
        30 => &[
            "Halfway to the top. The hard part is the other half.",
            "Thirty! Scarlet Monastery has opened its doors. Mind the zealots.",
        ],
        40 => &[
            "Level 40: mount time, if the gold is there. It never is.",
            "Forty! Riding lessons await, and so does a lighter purse.",
        ],
        50 => &[
            "Fifty. Blackrock Mountain is looming on the horizon.",
            "Ten more to go. The endgame can smell them coming.",
        ],
        60 => &[
            "Max level! Welcome to the endgame. Raid starts at eight, don't be late.",
            "Sixty! No more experience bars, only loot tables now.",
            "The journey is complete. Now the real grind begins.",
        ],
        _ => return None,
    })
}

/// Milestones that only some classes get, from the trainers and class quests of the era.
fn class_milestone(class: &str, level: u64) -> Option<&'static str> {
    Some(match (class, level) {
        ("Hunter", 10) => "A pet of their own at last. Go tame something with teeth.",
        ("Warrior", 10) => "Defensive Stance is on the horizon. Shields up.",
        ("Warlock", 10) => "The Voidwalker answers. Someone else gets to take the hits now.",
        ("Druid", 10) => "Bear Form awakens. Hibernation is not an option.",
        ("Druid", 20) => "Cat Form unlocked. Prowl responsibly.",
        ("Shaman", 20) => "Ghost Wolf! Walking is for other people now.",
        ("Warlock", 20) => "The Succubus has arrived. Nobody ask.",
        ("Druid", 30) => "Travel Form. The road just got a lot shorter.",
        ("Warrior", 30) => "Berserker Stance unlocked. Indoor voices no longer apply.",
        ("Warlock", 30) => "A Felhunter joins the household. The mages are nervous.",
        ("Paladin", 40) => "A Warhorse, summoned out of thin air. No stable fees.",
        ("Warlock", 40) => "Felsteed summoned. Walking is officially over.",
        _ => return None,
    })
}

/// Lines any character of a class can get, at any level that isn't a milestone.
fn class_lines(class: &str) -> &'static [&'static str] {
    match class {
        "Warrior" => &[
            "Rage generated, enemies thoroughly charged.",
            "Still no healer, still standing.",
            "Bandages remain a warrior's best friend.",
        ],
        "Paladin" => &[
            "Seal, judge, repeat.",
            "Blessed be the level. Now buff everyone.",
            "No bubble-hearth was needed this time.",
        ],
        "Hunter" => &[
            "The pet did most of the work. The hunter takes the credit.",
            "Arrows restocked? Arrows restocked.",
            "Feed the pet. Seriously, feed the pet.",
        ],
        "Rogue" => &[
            "Nobody saw it happen.",
            "Pockets were picked on the way up.",
            "Sap, sneak, ding.",
        ],
        "Priest" => &[
            "Healed through it all, including the party's poor decisions.",
            "Power Word. Ding.",
            "The wand was doing a lot of the heavy lifting.",
        ],
        "Shaman" => &[
            "The totems are proud.",
            "The elements approve.",
            "Windfury would have been faster. It was.",
        ],
        "Mage" => &[
            "Conjured a whole level out of thin air.",
            "Free water for everyone. Form an orderly line.",
            "Frost Nova, step back, ding.",
        ],
        "Warlock" => &[
            "Soul shards in the bag, a new level in the pocket.",
            "The demon is unimpressed. The warlock is not.",
            "Life Tap, drink, repeat.",
        ],
        "Druid" => &[
            "Bear, cat, or seal? Yes.",
            "Shapeshifted straight into a new level.",
            "Nature provides.",
        ],
        _ => &[],
    }
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
        let message = joined("Aldric Thornwood", "Thursday *raiders*", link);
        assert!(
            message.starts_with(
                "**Aldric Thornwood** joined [Thursday \\*raiders\\*](<https://zesquad.example/s/abc>).\n-# "
            ),
            "{message}"
        );
        let message = levelled("Aldric Thornwood", 27, Some("Rogue"), "Raiders", None);
        assert!(
            message.starts_with("**Aldric Thornwood** reached level 27 · Raiders\n-# "),
            "{message}"
        );
        assert!(hello("Raiders", None).contains("Raiders"));
        // A name can't close the link early or start a new one.
        assert_eq!(
            squad("a](<https://evil.example>) [b", link),
            "[a\\]\\(\\<https\\://evil.example\\>\\) \\[b](<https://zesquad.example/s/abc>)"
        );
    }

    fn flavour(message: &str) -> &str {
        message.split_once("\n-# ").unwrap().1
    }

    #[test]
    fn milestones_and_classes_get_their_own_lines() {
        let message = levelled("Brena Ashvale", 10, Some("Hunter"), "Raiders", None);
        assert_eq!(
            flavour(&message),
            "A pet of their own at last. Go tame something with teeth."
        );
        // A class without a milestone at that level gets the general one.
        let message = levelled("Brena Ashvale", 60, Some("Rogue"), "Raiders", None);
        assert!(milestone(60).unwrap().contains(&flavour(&message)));
        // An unknown class, or none, still gets a line.
        for class in [None, Some("Monk")] {
            let message = levelled("Brena Ashvale", 27, class, "Raiders", None);
            assert!(LEVELLED.contains(&flavour(&message)), "{message}");
        }
        // The same event always reads the same, but there's variety across characters.
        assert_eq!(
            levelled("Brena Ashvale", 27, Some("Mage"), "Raiders", None),
            levelled("Brena Ashvale", 27, Some("Mage"), "Raiders", None)
        );
        let lines: std::collections::HashSet<_> = (2..60)
            .map(|level| {
                flavour(&levelled("Brena Ashvale", level, Some("Mage"), "R", None)).to_owned()
            })
            .collect();
        assert!(lines.len() > 5, "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| class_lines("Mage").contains(&line.as_str()))
        );
    }

    #[test]
    fn flavour_is_plain_text() {
        let classes = [
            "Warrior", "Paladin", "Hunter", "Rogue", "Priest", "Shaman", "Mage", "Warlock", "Druid",
        ];
        let mut all: Vec<&str> = JOINED.iter().chain(LEVELLED).copied().collect();
        for level in 1..=60 {
            all.extend(milestone(level).unwrap_or_default());
            all.extend(classes.iter().filter_map(|c| class_milestone(c, level)));
        }
        all.extend(classes.iter().flat_map(|c| class_lines(c)));
        for class in classes {
            assert!(!class_lines(class).is_empty(), "{class}");
        }
        for line in all {
            assert!(!line.is_empty() && !line.contains('\n'), "{line:?}");
            assert!(
                !line.contains(['*', '_', '~', '`', '|', '>', '#', '[', ']', '<', '@', '\\']),
                "{line}"
            );
        }
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
