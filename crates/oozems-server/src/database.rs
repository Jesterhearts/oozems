use std::path::Path;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::SurrealKv;
use surrealdb::types::RecordId;
use surrealdb::types::SurrealValue;
use thiserror::Error;

pub const STARTER_MAP_ID: u32 = 100_000_000;

pub type Database = Surreal<Db>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerId(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterName(String);

#[derive(Debug, Error)]
pub enum PlayerIdError {
    #[error("player ID must contain 1 to 32 ASCII letters, digits, underscores, or hyphens")]
    Invalid,
}

#[derive(Debug, Error)]
pub enum CharacterNameError {
    #[error("character name must contain 3 to 12 ASCII letters, digits, or underscores")]
    Invalid,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerData {
    player_id: String,
    name: String,
    level: u32,
    map_id: u32,
    x: f64,
    y: f64,
    gender: Option<i32>,
    skin_id: Option<u32>,
    face_id: Option<u32>,
    hair_id: Option<u32>,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerRecord {
    id: RecordId,
    player_id: String,
    name: String,
    level: u32,
    map_id: u32,
    x: f64,
    y: f64,
    gender: Option<i32>,
    skin_id: Option<u32>,
    face_id: Option<u32>,
    hair_id: Option<u32>,
}

impl PlayerId {
    pub fn parse(value: &str) -> Result<Self, PlayerIdError> {
        let is_valid = !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));

        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(PlayerIdError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CharacterName {
    pub fn parse(value: &str) -> Result<Self, CharacterNameError> {
        let is_valid = (3..=12).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(CharacterNameError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub async fn open_surreal_kv(path: &Path) -> surrealdb::Result<Database> {
    let database = Surreal::new::<SurrealKv>(path).await?;
    database.use_ns("oozems").use_db("game").await?;
    initialize_schema(&database).await?;
    Ok(database)
}

async fn initialize_schema(database: &Database) -> surrealdb::Result<()> {
    database
        .query(
            r#"
            DEFINE TABLE IF NOT EXISTS player SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS player_id ON TABLE player TYPE string;
            DEFINE FIELD IF NOT EXISTS name ON TABLE player TYPE string;
            DEFINE FIELD IF NOT EXISTS level ON TABLE player TYPE int;
            DEFINE FIELD IF NOT EXISTS map_id ON TABLE player TYPE int;
            DEFINE FIELD IF NOT EXISTS x ON TABLE player TYPE float;
            DEFINE FIELD IF NOT EXISTS y ON TABLE player TYPE float;
            DEFINE FIELD IF NOT EXISTS gender ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS skin_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS face_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS hair_id ON TABLE player TYPE option<int>;
            "#,
        )
        .await?
        .check()?;
    Ok(())
}

pub async fn load_player(
    database: &Database,
    player_id: &PlayerId,
) -> surrealdb::Result<Option<PlayerState>> {
    let record: Option<PlayerRecord> = database.select(("player", player_id.as_str())).await?;
    Ok(record.map(player_from_record))
}

pub async fn create_player(
    database: &Database,
    player_id: &PlayerId,
    name: &CharacterName,
    appearance: CharacterAppearance,
    position: Vec2,
) -> surrealdb::Result<PlayerState> {
    let player = PlayerState {
        id: player_id.as_str().to_owned(),
        name: name.as_str().to_owned(),
        level: 1,
        map_id: STARTER_MAP_ID,
        position: Some(position),
        appearance: Some(appearance),
    };
    save_player(database, &player).await
}

pub async fn save_player(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<PlayerState> {
    let data = PlayerData::from(player);
    let record: Option<PlayerRecord> = database
        .upsert(("player", player.id.as_str()))
        .content(data)
        .await?;

    record
        .map(player_from_record)
        .ok_or_else(|| surrealdb::Error::internal("player upsert returned no record".to_owned()))
}

pub fn apply_player_movement(
    current: PlayerState,
    requested: &PlayerState,
    map_width: u32,
    map_height: u32,
) -> PlayerState {
    let requested_position = requested.position.as_ref().cloned().unwrap_or_default();
    let position = Vec2 {
        x: requested_position.x.clamp(0.0, map_width as f32),
        y: requested_position.y.clamp(0.0, map_height as f32),
    };

    PlayerState {
        id: current.id,
        name: current.name,
        level: current.level,
        map_id: requested.map_id,
        position: Some(position),
        appearance: current.appearance,
    }
}

fn player_from_record(record: PlayerRecord) -> PlayerState {
    let appearance = appearance_from_record(&record);
    let _record_id = record.id;
    PlayerState {
        id: record.player_id,
        name: record.name,
        level: record.level,
        map_id: record.map_id,
        position: Some(Vec2 {
            x: record.x as f32,
            y: record.y as f32,
        }),
        appearance,
    }
}

fn appearance_from_record(record: &PlayerRecord) -> Option<CharacterAppearance> {
    Some(CharacterAppearance {
        gender: record.gender?,
        skin_id: record.skin_id?,
        face_id: record.face_id?,
        hair_id: record.hair_id?,
    })
}

impl From<&PlayerState> for PlayerData {
    fn from(player: &PlayerState) -> Self {
        let position = player.position.as_ref().cloned().unwrap_or_default();
        let appearance = player.appearance.as_ref();
        Self {
            player_id: player.id.clone(),
            name: player.name.clone(),
            level: player.level,
            map_id: player.map_id,
            x: f64::from(position.x),
            y: f64::from(position.y),
            gender: appearance.map(|value| value.gender),
            skin_id: appearance.map(|value| value.skin_id),
            face_id: appearance.map(|value| value.face_id),
            hair_id: appearance.map(|value| value.hair_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::CharacterName;
    use super::PlayerId;
    use super::apply_player_movement;
    use super::create_player;
    use super::load_player;
    use super::open_surreal_kv;
    use super::save_player;

    #[test]
    fn movement_is_bounded_and_does_not_change_authoritative_fields() {
        let current = PlayerState {
            id: "local".to_owned(),
            name: "Newcomer".to_owned(),
            level: 7,
            map_id: 1,
            position: Some(Vec2 { x: 5.0, y: 5.0 }),
            appearance: Some(appearance()),
        };
        let requested = PlayerState {
            id: "local".to_owned(),
            name: "Changed".to_owned(),
            level: 99,
            map_id: 2,
            position: Some(Vec2 { x: -10.0, y: 900.0 }),
            appearance: None,
        };

        let result = apply_player_movement(current, &requested, 800, 600);

        assert_eq!(result.name, "Newcomer");
        assert_eq!(result.level, 7);
        assert_eq!(result.map_id, 2);
        assert_eq!(result.position, Some(Vec2 { x: 0.0, y: 600.0 }));
        assert_eq!(result.appearance, Some(appearance()));
    }

    #[tokio::test]
    async fn player_round_trip_uses_surreal_kv() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("database-test").expect("valid player ID");
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load absent player"),
            None
        );
        let name = CharacterName::parse("Mina").expect("valid name");
        let mut player = create_player(
            &database,
            &player_id,
            &name,
            appearance(),
            Vec2 { x: 160.0, y: 420.0 },
        )
        .await
        .expect("create player");
        player.position = Some(Vec2 { x: 321.0, y: 456.0 });

        save_player(&database, &player).await.expect("save player");
        let loaded = load_player(&database, &player_id)
            .await
            .expect("load player")
            .expect("saved player exists");

        assert_eq!(loaded, player);
    }

    fn appearance() -> CharacterAppearance {
        CharacterAppearance {
            gender: CharacterGender::Female as i32,
            skin_id: 2_000,
            face_id: 21_000,
            hair_id: 31_000,
        }
    }
}
