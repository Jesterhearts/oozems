use std::path::Path;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterStats;
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
    job_id: u32,
    hp: u32,
    max_hp: u32,
    mp: u32,
    max_mp: u32,
    experience: u64,
    experience_required: u64,
    fame: i32,
    ability_points: u32,
    strength: u32,
    dexterity: u32,
    intelligence: u32,
    luck: u32,
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
    job_id: Option<u32>,
    hp: Option<u32>,
    max_hp: Option<u32>,
    mp: Option<u32>,
    max_mp: Option<u32>,
    experience: Option<u64>,
    experience_required: Option<u64>,
    fame: Option<i32>,
    ability_points: Option<u32>,
    strength: Option<u32>,
    dexterity: Option<u32>,
    intelligence: Option<u32>,
    luck: Option<u32>,
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
            DEFINE FIELD IF NOT EXISTS job_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS hp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS max_hp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS mp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS max_mp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS experience ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS experience_required ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS fame ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS ability_points ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS strength ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS dexterity ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS intelligence ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS luck ON TABLE player TYPE option<int>;
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
        stats: Some(starter_character_stats()),
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
        stats: current.stats,
    }
}

fn player_from_record(record: PlayerRecord) -> PlayerState {
    let appearance = appearance_from_record(&record);
    let stats = stats_from_record(&record);
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
        stats: Some(stats),
    }
}

fn stats_from_record(record: &PlayerRecord) -> CharacterStats {
    let defaults = starter_character_stats();
    let max_hp = record.max_hp.unwrap_or(defaults.max_hp).max(1);
    let max_mp = record.max_mp.unwrap_or(defaults.max_mp).max(1);
    let experience_required = record
        .experience_required
        .unwrap_or(defaults.experience_required)
        .max(1);
    CharacterStats {
        job_id: record.job_id.unwrap_or(defaults.job_id),
        hp: record.hp.unwrap_or(defaults.hp).min(max_hp),
        max_hp,
        mp: record.mp.unwrap_or(defaults.mp).min(max_mp),
        max_mp,
        experience: record
            .experience
            .unwrap_or(defaults.experience)
            .min(experience_required),
        fame: record.fame.unwrap_or(defaults.fame),
        ability_points: record.ability_points.unwrap_or(defaults.ability_points),
        strength: record.strength.unwrap_or(defaults.strength),
        dexterity: record.dexterity.unwrap_or(defaults.dexterity),
        intelligence: record.intelligence.unwrap_or(defaults.intelligence),
        luck: record.luck.unwrap_or(defaults.luck),
        experience_required,
    }
}

fn starter_character_stats() -> CharacterStats {
    CharacterStats {
        job_id: 0,
        hp: 50,
        max_hp: 50,
        mp: 5,
        max_mp: 5,
        experience: 0,
        experience_required: 15,
        fame: 0,
        ability_points: 0,
        strength: 12,
        dexterity: 5,
        intelligence: 4,
        luck: 4,
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
        let stats = player.stats.unwrap_or_else(starter_character_stats);
        Self {
            player_id: player.id.clone(),
            name: player.name.clone(),
            level: player.level,
            job_id: stats.job_id,
            hp: stats.hp,
            max_hp: stats.max_hp,
            mp: stats.mp,
            max_mp: stats.max_mp,
            experience: stats.experience,
            experience_required: stats.experience_required,
            fame: stats.fame,
            ability_points: stats.ability_points,
            strength: stats.strength,
            dexterity: stats.dexterity,
            intelligence: stats.intelligence,
            luck: stats.luck,
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
    use surrealdb::types::SurrealValue;

    use super::CharacterName;
    use super::PlayerId;
    use super::apply_player_movement;
    use super::create_player;
    use super::load_player;
    use super::open_surreal_kv;
    use super::save_player;
    use super::starter_character_stats;

    #[derive(SurrealValue)]
    struct LegacyPlayerData {
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

    #[test]
    fn movement_is_bounded_and_does_not_change_authoritative_fields() {
        let current = PlayerState {
            id: "local".to_owned(),
            name: "Newcomer".to_owned(),
            level: 7,
            map_id: 1,
            position: Some(Vec2 { x: 5.0, y: 5.0 }),
            appearance: Some(appearance()),
            stats: Some(starter_character_stats()),
        };
        let requested = PlayerState {
            id: "local".to_owned(),
            name: "Changed".to_owned(),
            level: 99,
            map_id: 2,
            position: Some(Vec2 { x: -10.0, y: 900.0 }),
            appearance: None,
            stats: None,
        };

        let result = apply_player_movement(current, &requested, 800, 600);

        assert_eq!(result.name, "Newcomer");
        assert_eq!(result.level, 7);
        assert_eq!(result.map_id, 2);
        assert_eq!(result.position, Some(Vec2 { x: 0.0, y: 600.0 }));
        assert_eq!(result.appearance, Some(appearance()));
        assert_eq!(result.stats, Some(starter_character_stats()));
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

    #[tokio::test]
    async fn legacy_player_records_receive_starter_stats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("legacy-test").expect("valid player ID");
        let legacy = LegacyPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "Legacy".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            gender: Some(CharacterGender::Male as i32),
            skin_id: Some(2_000),
            face_id: Some(20_000),
            hair_id: Some(30_000),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(legacy)
            .await
            .expect("insert legacy player");

        let loaded = load_player(&database, &player_id)
            .await
            .expect("load legacy player")
            .expect("legacy player exists");

        assert_eq!(loaded.stats, Some(starter_character_stats()));
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
