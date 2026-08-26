use std::path::Path;
use std::time::Duration;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::SurrealKv;
use surrealdb::types::Value;
use thiserror::Error;

use self::player_record::PlayerRecord;
use crate::player_lock::PlayerGuard;

mod player_record;

pub type Database = Surreal<Db>;

// Embedded transactions can conflict under legitimate concurrent writes. Bound
// both retry count and delay so a save cannot hold its per-player request
// forever.
const PLAYER_SAVE_MAX_ATTEMPTS: usize = 8;
const PLAYER_SAVE_MAX_BACKOFF: Duration = Duration::from_millis(16);
const SURREALDB_TRANSACTION_CONFLICT_PREFIX: &str = "Transaction conflict:";

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

pub async fn open_surreal_kv(
    path: &Path,
    initial_cash_points: u64,
) -> surrealdb::Result<Database> {
    let database = Surreal::new::<SurrealKv>(path).await?;
    database.use_ns("oozems").use_db("game").await?;
    initialize_schema(&database, initial_cash_points).await?;
    Ok(database)
}

async fn initialize_schema(
    database: &Database,
    initial_cash_points: u64,
) -> surrealdb::Result<()> {
    let initial_cash_points = i64::try_from(initial_cash_points).map_err(|_| {
        surrealdb::Error::internal("initial cash points exceed the persisted range".to_owned())
    })?;
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
            DEFINE FIELD OVERWRITE cash_points ON player TYPE int;
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
    database
        .query("UPDATE player SET cash_points = $initial_cash_points WHERE cash_points = NONE")
        .bind(("initial_cash_points", initial_cash_points))
        .await?
        .check()?;
    database
        .query(
            r#"
            UPDATE player SET
                stats.ability_points = 9,
                stats.strength = 4,
                stats.dexterity = 4
            WHERE level = 1
                AND stats.ability_points = 0
                AND stats.strength = 12
                AND stats.dexterity = 5
                AND stats.intelligence = 4
                AND stats.luck = 4;
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
    guard: &PlayerGuard,
    player_id: &PlayerId,
    name: &CharacterName,
    appearance: CharacterAppearance,
    inventory: InventoryState,
    map_id: u32,
    position: Vec2,
    experience_required: u64,
    initial_skill_points: u32,
    initial_cash_points: u64,
) -> surrealdb::Result<PlayerState> {
    require_player_guard(guard, player_id)?;
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
        inventory: Some(inventory),
        key_bindings: crate::keymap::default_bindings(),
        skill_points: initial_skill_points,
        learned_skills: Vec::new(),
        mesos: 0,
        quests: Vec::new(),
        revision: 0,
        quest_records: Vec::new(),
        monster_book_cards: Vec::new(),
        cash_points: initial_cash_points,
    };
    save_player(database, guard, &player).await
}

