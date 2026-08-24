use std::path::Path;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::SurrealKv;
use surrealdb::types::Value;
use thiserror::Error;

use self::player_record::PlayerRecord;

mod player_record;

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

            DEFINE FIELD OVERWRITE revision ON player TYPE int;
            DEFINE FIELD OVERWRITE name ON player TYPE string;
            DEFINE FIELD OVERWRITE level ON player TYPE int;
            DEFINE FIELD OVERWRITE map_id ON player TYPE int;
            DEFINE FIELD OVERWRITE position ON player TYPE { x: float, y: float };
            DEFINE FIELD OVERWRITE appearance ON player TYPE {
                gender: int,
                skin_id: int,
                face_id: int,
                hair_id: int
            };
            DEFINE FIELD OVERWRITE stats ON player TYPE {
                job_id: int,
                hp: int,
                max_hp: int,
                mp: int,
                max_mp: int,
                experience: int,
                experience_required: int,
                fame: int,
                ability_points: int,
                strength: int,
                dexterity: int,
                intelligence: int,
                luck: int
            };
            DEFINE FIELD OVERWRITE inventory ON player TYPE {
                capacity: int,
                stacks: array<{
                    item_id: int,
                    quantity: int,
                    expires_at_unix_ms: int
                }>,
                equipment: array<{
                    slot: int,
                    item_id: int,
                    expires_at_unix_ms: int
                }>
            };
            DEFINE FIELD OVERWRITE key_bindings ON player TYPE array<{
                code: string,
                action: int,
                skill_id: int
            }>;
            DEFINE FIELD OVERWRITE skill_points ON player TYPE int;
            DEFINE FIELD OVERWRITE learned_skills ON player TYPE array<{
                skill_id: int,
                level: int,
                master_level: int
            }>;
            DEFINE FIELD OVERWRITE mesos ON player TYPE int;
            DEFINE FIELD OVERWRITE quests ON player TYPE array<{
                quest_id: int,
                status: int,
                mob_progress: array<{ mob_id: int, count: int }>,
                accepted_at_unix_ms: int,
                completed_at_unix_ms: int,
                dialogue_step: int,
                completion_quiz_passed: bool
            }>;
            DEFINE FIELD OVERWRITE quest_records ON player TYPE array<{
                quest_id: int,
                entries: array<{ index: int, value: string }>
            }>;
            DEFINE FIELD OVERWRITE monster_book_cards ON player TYPE array<{
                card_item_id: int,
                count: int
            }>;
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
    select_player_record(database, player_id)
        .await?
        .map(|record| player_record::player_from_record(player_id, record))
        .transpose()
}

pub async fn create_player(
    database: &Database,
    player_id: &PlayerId,
    name: &CharacterName,
    appearance: CharacterAppearance,
    map_id: u32,
    position: Vec2,
    experience_required: u64,
    initial_skill_points: u32,
) -> surrealdb::Result<PlayerState> {
    let mut stats = starter_character_stats();
    stats.experience_required = experience_required;
    let player = PlayerState {
        id: player_id.as_str().to_owned(),
        name: name.as_str().to_owned(),
        level: 1,
        map_id,
        position: Some(position),
        appearance: Some(appearance),
        stats: Some(stats),
        inventory: Some(crate::items::starter_inventory()),
        key_bindings: crate::keymap::default_bindings(),
        skill_points: initial_skill_points,
        learned_skills: Vec::new(),
        mesos: 0,
        quests: Vec::new(),
        revision: 0,
        quest_records: Vec::new(),
        monster_book_cards: Vec::new(),
    };
    save_player(database, &player).await
}

