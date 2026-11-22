//! Zesquad's pure logic: validating what people type (character names, tags, squad names) and
//! turning Battle.net responses into what the pages show. Nothing here does I/O.

pub mod account;
pub mod archive;
pub mod armory;
pub mod auctions;
pub mod discord;
pub mod link;
pub mod name;
pub mod recent;
pub mod redirect;
pub mod refresh;
pub mod region;
pub mod roster;
pub mod sparkline;
pub mod squad_id;
pub mod tags;
pub mod text;

pub use name::{CharacterName, NameError};
pub use region::{Region, RegionError};
pub use text::{SquadName, Tag, TextError, parse_tags};
