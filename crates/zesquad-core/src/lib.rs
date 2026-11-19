//! Zesquad's pure logic: validating what people type (character names, tags, squad names) and
//! turning Battle.net responses into what the pages show. Nothing here does I/O.

pub mod armory;
pub mod name;
pub mod redirect;
pub mod refresh;
pub mod region;
pub mod squad_id;
pub mod tags;
pub mod text;

pub use name::{CharacterName, NameError};
pub use region::{Region, RegionError};
pub use text::{SquadName, Tag, TextError, parse_tags};
