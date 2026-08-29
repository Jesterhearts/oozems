use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MonsterBookCard;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestRecordEntry;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::Vec2;
use rusqlite::ErrorCode;
use rusqlite::TransactionBehavior;

use super::APPLICATION_ID;
use super::DatabaseError;
use super::PlayerId;
use super::SCHEMA_VERSION;
use super::create_player;
use super::load_player;
use super::open_sqlite;
use super::restore_player;
use super::save_player;

#[tokio::test]
async fn schema_and_worker_pragmas_are_installed_before_use() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");

    let writer_pragmas = database
        .write(|connection| {
            Ok((
                connection
                    .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))?,
                connection.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))?,
                connection
                    .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
                connection.pragma_query_value(None, "synchronous", |row| row.get::<_, i32>(0))?,
                connection.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i32>(0))?,
                connection
                    .pragma_query_value(None, "trusted_schema", |row| row.get::<_, i32>(0))?,
            ))
        })
        .await
        .expect("writer pragmas");
    assert_eq!(writer_pragmas.0, APPLICATION_ID);
    assert_eq!(writer_pragmas.1, SCHEMA_VERSION);
    assert_eq!(writer_pragmas.2.to_ascii_lowercase(), "wal");
    assert_eq!(writer_pragmas.3, 2);
    assert_eq!(writer_pragmas.4, 1);
    assert_eq!(writer_pragmas.5, 0);

    let reader_pragmas = database
        .read(|connection| {
            Ok((
                connection.pragma_query_value(None, "query_only", |row| row.get::<_, i32>(0))?,
                connection.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i32>(0))?,
            ))
        })
        .await
        .expect("reader pragmas");
    assert_eq!(reader_pragmas, (1, 1));

    let (strict_tables, target_indexes) = database
        .read(|connection| {
            let strict_tables = connection.query_row(
                "SELECT count(*) FROM pragma_table_list
                 WHERE schema = 'main' AND type = 'table' AND strict = 1
                     AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            let target_indexes = connection.query_row(
                "SELECT count(*) FROM sqlite_schema
                 WHERE type = 'index'
                     AND name IN (
                         'key_bindings_unique_action_target',
                         'key_bindings_unique_skill_target'
                     )
                     AND sql LIKE '%WHERE%'",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok((strict_tables, target_indexes))
        })
        .await
        .expect("schema metadata");
    assert_eq!(strict_tables, 10);
    assert_eq!(target_indexes, 2);

    database.close().await.expect("close database");
    let error = load_player(
        &database,
        &PlayerId::parse("closed-player").expect("player ID"),
    )
    .await
    .expect_err("closed database must reject commands");
    assert!(matches!(error, DatabaseError::Worker { .. }));
}

#[test]
fn foreign_and_unsupported_schema_versions_are_rejected() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let foreign_path = directory.path().join("foreign.sqlite");
    let foreign = rusqlite::Connection::open(&foreign_path).expect("open foreign database");
    foreign
        .pragma_update(None, "application_id", 123_i32)
        .expect("set foreign application ID");
    drop(foreign);

    let error = open_sqlite(&foreign_path)
        .err()
        .expect("foreign database must fail");
    assert!(matches!(error, DatabaseError::Schema { .. }));
    assert!(error.to_string().contains("not an oozems database"));

    let newer_path = directory.path().join("newer.sqlite");
    let newer = rusqlite::Connection::open(&newer_path).expect("open newer database");
    newer
        .pragma_update(None, "application_id", APPLICATION_ID)
        .expect("set application ID");
    newer
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("set newer schema version");
    drop(newer);

    let error = open_sqlite(&newer_path)
        .err()
        .expect("newer schema must fail");
    assert!(matches!(error, DatabaseError::Schema { .. }));
    assert!(error.to_string().contains("unsupported schema version"));
}

