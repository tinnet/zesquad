//! Calls the real Battle.net API. Ignored by `just test`; `just network-test` runs it with the
//! credentials from .env and a real character in `ZESQUAD_TEST_CHARACTER` (`Name-Surname`), in
//! `ZESQUAD_TEST_REGION` (default `eu`). Real character names stay out of the repo, so the
//! character comes from the environment.

use zesquad::armory::{Client, Config, Lookup};
use zesquad_core::armory::{Documents, Endpoint, Profile};
use zesquad_core::auctions;
use zesquad_core::{CharacterName, Region};

fn client() -> Client {
    let _ = dotenvy::dotenv();
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name} (or put it in .env)"))
    };
    let config = Config::new(
        var("ZESQUAD_BNET_CLIENT_ID"),
        var("ZESQUAD_BNET_CLIENT_SECRET"),
        None,
        zesquad::armory::TOKEN_URL,
    )
    .unwrap();
    Client::new(config).unwrap()
}

#[tokio::test]
#[ignore = "calls the real Battle.net API"]
async fn looks_up_a_real_forever_character() {
    let _ = dotenvy::dotenv();
    let name: CharacterName = std::env::var("ZESQUAD_TEST_CHARACTER")
        .expect("set ZESQUAD_TEST_CHARACTER=Name-Surname to a Forever character")
        .parse()
        .unwrap();
    let region: Region = std::env::var("ZESQUAD_TEST_REGION")
        .map_or(Ok(Region::Eu), |r| r.parse())
        .unwrap();
    let client = client();
    let mut docs = Documents::default();
    for endpoint in [Endpoint::Summary, Endpoint::Media, Endpoint::Equipment] {
        match client
            .character(region, &name.key(), endpoint)
            .await
            .unwrap()
        {
            Lookup::Found(doc) => docs.set(endpoint, doc),
            Lookup::NotFound => panic!(
                "{} not found on {} in {region}",
                endpoint.as_str(),
                name.key()
            ),
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
        .character(Region::Eu, "nobody-xqzvjw", Endpoint::Summary)
        .await
        .unwrap();
    assert!(matches!(lookup, Lookup::NotFound));
}

#[tokio::test]
#[ignore = "calls the real Battle.net API"]
async fn reads_real_auction_houses_in_eu_and_us() {
    let client = client();
    for region in auctions::REGIONS {
        let items = client.auctions(region, 110, false).await.unwrap();
        let commodities = client.auctions(region, 110, true).await.unwrap();
        let snapshot = auctions::snapshot(&[&items, &commodities]).unwrap();
        // A busy house: thousands of items (about 7,000 in November 2026).
        assert!(
            snapshot.items.len() > 1000,
            "{region}: {}",
            snapshot.items.len()
        );
    }
}

/// The followed houses are hard-coded; this notices if Blizzard renumbers them.
#[tokio::test]
#[ignore = "calls the real Battle.net API"]
async fn the_followed_auction_houses_still_exist() {
    let client = client();
    for region in auctions::REGIONS {
        let path = format!("/data/wow/ruleset/{}/auctions/index", auctions::RULESET);
        let (status, body) = client
            .raw(region, &path, &region.dynamic_namespace())
            .await
            .unwrap();
        assert_eq!(status, 200, "{region}: {body}");
        let index: serde_json::Value = serde_json::from_str(&body).unwrap();
        let houses = index["auctions"].as_array().unwrap();
        for (id, faction, _) in auctions::HOUSES {
            let name = houses
                .iter()
                .find(|h| h["id"] == id)
                .and_then(|h| h["name"].as_str())
                .unwrap_or_else(|| panic!("{region}: no house {id} in {body}"));
            let expected = match faction {
                auctions::Faction::Alliance => "Alliance",
                auctions::Faction::Horde => "Horde",
            };
            assert!(name.contains(expected), "{region}: house {id} is {name:?}");
        }
    }
}