pub async fn save_player(
    database: &Database,
    guard: &PlayerGuard,
    player: &PlayerState,
) -> surrealdb::Result<PlayerState> {
    let player_id = PlayerId::parse(&player.id).map_err(player_record::invalid_player_error)?;
    require_player_guard(guard, &player_id)?;
    for attempt in 1..=PLAYER_SAVE_MAX_ATTEMPTS {
        let transaction = database.clone().begin().await?;
        let result = async {
            let value: Option<Value> = transaction.select(("player", player_id.as_str())).await?;
            let current_revision = value
                .map(player_record::decode_player_record)
                .transpose()?
                .map(player_record::record_revision)
                .transpose()?
                .unwrap_or_default();
            let revision = next_player_revision(player.revision, current_revision)?;
            let record = player_record::record_from_player(player, revision)?;
            let value: Option<Value> = transaction
                .upsert(("player", player.id.as_str()))
                .content(record)
                .await?;
            let record = value
                .map(player_record::decode_player_record)
                .transpose()?
                .ok_or_else(|| {
                    surrealdb::Error::internal("player upsert returned no record".to_owned())
                })?;
            player_record::player_from_record(&player_id, record)
        }
        .await;
        let saved = match result {
            Ok(saved) => saved,
            Err(error) => {
                let retry = is_transaction_conflict(&error);
                if let Err(cancel_error) = transaction.cancel().await {
                    return Err(surrealdb::Error::internal(format!(
                        "player save failed: {error}; transaction cancellation failed: \
                         {cancel_error}"
                    )));
                }
                if retry {
                    retry_transaction_conflict(attempt, &error).await?;
                    continue;
                }
                return Err(error);
            }
        };
        match transaction.commit().await {
            Ok(_) => return Ok(saved),
            Err(error) if is_transaction_conflict(&error) => {
                retry_transaction_conflict(attempt, &error).await?;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the finite player save attempt loop returns or exhausts")
}

fn is_transaction_conflict(error: &surrealdb::Error) -> bool {
    is_transaction_conflict_message(error.message())
}

fn is_transaction_conflict_message(message: &str) -> bool {
    // SurrealDB 3.2.4 erases the structured QueryError for embedded commit
    // failures.
    message.starts_with(SURREALDB_TRANSACTION_CONFLICT_PREFIX)
}

async fn retry_transaction_conflict(
    attempt: usize,
    error: &surrealdb::Error,
) -> surrealdb::Result<()> {
    let Some(delay) = player_save_retry_delay(attempt) else {
        return Err(surrealdb::Error::internal(format!(
            "player save exhausted {PLAYER_SAVE_MAX_ATTEMPTS} transaction-conflict attempts: \
             {error}"
        )));
    };
    tokio::time::sleep(delay).await;
    Ok(())
}

fn player_save_retry_delay(attempt: usize) -> Option<Duration> {
    if attempt >= PLAYER_SAVE_MAX_ATTEMPTS {
        return None;
    }
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let multiplier = 2_u32.checked_pow(exponent).unwrap_or(u32::MAX);
    Some(Duration::from_millis(u64::from(multiplier)).min(PLAYER_SAVE_MAX_BACKOFF))
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
    guard: &PlayerGuard,
    player: &PlayerState,
) -> surrealdb::Result<()> {
    let player_id = PlayerId::parse(&player.id).map_err(player_record::invalid_player_error)?;
    require_player_guard(guard, &player_id)?;
    let data = player_record::position_data_from_player(player)?;
    let _: Option<Value> = database
        .update(("player", player.id.as_str()))
        .merge(data)
        .await?;
    Ok(())
}

fn require_player_guard(
    guard: &PlayerGuard,
    player_id: &PlayerId,
) -> surrealdb::Result<()> {
    if guard.holds_player(player_id.as_str()) {
        Ok(())
    } else {
        Err(surrealdb::Error::internal(format!(
            "player guard does not hold player {}",
            player_id.as_str()
        )))
    }
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
        ability_points: 9,
        strength: 4,
        dexterity: 4,
        intelligence: 4,
        luck: 4,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
    use tokio::sync::Barrier;

    use super::CharacterName;
    use super::PlayerId;
    use super::apply_player_preferences;
    use super::create_player;
    use super::load_player;
    use super::open_surreal_kv;
    use super::save_player;
    use super::save_player_position;
    use super::starter_character_stats;
    use crate::player_lock::PlayerGuard;
    use crate::player_lock::PlayerLocks;
    use crate::player_lock::acquire_player;

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
            cash_points: 99_999,
        };

        let result = apply_player_preferences(current.clone(), &requested);

        assert!(result.key_bindings.is_empty());
        assert_eq!(result.name, current.name);
        assert_eq!(result.position, current.position);
        assert_eq!(result.stats, current.stats);
        assert_eq!(result.inventory, current.inventory);
        assert_eq!(result.skill_points, current.skill_points);
        assert_eq!(result.cash_points, current.cash_points);
        assert_eq!(result.revision, current.revision);
    }

    #[test]
    fn surrealdb_conflict_detection_is_an_exact_prefix_compatibility_boundary() {
        assert!(super::is_transaction_conflict_message(
            "Transaction conflict: record changed"
        ));
        assert!(!super::is_transaction_conflict_message(
            "transaction conflict: record changed"
        ));
        assert!(!super::is_transaction_conflict_message(
            "wrapped Transaction conflict: record changed"
        ));
    }

    #[test]
    fn player_save_retry_schedule_is_finite_and_bounded() {
        let delays = (1..=super::PLAYER_SAVE_MAX_ATTEMPTS)
            .map(super::player_save_retry_delay)
            .collect::<Vec<_>>();

        assert_eq!(
            delays,
            vec![
                Some(std::time::Duration::from_millis(1)),
                Some(std::time::Duration::from_millis(2)),
                Some(std::time::Duration::from_millis(4)),
                Some(std::time::Duration::from_millis(8)),
                Some(std::time::Duration::from_millis(16)),
                Some(std::time::Duration::from_millis(16)),
                Some(std::time::Duration::from_millis(16)),
                None,
            ]
        );
    }

    #[tokio::test]
    async fn exhausted_player_save_conflicts_return_a_clear_error() {
        let conflict = surrealdb::Error::internal("Transaction conflict: test".to_owned());

        let error = super::retry_transaction_conflict(super::PLAYER_SAVE_MAX_ATTEMPTS, &conflict)
            .await
            .expect_err("the final conflict must exhaust retries");

        assert!(
            error
                .message()
                .contains("player save exhausted 8 transaction-conflict attempts")
        );
    }

    #[tokio::test]
    async fn player_round_trip_uses_the_current_nested_schema() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        super::initialize_schema(&database, 10_000)
            .await
            .expect("schema initialization is idempotent");
        let player_id = PlayerId::parse("database-test").expect("valid player ID");
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load absent player"),
            None
        );
        let guard = test_guard(&player_id).await;
        let mut player = create_test_player(&database, &guard, &player_id).await;
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
        player.cash_points = 9_876;
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

        player = save_player(&database, &guard, &player)
            .await
            .expect("save player");
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
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("position-test").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        let mut player = create_test_player(&database, &guard, &player_id).await;
        let original_stats = player.stats;
        let original_revision = player.revision;
        player.position = Some(Vec2 { x: 300.0, y: 250.0 });
        player.stats.as_mut().expect("stats").hp = 1;

        save_player_position(&database, &guard, &player)
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
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("revision-test").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        let created = create_test_player(&database, &guard, &player_id).await;

        let saved = save_player(&database, &guard, &created)
            .await
            .expect("save player");
        let mut ahead = created.clone();
        ahead.revision = 7;
        let saved_ahead = save_player(&database, &guard, &ahead)
            .await
            .expect("save player with an ahead revision");
        let saved_again = save_player(&database, &guard, &created)
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

    #[tokio::test]
    async fn concurrent_stale_saves_receive_monotonic_revisions() {
        const WRITER_COUNT: u64 = 8;

        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("concurrent-revision").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        let stale = create_test_player(&database, &guard, &player_id).await;
        drop(guard);
        let barrier = Arc::new(Barrier::new(WRITER_COUNT as usize));
        let mut writers = Vec::new();
        for mesos in 0..WRITER_COUNT {
            let database = database.clone();
            let barrier = barrier.clone();
            let mut player = stale.clone();
            player.mesos = mesos;
            writers.push(tokio::spawn(async move {
                let locks = PlayerLocks::default();
                let guard = acquire_player(&locks, &player.id)
                    .await
                    .expect("player guard");
                barrier.wait().await;
                save_player(&database, &guard, &player).await
            }));
        }

        let mut revisions = Vec::new();
        for writer in writers {
            let saved = writer
                .await
                .expect("writer task")
                .expect("save stale player");
            revisions.push(saved.revision);
        }
        revisions.sort_unstable();

        assert_eq!(revisions, (2..=WRITER_COUNT + 1).collect::<Vec<_>>());
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load player")
                .expect("saved player")
                .revision,
            WRITER_COUNT + 1
        );
    }

    #[tokio::test]
    async fn mutating_operations_reject_a_guard_for_another_player() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("guarded-player").expect("valid player ID");
        let other_id = PlayerId::parse("other-player").expect("valid player ID");
        let wrong_guard = test_guard(&other_id).await;

        let create_error = create_player(
            &database,
            &wrong_guard,
            &player_id,
            &CharacterName::parse("Mina").expect("valid name"),
            appearance(),
            crate::items::starter_inventory(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
            10_000,
        )
        .await
        .expect_err("another player's guard must not create a player");
        assert!(create_error.to_string().contains("guarded-player"));
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load absent player"),
            None
        );

        let guard = test_guard(&player_id).await;
        let saved = create_test_player(&database, &guard, &player_id).await;
        let mut changed = saved.clone();
        changed.mesos = 500;
        changed.position = Some(Vec2 { x: 300.0, y: 250.0 });

        save_player(&database, &wrong_guard, &changed)
            .await
            .expect_err("another player's guard must not save a player");
        save_player_position(&database, &wrong_guard, &changed)
            .await
            .expect_err("another player's guard must not save a position");

        let loaded = load_player(&database, &player_id)
            .await
            .expect("load player")
            .expect("saved player");
        assert_eq!(loaded, saved);
    }

    #[tokio::test]
    async fn schema_backfills_only_missing_cash_point_balances() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 100)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("cash-migration").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        create_test_player(&database, &guard, &player_id).await;

        super::initialize_schema(&database, 999)
            .await
            .expect("repeat schema initialization");
        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load current player")
                .expect("current player exists")
                .cash_points,
            10_000
        );

        database
            .query("REMOVE FIELD cash_points ON player; UPDATE player UNSET cash_points")
            .await
            .expect("remove legacy cash-point field")
            .check()
            .expect("write legacy player");
        super::initialize_schema(&database, 999)
            .await
            .expect("migrate legacy schema");

        assert_eq!(
            load_player(&database, &player_id)
                .await
                .expect("load migrated player")
                .expect("migrated player exists")
                .cash_points,
            999
        );
    }

    #[tokio::test]
    async fn schema_migrates_only_the_legacy_unallocated_starter_stats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 100)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("stats-migration").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        let mut player = create_test_player(&database, &guard, &player_id).await;
        let stats = player.stats.as_mut().expect("stats");
        stats.ability_points = 0;
        stats.strength = 12;
        stats.dexterity = 5;
        save_player(&database, &guard, &player)
            .await
            .expect("save legacy starter stats");

        super::initialize_schema(&database, 100)
            .await
            .expect("migrate legacy stats");

        let migrated = load_player(&database, &player_id)
            .await
            .expect("load migrated player")
            .expect("migrated player exists");
        let stats = migrated.stats.expect("stats");
        assert_eq!(stats.ability_points, 9);
        assert_eq!((stats.strength, stats.dexterity), (4, 4));
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
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("invalid-save").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        let player = create_test_player(&database, &guard, &player_id).await;

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
            let error = save_player(&database, &guard, &missing)
                .await
                .expect_err("missing required component must fail");
            assert!(error.to_string().contains(expected));
        }

        let mut nonfinite = player.clone();
        nonfinite.position.as_mut().expect("position").x = f32::NAN;
        let error = save_player(&database, &guard, &nonfinite)
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
        let error = save_player(&database, &guard, &invalid_collection)
            .await
            .expect_err("duplicate learned skills must fail");
        assert!(error.to_string().contains("appears more than once"));

        let mut oversized_balance = player.clone();
        oversized_balance.cash_points = i64::MAX as u64 + 1;
        let error = save_player(&database, &guard, &oversized_balance)
            .await
            .expect_err("oversized cash-point balance must fail");
        assert!(error.to_string().contains("cash_points"));

        let mut invalid_inventory = player;
        invalid_inventory
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks[0]
            .quantity = 0;
        let error = save_player(&database, &guard, &invalid_inventory)
            .await
            .expect_err("zero-quantity inventory stack must fail");
        assert!(
            error
                .to_string()
                .contains("positive item IDs and quantities")
        );

        let current = load_player(&database, &player_id)
            .await
            .expect("load valid player after cancelled saves")
            .expect("valid player");
        save_player(&database, &guard, &current)
            .await
            .expect("save after cancelled transactions");
    }

    #[tokio::test]
    async fn load_reports_semantically_invalid_current_records_as_corrupt() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"), 10_000)
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("invalid-load").expect("valid player ID");
        let guard = test_guard(&player_id).await;
        create_test_player(&database, &guard, &player_id).await;
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
        guard: &PlayerGuard,
        player_id: &PlayerId,
    ) -> PlayerState {
        create_player(
            database,
            guard,
            player_id,
            &CharacterName::parse("Mina").expect("valid name"),
            appearance(),
            crate::items::starter_inventory(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
            10_000,
        )
        .await
        .expect("create player")
    }

    async fn test_guard(player_id: &PlayerId) -> PlayerGuard {
        let locks = PlayerLocks::default();
        acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard")
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
            cash_points: 5_000,
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