#[tokio::test]
async fn comprehensive_round_trip_is_normalized_and_does_not_load_position() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let input = comprehensive_player("round-trip");

    let created = create_player(&database, &input)
        .await
        .expect("create player");
    assert_eq!(created.revision, 1);
    assert_eq!(created.position, input.position);
    assert_eq!(created.inventory.as_ref().unwrap().stacks.len(), 2);
    assert_eq!(
        created.inventory.as_ref().unwrap().stacks[0],
        created.inventory.as_ref().unwrap().stacks[1]
    );
    assert_eq!(
        created
            .inventory
            .as_ref()
            .unwrap()
            .equipment
            .iter()
            .map(|item| item.slot)
            .collect::<Vec<_>>(),
        vec![EquipmentSlot::Top as i32, EquipmentSlot::Weapon as i32]
    );
    assert!(created.quest_records[0].entries.is_empty());
    assert_eq!(created.learned_skills[0].skill_id, 1_000);
    assert_eq!(created.monster_book_cards[0].card_item_id, 2_380_000);

    let loaded = load_player(&database, &PlayerId::parse(&input.id).expect("player ID"))
        .await
        .expect("load player")
        .expect("player exists");
    let mut expected = created;
    expected.position = None;
    assert_eq!(loaded, expected);
}

#[tokio::test]
async fn create_conflicts_without_replacing_the_existing_player() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let player = comprehensive_player("create-conflict");
    let created = create_player(&database, &player)
        .await
        .expect("create player");
    let mut replacement = player;
    replacement.name = "Other".to_owned();

    let error = create_player(&database, &replacement)
        .await
        .expect_err("duplicate create must fail");
    assert!(matches!(error, DatabaseError::Exists { .. }));
    let loaded = load(&database, "create-conflict").await;
    assert_eq!(loaded.name, created.name);
}

#[tokio::test]
async fn missing_players_and_signed_range_overflow_have_typed_errors() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let mut missing = comprehensive_player("missing-player");
    missing.revision = 1;
    let error = save_player(&database, &missing, &missing)
        .await
        .expect_err("saving an absent player must fail");
    assert!(matches!(error, DatabaseError::NotFound { .. }));

    let mut overflowing = comprehensive_player("overflow-player");
    overflowing.cash_points = i64::MAX as u64 + 1;
    let error = create_player(&database, &overflowing)
        .await
        .expect_err("oversized balance must fail");
    assert!(matches!(
        error,
        DatabaseError::Overflow {
            field: "cash_points"
        }
    ));
}

#[tokio::test]
async fn save_uses_exact_revision_cas_and_preserves_staged_position() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let original = create_player(&database, &comprehensive_player("save-cas"))
        .await
        .expect("create player");
    let mut staged = original.clone();
    staged.mesos += 50;
    staged.position = Some(Vec2 { x: 900.0, y: 75.0 });
    let mut mismatched = staged.clone();
    mismatched.revision += 1;

    let error = save_player(&database, &original, &mismatched)
        .await
        .expect_err("mismatched input revisions must fail");
    assert!(matches!(error, DatabaseError::Invalid { .. }));

    let committed = save_player(&database, &original, &staged)
        .await
        .expect("save player");
    assert_eq!(committed.revision, 2);
    assert_eq!(committed.position, staged.position);
    let loaded = load(&database, "save-cas").await;
    assert_eq!(loaded.position, None);
    assert_eq!(loaded.mesos, staged.mesos);

    let mut stale = original.clone();
    stale.cash_points += 1;
    let error = save_player(&database, &original, &stale)
        .await
        .expect_err("stale save must fail");
    assert!(matches!(
        error,
        DatabaseError::RevisionConflict {
            expected: 1,
            actual: 2,
            ..
        }
    ));
}