pub async fn save_player(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<PlayerState> {
    let player_id = PlayerId::parse(&player.id).map_err(player_record::invalid_player_error)?;
    let current_revision = select_player_record(database, &player_id)
        .await?
        .map(player_record::record_revision)
        .transpose()?
        .unwrap_or_default();
    let revision = next_player_revision(player.revision, current_revision)?;
    let record = player_record::record_from_player(player, revision)?;
    let value: Option<Value> = database
        .upsert(("player", player.id.as_str()))
        .content(record)
        .await?;
    let record = value
        .map(player_record::decode_player_record)
        .transpose()?
        .ok_or_else(|| surrealdb::Error::internal("player upsert returned no record".to_owned()))?;
    player_record::player_from_record(&player_id, record)
}

fn next_player_revision(
    input_revision: u64,
    saved_revision: u64,
) -> surrealdb::Result<u64> {
    input_revision
        .max(saved_revision)
        .checked_add(1)
        .filter(|revision| i64::try_from(*revision).is_ok())
        .ok_or_else(|| surrealdb::Error::internal("player revision overflow".to_owned()))
}

pub async fn save_player_position(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<()> {
    let data = player_record::position_data_from_player(player)?;
    let _: Option<Value> = database
        .update(("player", player.id.as_str()))
        .merge(data)
        .await?;
    Ok(())
}

pub fn apply_player_preferences(
    mut current: PlayerState,
    requested: &PlayerState,
) -> PlayerState {
    current.key_bindings = requested.key_bindings.clone();
    current
}

async fn select_player_record(
    database: &Database,
    player_id: &PlayerId,
) -> surrealdb::Result<Option<PlayerRecord>> {
    let value: Option<Value> = database.select(("player", player_id.as_str())).await?;
    value.map(player_record::decode_player_record).transpose()
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::LearnedSkill;
    use oozems_proto::v1::MonsterBookCard;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestMobProgress;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::Vec2;
    use surrealdb::types::Value;

    use super::CharacterName;
    use super::PlayerId;
    use super::apply_player_preferences;
    use super::create_player;
    use super::load_player;
    use super::open_surreal_kv;
    use super::save_player;
    use super::save_player_position;
    use super::starter_character_stats;

    #[test]
    fn preference_updates_do_not_change_authoritative_fields() {
        let current = test_player_state();
        let requested = PlayerState {
            id: "local".to_owned(),
            name: "Changed".to_owned(),
            level: 99,
            map_id: 2,
            position: None,
            appearance: None,
            stats: None,
            inventory: None,
            key_bindings: Vec::new(),
            skill_points: 99,
            learned_skills: vec![LearnedSkill {
                skill_id: 1_000,
                level: 2,
                master_level: 0,
            }],
            mesos: 99_999,
            quests: Vec::new(),
            revision: u64::MAX,
            quest_records: Vec::new(),
            monster_book_cards: Vec::new(),
        };

        let result = apply_player_preferences(current.clone(), &requested);

        assert!(result.key_bindings.is_empty());
        assert_eq!(result.name, current.name);
        assert_eq!(result.position, current.position);
        assert_eq!(result.stats, current.stats);
        assert_eq!(result.inventory, current.inventory);
        assert_eq!(result.skill_points, current.skill_points);
        assert_eq!(result.revision, current.revision);
    }

    #[tokio::test]
    async fn player_round_trip_uses_the_current_nested_schema() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        super::initialize_schema(&database)
            .await
            .expect("schema initialization is idempotent");
        let player_id = PlayerId::parse("database-test").expect("valid player ID");
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load absent player"),
            None
        );
        let mut player = create_test_player(&database, &player_id).await;
        player.position = Some(Vec2 { x: 321.0, y: 456.0 });
        player.skill_points = 2;
        player.learned_skills = vec![
            LearnedSkill {
                skill_id: 2_321_003,
                level: 0,
                master_level: 10,
            },
            LearnedSkill {
                skill_id: 1_000,
                level: 1,
                master_level: 3,
            },
        ];
        player.mesos = 1_234;
        player.monster_book_cards = vec![
            MonsterBookCard {
                card_item_id: 2_380_001,
                count: 5,
            },
            MonsterBookCard {
                card_item_id: 2_380_000,
                count: 2,
            },
        ];
        player.quests.push(PlayerQuest {
            quest_id: 1_009,
            status: QuestStatus::Started as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: 100_100,
                count: 3,
            }],
            accepted_at_unix_ms: 123_456,
            completed_at_unix_ms: 0,
            dialogue_step: 2,
            completion_quiz_passed: false,
        });
        player.key_bindings.push(KeyBinding {
            code: "F1".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 1_000,
        });
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks
            .push(InventoryItemStack {
                item_id: 2_000_000,
                quantity: 3,
                expires_at_unix_ms: 1_900_000_000_000,
            });
        player.inventory.as_mut().expect("inventory").equipment[0].expires_at_unix_ms =
            1_900_000_000_123;
        crate::quest_records::set(&mut player, 2_236, 42, "sparse".to_owned())
            .expect("sparse quest record");
        crate::quest_records::set(&mut player, 2_236, 0, "000000".to_owned())
            .expect("primary quest record");

        player = save_player(&database, &player).await.expect("save player");
        let loaded = load_player(&database, &player_id)
            .await
            .expect("load player")
            .expect("saved player exists");

        assert_eq!(player.revision, 2);
        assert_eq!(loaded, player);
        assert_eq!(loaded.learned_skills[0].skill_id, 1_000);
        assert_eq!(loaded.monster_book_cards[0].card_item_id, 2_380_000);
        assert_eq!(loaded.quest_records[0].entries[0].index, 0);
    }

    #[tokio::test]
    async fn position_updates_remain_partial_and_keep_revision() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("position-test").expect("valid player ID");
        let mut player = create_test_player(&database, &player_id).await;
        let original_stats = player.stats;
        let original_revision = player.revision;
        player.position = Some(Vec2 { x: 300.0, y: 250.0 });
        player.stats.as_mut().expect("stats").hp = 1;

        save_player_position(&database, &player)
            .await
            .expect("save position");
        let loaded = load_player(&database, &player_id)
            .await
            .expect("load player")
            .expect("saved player exists");

        assert_eq!(loaded.position, Some(Vec2 { x: 300.0, y: 250.0 }));
        assert_eq!(loaded.stats, original_stats);
        assert_eq!(loaded.revision, original_revision);
    }

    #[tokio::test]
    async fn full_saves_increment_past_the_input_and_current_revisions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("revision-test").expect("valid player ID");
        let created = create_test_player(&database, &player_id).await;

        let saved = save_player(&database, &created).await.expect("save player");
        let mut ahead = created.clone();
        ahead.revision = 7;
        let saved_ahead = save_player(&database, &ahead)
            .await
            .expect("save player with an ahead revision");
        let saved_again = save_player(&database, &created)
            .await
            .expect("save stale player");

        assert_eq!(created.revision, 1);
        assert_eq!(saved.revision, 2);
        assert_eq!(saved_ahead.revision, 8);
        assert_eq!(saved_again.revision, 9);
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            9
        );
    }

    #[test]
    fn revision_increment_reports_overflow() {
        let error =
            super::next_player_revision(i64::MAX as u64, 0).expect_err("revision must overflow");

        assert!(error.to_string().contains("player revision overflow"));
    }

    #[tokio::test]
    async fn save_rejects_missing_nonfinite_and_invalid_current_data() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("invalid-save").expect("valid player ID");
        let player = create_test_player(&database, &player_id).await;

        for (expected, missing) in [
            (
                "position is required",
                PlayerState {
                    position: None,
                    ..player.clone()
                },
            ),
            (
                "appearance is required",
                PlayerState {
                    appearance: None,
                    ..player.clone()
                },
            ),
            (
                "stats are required",
                PlayerState {
                    stats: None,
                    ..player.clone()
                },
            ),
            (
                "inventory is required",
                PlayerState {
                    inventory: None,
                    ..player.clone()
                },
            ),
        ] {
            let error = save_player(&database, &missing)
                .await
                .expect_err("missing required component must fail");
            assert!(error.to_string().contains(expected));
        }

        let mut nonfinite = player.clone();
        nonfinite.position.as_mut().expect("position").x = f32::NAN;
        let error = save_player(&database, &nonfinite)
            .await
            .expect_err("nonfinite position must fail");
        assert!(error.to_string().contains("position is not finite"));

        let mut invalid_collection = player.clone();
        invalid_collection.learned_skills = vec![
            LearnedSkill {
                skill_id: 1_000,
                level: 1,
                master_level: 0,
            },
            LearnedSkill {
                skill_id: 1_000,
                level: 2,
                master_level: 0,
            },
        ];
        let error = save_player(&database, &invalid_collection)
            .await
            .expect_err("duplicate learned skills must fail");
        assert!(error.to_string().contains("appears more than once"));

        let mut invalid_inventory = player;
        invalid_inventory
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks[0]
            .quantity = 0;
        let error = save_player(&database, &invalid_inventory)
            .await
            .expect_err("zero-quantity inventory stack must fail");
        assert!(
            error
                .to_string()
                .contains("positive item IDs and quantities")
        );
    }

    #[tokio::test]
    async fn load_reports_semantically_invalid_current_records_as_corrupt() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("invalid-load").expect("valid player ID");
        create_test_player(&database, &player_id).await;
        let mut record = super::select_player_record(&database, &player_id)
            .await
            .expect("select current record")
            .expect("current record");
        super::player_record::set_first_inventory_stack_quantity(&mut record, 0);
        let _: Option<Value> = database
            .upsert(("player", player_id.as_str()))
            .content(record)
            .await
            .expect("write schema-valid corrupt record");

        let error = load_player(&database, &player_id)
            .await
            .expect_err("corrupt record must fail");

        assert!(error.to_string().contains("persisted player is corrupt"));
        assert!(
            error
                .to_string()
                .contains("positive item IDs and quantities")
        );
    }

    async fn create_test_player(
        database: &super::Database,
        player_id: &PlayerId,
    ) -> PlayerState {
        create_player(
            database,
            player_id,
            &CharacterName::parse("Mina").expect("valid name"),
            appearance(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
        )
        .await
        .expect("create player")
    }

    fn test_player_state() -> PlayerState {
        PlayerState {
            id: "local".to_owned(),
            name: "Newcomer".to_owned(),
            level: 7,
            map_id: 1,
            position: Some(Vec2 { x: 5.0, y: 5.0 }),
            appearance: Some(appearance()),
            stats: Some(starter_character_stats()),
            inventory: Some(crate::items::starter_inventory()),
            key_bindings: crate::keymap::default_bindings(),
            skill_points: 3,
            learned_skills: Vec::new(),
            mesos: 500,
            quests: Vec::new(),
            revision: 10,
            quest_records: Vec::new(),
            monster_book_cards: Vec::new(),
        }
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
