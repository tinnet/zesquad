//! Calls the real Battle.net API. Ignored by `just test`; `just network-test` runs it with the
//! credentials from .env and a real character in `ZESQUAD_TEST_CHARACTER` (`Name-Surname`, EU).
//! Real character names stay out of the repo, so the character comes from the environment.

use zesquad::armory::{Client, Config, Lookup};
use zesquad_core::CharacterName;
use zesquad_core::armory::{Documents, Endpoint, Profile};

fn client() -> Client {
    let _ = dotenvy::dotenv();
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name} (or put it in .env)"))
    };
    let config = Config::new(
        var("ZESQUAD_BNET_CLIENT_ID"),
        var("ZESQUAD_BNET_CLIENT_SECRET"),
        "eu".into(),
        None,
        "https://oauth.battle.net/token",
    )
    .unwrap();
    Client::new(config).unwrap()
}

#[tokio::test]
#[ignore = "calls the real Battle.net API"]
async fn looks_up_a_real_forever_character() {
    let _ = dotenvy::dotenv();
    let name: CharacterName = std::env::var("ZESQUAD_TEST_CHARACTER")
        .expect("set ZESQUAD_TEST_CHARACTER=Name-Surname to a Forever character in EU")
        .parse()
        .unwrap();
    let client = client();
    let mut docs = Documents::default();
    for endpoint in [Endpoint::Summary, Endpoint::Media, Endpoint::Equipment] {
        match client.character(&name.key(), endpoint).await.unwrap() {
            Lookup::Found(doc) => docs.set(endpoint, doc),
            Lookup::NotFound => panic!("{} not found on {}", endpoint.as_str(), name.key()),
        }
    }
    let profile = Profile::from_documents(&docs);
    assert_eq!(
        profile.name.as_deref().map(str::to_lowercase),
        Some(name.name().to_lowercase())
    );
    assert!(
        profile.level.is_some() && profile.class.is_some(),
        "{profile:?}"
    );
    assert!(profile.avatar_url.is_some(), "{profile:?}");
}

#[tokio::test]
#[ignore = "calls the real Battle.net API"]
async fn unknown_characters_are_not_found() {
    let lookup = client()
        .character("nobody-xqzvjw", Endpoint::Summary)
        .await
        .unwrap();
    assert!(matches!(lookup, Lookup::NotFound));
}