#[tokio::test]
async fn restore_applies_a_reverse_diff_with_its_own_cas_revision() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let original = create_player(&database, &comprehensive_player("restore-player"))
        .await
        .expect("create player");
    let mut staged = original.clone();
    staged.name = "Changed".to_owned();
    staged.inventory.as_mut().unwrap().stacks.remove(0);
    staged.learned_skills.clear();
    staged.quest_records.clear();
    staged.monster_book_cards[0].count = 5;
    staged.key_bindings.swap(0, 1);
    staged.position = Some(Vec2 { x: 700.0, y: 800.0 });
    let committed = save_player(&database, &original, &staged)
        .await
        .expect("save staged player");

    let restored = restore_player(&database, &committed, &original)
        .await
        .expect("restore original player");
    let mut expected = original.clone();
    expected.revision = 3;
    assert_eq!(restored, expected);

    expected.position = None;
    assert_eq!(load(&database, "restore-player").await, expected);

    let error = restore_player(&database, &committed, &original)
        .await
        .expect_err("repeated compensation must conflict");
    assert!(matches!(
        error,
        DatabaseError::RevisionConflict {
            expected: 2,
            actual: 3,
            ..
        }
    ));
}

#[tokio::test]
async fn saves_touch_only_changed_child_rows_and_revision_only_when_unchanged() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let original = create_player(&database, &comprehensive_player("row-diff"))
        .await
        .expect("create player");
    database
        .write(|connection| {
            connection.execute_batch(
                "CREATE TABLE inventory_audit (
                     operation TEXT NOT NULL,
                     slot_index INTEGER NOT NULL
                 ) STRICT;
                 CREATE TRIGGER audit_inventory_update
                 AFTER UPDATE ON inventory_stacks
                 BEGIN
                     INSERT INTO inventory_audit VALUES ('update', NEW.slot_index);
                 END;
                 CREATE TRIGGER audit_inventory_insert
                 AFTER INSERT ON inventory_stacks
                 BEGIN
                     INSERT INTO inventory_audit VALUES ('insert', NEW.slot_index);
                 END;
                 CREATE TRIGGER audit_inventory_delete
                 AFTER DELETE ON inventory_stacks
                 BEGIN
                     INSERT INTO inventory_audit VALUES ('delete', OLD.slot_index);
                 END;",
            )?;
            Ok(())
        })
        .await
        .expect("install audit triggers");

    let mut staged = original.clone();
    staged.inventory.as_mut().unwrap().stacks[1].quantity += 1;
    let committed = save_player(&database, &original, &staged)
        .await
        .expect("save one stack");
    let audit = inventory_audit(&database).await;
    assert_eq!(audit, vec![("update".to_owned(), 1)]);

    let mut transient_only = committed.clone();
    transient_only.position = Some(Vec2 { x: 1.0, y: 2.0 });
    let unchanged = save_player(&database, &committed, &transient_only)
        .await
        .expect("revision-only save");
    assert_eq!(unchanged.revision, 3);
    assert_eq!(inventory_audit(&database).await, audit);
}

#[tokio::test]
async fn child_failure_rolls_back_parent_and_prior_child_changes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let original = create_player(&database, &comprehensive_player("rollback-child"))
        .await
        .expect("create player");
    database
        .write(|connection| {
            connection.execute_batch(
                "CREATE TRIGGER reject_test_skill
                 BEFORE INSERT ON learned_skills
                 WHEN NEW.skill_id = 999999
                 BEGIN
                     SELECT RAISE(ABORT, 'rejected test skill');
                 END;",
            )?;
            Ok(())
        })
        .await
        .expect("install rejecting trigger");

    let mut staged = original.clone();
    staged.mesos += 1_000;
    staged.inventory.as_mut().unwrap().stacks[0].quantity += 10;
    staged.learned_skills.push(LearnedSkill {
        skill_id: 999_999,
        level: 1,
        master_level: 0,
    });
    let error = save_player(&database, &original, &staged)
        .await
        .expect_err("child trigger must abort save");
    assert!(matches!(error, DatabaseError::Storage(_)));

    let mut expected = original;
    expected.position = None;
    assert_eq!(load(&database, "rollback-child").await, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_reader_transaction_observes_one_aggregate_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    let original = create_player(&database, &comprehensive_player("reader-snapshot"))
        .await
        .expect("create player");
    let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
    let (release_sender, release_receiver) = mpsc::sync_channel(1);
    let reader_database = database.clone();
    let reader = tokio::spawn(async move {
        reader_database
            .read(move |connection| {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
                let revision = transaction.query_row(
                    "SELECT revision FROM players WHERE player_id = 'reader-snapshot'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                entered_sender.send(()).expect("signal reader snapshot");
                release_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release reader snapshot");
                let stack_count = transaction.query_row(
                    "SELECT count(*) FROM inventory_stacks
                     WHERE player_id = 'reader-snapshot'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                transaction.commit()?;
                Ok((revision, stack_count))
            })
            .await
    });
    entered_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("reader entered transaction");

    let mut staged = original.clone();
    staged
        .inventory
        .as_mut()
        .unwrap()
        .stacks
        .push(InventoryItemStack {
            item_id: 2_000_100,
            quantity: 1,
            expires_at_unix_ms: 0,
        });
    let committed = save_player(&database, &original, &staged)
        .await
        .expect("writer commits during reader snapshot");
    assert_eq!(committed.revision, 2);
    release_sender.send(()).expect("release reader");
    assert_eq!(
        reader.await.expect("reader task").expect("reader result"),
        (1, 2)
    );
    assert_eq!(
        load(&database, "reader-snapshot")
            .await
            .inventory
            .unwrap()
            .stacks
            .len(),
        3
    );
}

#[tokio::test]
async fn foreign_keys_and_integrity_checks_hold() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    create_player(&database, &comprehensive_player("integrity-player"))
        .await
        .expect("create player");

    let orphan_error = database
        .write(|connection| {
            connection.execute(
                "INSERT INTO quest_record_entries
                 (player_id, quest_id, entry_index, value)
                 VALUES ('integrity-player', 999999, 0, 'orphan')",
                [],
            )?;
            Ok(())
        })
        .await
        .expect_err("grandchild orphan must fail");
    assert!(matches!(
        orphan_error,
        DatabaseError::Storage(ref error)
            if error.sqlite_error_code() == Some(ErrorCode::ConstraintViolation)
    ));

    let checks = database
        .read(|connection| {
            let quick_check =
                connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
            let foreign_key_violations = connection.query_row(
                "SELECT count(*) FROM pragma_foreign_key_check",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok((quick_check, foreign_key_violations))
        })
        .await
        .expect("integrity checks");
    assert_eq!(checks, ("ok".to_owned(), 0));
}

#[tokio::test]
async fn invalid_loaded_data_is_reported_as_corrupt() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    create_player(&database, &comprehensive_player("corrupt-player"))
        .await
        .expect("create player");
    database
        .write(|connection| {
            connection.pragma_update(None, "ignore_check_constraints", true)?;
            connection.execute(
                "UPDATE players SET name = '!' WHERE player_id = 'corrupt-player'",
                [],
            )?;
            connection.pragma_update(None, "ignore_check_constraints", false)?;
            Ok(())
        })
        .await
        .expect("write corrupt fixture");

    let error = load_player(
        &database,
        &PlayerId::parse("corrupt-player").expect("player ID"),
    )
    .await
    .expect_err("corrupt player must fail");
    assert!(matches!(error, DatabaseError::Corrupt { .. }));
}

#[tokio::test]
async fn neighboring_surreal_kv_files_are_ignored() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let surreal_path = directory.path().join("surrealkv");
    fs::create_dir(&surreal_path).expect("create neighboring SurrealKV directory");
    let marker = surreal_path.join("marker");
    fs::write(&marker, b"leave me alone").expect("write marker");

    let database = open_sqlite(&directory.path().join("players.sqlite")).expect("open SQLite");
    create_player(&database, &comprehensive_player("neighbor-test"))
        .await
        .expect("create player");

    assert_eq!(fs::read(marker).expect("read marker"), b"leave me alone");
    assert!(directory.path().join("players.sqlite").is_file());
}

async fn load(
    database: &super::Database,
    player_id: &str,
) -> PlayerState {
    load_player(
        database,
        &PlayerId::parse(player_id).expect("valid player ID"),
    )
    .await
    .expect("load player")
    .expect("player exists")
}

async fn inventory_audit(database: &super::Database) -> Vec<(String, i64)> {
    database
        .read(|connection| {
            let mut statement = connection
                .prepare("SELECT operation, slot_index FROM inventory_audit ORDER BY rowid")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()
                .map_err(DatabaseError::Storage)
        })
        .await
        .expect("read inventory audit")
}

fn comprehensive_player(player_id: &str) -> PlayerState {
    let duplicate_stack = InventoryItemStack {
        item_id: 2_000_000,
        quantity: 3,
        expires_at_unix_ms: 1_900_000_000_000,
    };
    let mut key_bindings = crate::keymap::default_bindings();
    key_bindings.push(KeyBinding {
        code: "F1".to_owned(),
        action: KeyAction::Unspecified as i32,
        skill_id: 1_000,
    });
    PlayerState {
        id: player_id.to_owned(),
        name: "Mina".to_owned(),
        level: 7,
        map_id: 10_000,
        position: Some(Vec2 { x: 160.0, y: 420.0 }),
        appearance: Some(CharacterAppearance {
            gender: CharacterGender::Female as i32,
            skin_id: 2_000,
            face_id: 21_000,
            hair_id: 31_000,
        }),
        stats: Some(CharacterStats {
            job_id: 100,
            hp: 200,
            max_hp: 250,
            mp: 100,
            max_mp: 150,
            experience: 9_999,
            fame: -5,
            ability_points: 3,
            strength: 20,
            dexterity: 15,
            intelligence: 4,
            luck: 4,
            experience_required: 12_345,
        }),
        inventory: Some(InventoryState {
            equipment: vec![
                EquippedItem {
                    slot: EquipmentSlot::Weapon as i32,
                    item_id: 1_300_000,
                    expires_at_unix_ms: 1_900_000_000_123,
                },
                EquippedItem {
                    slot: EquipmentSlot::Top as i32,
                    item_id: 1_040_000,
                    expires_at_unix_ms: 0,
                },
            ],
            capacity: 8,
            stacks: vec![duplicate_stack, duplicate_stack],
        }),
        key_bindings,
        skill_points: 2,
        learned_skills: vec![
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
        ],
        mesos: 1_234,
        quests: vec![
            PlayerQuest {
                quest_id: 1_010,
                status: QuestStatus::Unspecified as i32,
                mob_progress: Vec::new(),
                accepted_at_unix_ms: 0,
                completed_at_unix_ms: 0,
                dialogue_step: 1,
                completion_quiz_passed: false,
            },
            PlayerQuest {
                quest_id: 1_009,
                status: QuestStatus::Started as i32,
                mob_progress: vec![
                    QuestMobProgress {
                        mob_id: 100_101,
                        count: 2,
                    },
                    QuestMobProgress {
                        mob_id: 100_100,
                        count: 3,
                    },
                ],
                accepted_at_unix_ms: 123_456,
                completed_at_unix_ms: 0,
                dialogue_step: 0,
                completion_quiz_passed: false,
            },
        ],
        revision: 99,
        quest_records: vec![
            QuestRecord {
                quest_id: 2_236,
                entries: vec![
                    QuestRecordEntry {
                        index: 42,
                        value: "sparse".to_owned(),
                    },
                    QuestRecordEntry {
                        index: 0,
                        value: "000000".to_owned(),
                    },
                ],
            },
            QuestRecord {
                quest_id: 2_235,
                entries: Vec::new(),
            },
        ],
        monster_book_cards: vec![
            MonsterBookCard {
                card_item_id: 2_380_001,
                count: 5,
            },
            MonsterBookCard {
                card_item_id: 2_380_000,
                count: 2,
            },
        ],
        cash_points: 9_876,
    }
}
