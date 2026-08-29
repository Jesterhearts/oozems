use std::sync::Arc;

use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::PlayerState;

use super::ApiError;
use crate::app::AppState;
use crate::database::Database;
use crate::database::PlayerId;
use crate::effects::ActiveEffects;
use crate::effects::PlayerEffects;
use crate::effects::ProjectedEffects;
use crate::items::EquipmentStats;
use crate::player_lock::PlayerGuard;

pub(super) struct LoadedPlayer {
    pub(super) original: PlayerState,
    pub(super) player: PlayerState,
    pub(super) changed: bool,
}

pub(crate) struct PlayerMutation {
    pub(crate) original: PlayerState,
    pub(crate) player: PlayerState,
    pub(crate) original_effects: PlayerEffects,
    pub(crate) effects: PlayerEffects,
    pub(crate) now_unix_ms: u64,
}

pub(crate) fn project_combat_effects(
    mut effects: ProjectedEffects,
    equipment: EquipmentStats,
) -> ProjectedEffects {
    effects.modifiers.weapon_attack = effects
        .modifiers
        .weapon_attack
        .saturating_add(equipment.weapon_attack);
    effects.modifiers.weapon_defense = effects
        .modifiers
        .weapon_defense
        .saturating_add(equipment.weapon_defense);
    effects.modifiers.magic_defense = effects
        .modifiers
        .magic_defense
        .saturating_add(equipment.magic_defense);
    effects
}

pub(crate) async fn begin_player_mutation(
    state: &AppState,
    guard: &PlayerGuard,
    player_id: &PlayerId,
    now_unix_ms: u64,
) -> Result<PlayerMutation, ApiError> {
    crate::player_lock::validate_player_guard(&state.player_locks, guard, player_id.as_str())?;
    let mut loaded = load_player(state, player_id)
        .await?
        .filter(|loaded| loaded.player.appearance.is_some())
        .ok_or_else(|| ApiError::not_found("player_not_found", "player does not exist"))?;
    let synchronized = crate::movement::synchronize_player(&state.movement, loaded.player.clone())?;
    let runtime_map = if synchronized.position.is_some() {
        loaded.player = synchronized;
        None
    } else {
        let (map, position, repaired) =
            super::resolve_reconnect_destination(state, loaded.player.map_id).await?;
        loaded.player.map_id = map.id;
        loaded.player.position = Some(position);
        loaded.changed |= repaired;
        Some(map)
    };
    let original_effects =
        crate::effects::snapshot(&state.active_effects, &loaded.player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(
        state,
        loaded.player.clone(),
        original_effects.clone(),
        now_unix_ms,
    );
    let mutation = persist_player_baseline(
        &state.database,
        state.active_effects.clone(),
        guard,
        loaded,
        original_effects,
        advanced,
        now_unix_ms,
    )
    .await?;
    if let Some(map) = runtime_map {
        crate::movement::initialize_player(
            &state.movement,
            &mutation.player,
            &map,
            state.gameplay.movement,
            now_unix_ms,
        )?;
    }
    Ok(mutation)
}

async fn persist_player_baseline(
    database: &Database,
    active_effects: Arc<ActiveEffects>,
    guard: &PlayerGuard,
    loaded: LoadedPlayer,
    original_effects: PlayerEffects,
    advanced: crate::quests::AutomaticQuestAdvance,
    now_unix_ms: u64,
) -> Result<PlayerMutation, ApiError> {
    if !loaded.changed && !advanced.changed {
        return Ok(PlayerMutation {
            original: loaded.original,
            player: advanced.player,
            original_effects,
            effects: advanced.effects,
            now_unix_ms,
        });
    }

    let mut transaction = crate::player_transaction::new_player_transaction(
        loaded.original,
        advanced.player,
        crate::player_transaction::PlayerPersistence::Full,
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        active_effects,
        original_effects,
        advanced.effects,
    );
    let committed =
        crate::player_transaction::commit_player_transaction(database, guard, transaction).await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("baseline player transaction stages effects");
    Ok(PlayerMutation {
        original: player.clone(),
        player,
        original_effects: effects.clone(),
        effects,
        now_unix_ms,
    })
}

pub(crate) fn advance_automatic_player(
    state: &AppState,
    player: PlayerState,
    effects: PlayerEffects,
    now_unix_ms: u64,
) -> crate::quests::AutomaticQuestAdvance {
    let definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    let consume_effects = state.catalog.consume_effect_definitions();
    let advanced = crate::quests::advance_automatic_quests(
        player,
        effects,
        definitions,
        state.experience.default_curve(),
        state.catalog.item_definition_slice(),
        &consume_effects,
        &state.quest_scripts,
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: state.gameplay.world_id,
        },
    );
    if !advanced.failures.is_empty() {
        tracing::warn!(
            player_id = %advanced.player.id,
            failures = ?advanced.failures,
            "automatic quest transitions were blocked"
        );
    }
    advanced
}

pub(super) async fn load_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<Option<LoadedPlayer>, ApiError> {
    let Some(mut player) = crate::database::load_player(&state.database, player_id).await? else {
        return Ok(None);
    };
    let original = player.clone();
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(crate::items::ItemRuleError::MissingInventory)
        .map_err(super::item_rule_error)?;
    let inventory_pruned = crate::items::prune_and_validate_inventory(
        inventory,
        state.catalog.as_ref(),
        super::unix_time_ms()?,
    )
    .map_err(|error| ApiError::PlayerData(error.to_string()))?;
    let appearance = player
        .appearance
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("character appearance is missing".to_owned()))?;
    if !state.catalog.supports_character(appearance) {
        return Err(ApiError::PlayerData(
            "character appearance is not available in the current content".to_owned(),
        ));
    }
    let skill_context = super::load_skill_book(state, &player).await?;
    crate::skills::validate_bound_skills(&player.key_bindings, &player, &skill_context)
        .map_err(|error| ApiError::PlayerData(error.to_string()))?;
    let known_card_ids = state.catalog.monster_book_card_ids();
    if !known_card_ids.is_empty() {
        let unknown_card_ids = player
            .monster_book_cards
            .iter()
            .map(|card| card.card_item_id)
            .filter(|card_item_id| !known_card_ids.contains(card_item_id))
            .collect::<Vec<_>>();
        if !unknown_card_ids.is_empty() {
            tracing::warn!(
                player_id = %player.id,
                ?unknown_card_ids,
                "preserving Monster Book cards absent from the current content catalog"
            );
        }
    }
    player = crate::experience::apply_curve(player, state.experience.default_curve())?;
    Ok(Some(LoadedPlayer {
        original,
        player,
        changed: inventory_pruned,
    }))
}

pub(super) async fn require_player(
    state: &AppState,
    guard: &PlayerGuard,
    player_id: &PlayerId,
) -> Result<PlayerState, ApiError> {
    require_player_at(state, guard, player_id, super::unix_time_ms()?).await
}

async fn require_player_at(
    state: &AppState,
    guard: &PlayerGuard,
    player_id: &PlayerId,
    now_unix_ms: u64,
) -> Result<PlayerState, ApiError> {
    Ok(begin_player_mutation(state, guard, player_id, now_unix_ms)
        .await?
        .player)
}

pub(super) async fn process_automatic_quests(
    state: &AppState,
    guard: &PlayerGuard,
    loaded: LoadedPlayer,
    now_unix_ms: u64,
) -> Result<PlayerState, ApiError> {
    crate::player_lock::validate_player_guard(&state.player_locks, guard, &loaded.player.id)?;
    let effects = crate::effects::snapshot(&state.active_effects, &loaded.player.id, now_unix_ms)?;
    let advanced =
        advance_automatic_player(state, loaded.player.clone(), effects.clone(), now_unix_ms);
    Ok(persist_player_baseline(
        &state.database,
        state.active_effects.clone(),
        guard,
        loaded,
        effects,
        advanced,
        now_unix_ms,
    )
    .await?
    .player)
}

pub(crate) fn prepare_player_mutation(
    state: &AppState,
    mutation: PlayerMutation,
    player: PlayerState,
    persistence_required: bool,
    activity: bool,
) -> (
    crate::player_transaction::PlayerTransaction,
    ActiveBuffState,
) {
    let advanced = advance_automatic_player(state, player, mutation.effects, mutation.now_unix_ms);
    let active_buffs = crate::effects::state(&advanced.effects, mutation.now_unix_ms);
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        advanced.player,
        if persistence_required || advanced.changed {
            crate::player_transaction::PlayerPersistence::Full
        } else {
            crate::player_transaction::PlayerPersistence::None
        },
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        advanced.effects,
    );
    if activity {
        let player_id = crate::player_transaction::staged_player(&transaction)
            .id
            .clone();
        crate::player_transaction::stage_activity(
            &mut transaction,
            state.recovery_timers.clone(),
            player_id,
            mutation.now_unix_ms,
        );
    }
    (transaction, active_buffs)
}

pub(crate) async fn persist_player_mutation(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    player: PlayerState,
    persistence_required: bool,
    activity: bool,
) -> Result<crate::player_transaction::CommittedPlayerTransaction, ApiError> {
    let (transaction, _) =
        prepare_player_mutation(state, mutation, player, persistence_required, activity);
    Ok(
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?,
    )
}

pub(crate) fn prepare_simulation_player_effects(
    state: &AppState,
    mut player: PlayerState,
    simulation: &crate::mobs::MobUpdate,
    mut effects: PlayerEffects,
    now_unix_ms: u64,
    persistence_required: bool,
) -> PreparedSimulationPersistence {
    let player_damage = simulation.player_damage();
    if player_damage > 0 {
        let stats = player.stats.get_or_insert_default();
        stats.hp = stats
            .hp
            .saturating_sub(u32::try_from(player_damage).unwrap_or(u32::MAX));
        crate::effects::cancel_damage_morphs(&mut effects, |morph_id| {
            state.catalog.morph_definition(morph_id)
        });
    }
    let mob_kills = simulation
        .mob_deaths
        .iter()
        .map(|death| (death.definition_id, death.source_skill_id))
        .collect::<Vec<_>>();
    let kill_result =
        crate::quests::record_mob_kills(player, &mob_kills, state.catalog.quest_definitions());
    let advanced = advance_automatic_player(state, kill_result.player, effects, now_unix_ms);
    let should_save = persistence_required
        || player_damage > 0
        || !kill_result.changed_quest_ids.is_empty()
        || advanced.changed;
    PreparedSimulationPersistence {
        player: advanced.player,
        effects: advanced.effects,
        should_save,
    }
}

pub(crate) struct PreparedSimulationPersistence {
    pub(crate) player: PlayerState,
    pub(crate) effects: PlayerEffects,
    pub(crate) should_save: bool,
}

pub(crate) fn merge_dropped_items(
    current: &mut Vec<oozems_proto::v1::DroppedItem>,
    additions: Vec<oozems_proto::v1::DroppedItem>,
) {
    for drop in additions {
        if !current.iter().any(|current| current.id == drop.id) {
            current.push(drop);
        }
    }
}

pub(super) fn active_buff_state(
    state: &AppState,
    player_id: &str,
    now_unix_ms: u64,
) -> Result<ActiveBuffState, ApiError> {
    let effects = crate::effects::snapshot(&state.active_effects, player_id, now_unix_ms)?;
    Ok(crate::effects::state(&effects, now_unix_ms))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::Vec2;

    use super::LoadedPlayer;
    use super::persist_player_baseline;
    use super::project_combat_effects;
    use crate::database::PlayerId;
    use crate::player_lock::PlayerLocks;
    use crate::player_lock::acquire_player;

    #[test]
    fn equipment_stats_are_added_to_active_effect_modifiers() {
        let effects = crate::effects::ProjectedEffects {
            modifiers: crate::effects::EffectModifiers {
                weapon_defense: 4,
                magic_defense: 6,
                ..crate::effects::EffectModifiers::default()
            },
            ..crate::effects::ProjectedEffects::default()
        };

        let projected = project_combat_effects(
            effects,
            crate::items::EquipmentStats {
                weapon_attack: 20,
                weapon_defense: 8,
                magic_defense: 5,
            },
        );

        assert_eq!(projected.modifiers.weapon_defense, 12);
        assert_eq!(projected.modifiers.magic_defense, 11);
        assert_eq!(projected.modifiers.weapon_attack, 20);
    }

    #[tokio::test]
    async fn rejected_action_and_later_rollback_keep_the_eager_automatic_baseline() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player_id = PlayerId::parse("baseline-rejection").expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let original = crate::database::create_player(
            &database,
            &oozems_proto::v1::PlayerState {
                id: player_id.as_str().to_owned(),
                name: "Mina".to_owned(),
                level: 1,
                map_id: 100,
                position: Some(Vec2 { x: 10.0, y: 20.0 }),
                appearance: Some(CharacterAppearance {
                    gender: CharacterGender::Female as i32,
                    skin_id: 2_000,
                    face_id: 21_000,
                    hair_id: 31_000,
                }),
                stats: Some(oozems_proto::v1::CharacterStats {
                    hp: 50,
                    max_hp: 50,
                    mp: 5,
                    max_mp: 5,
                    experience_required: 15,
                    ..oozems_proto::v1::CharacterStats::default()
                }),
                inventory: Some(crate::items::starter_inventory()),
                key_bindings: crate::keymap::default_bindings(),
                skill_points: 3,
                ..oozems_proto::v1::PlayerState::default()
            },
        )
        .await
        .expect("create player");
        let effects = Arc::new(crate::effects::ActiveEffects::default());
        let original_effects = crate::effects::snapshot(&effects, player_id.as_str(), 10_000)
            .expect("effect snapshot");
        let mut advanced_player = original.clone();
        advanced_player.quests.push(PlayerQuest {
            quest_id: 100,
            status: QuestStatus::Completed as i32,
            completed_at_unix_ms: 10_000,
            ..PlayerQuest::default()
        });
        let advanced = crate::quests::AutomaticQuestAdvance {
            player: advanced_player,
            effects: original_effects.clone(),
            changed: true,
            started_quest_ids: Vec::new(),
            completed_quest_ids: vec![100],
            expired_quest_ids: Vec::new(),
            failures: Vec::new(),
        };

        let mutation = persist_player_baseline(
            &database,
            effects.clone(),
            &guard,
            LoadedPlayer {
                original: original.clone(),
                player: original,
                changed: false,
            },
            original_effects,
            advanced,
            10_000,
        )
        .await
        .expect("persist automatic baseline");
        let rejected_action = Result::<(), &str>::Err("later action validation failed");
        assert!(rejected_action.is_err());

        let durable = crate::database::load_player(&database, &player_id)
            .await
            .expect("load durable player")
            .expect("durable player");
        assert_eq!(durable.quests.len(), 1);
        assert_eq!(durable.quests[0].quest_id, 100);
        assert_eq!(
            QuestStatus::try_from(durable.quests[0].status),
            Ok(QuestStatus::Completed)
        );

        let baseline = mutation.original.clone();
        let mut concurrent_effects = mutation.original_effects.clone();
        crate::effects::apply_skill_effect(
            &mut concurrent_effects,
            &oozems_proto::v1::SkillUseResult {
                skill_id: 1_000,
                duration_ms: 10_000,
                ..oozems_proto::v1::SkillUseResult::default()
            },
            10_000,
        );
        crate::effects::commit(&effects, player_id.as_str(), concurrent_effects)
            .expect("install a conflicting effect snapshot");
        let mut later_player = mutation.player;
        later_player.mesos = 100;
        let mut transaction = crate::player_transaction::new_player_transaction(
            mutation.original,
            later_player,
            crate::player_transaction::PlayerPersistence::Full,
        );
        crate::player_transaction::stage_effects(
            &mut transaction,
            effects,
            mutation.original_effects,
            mutation.effects,
        );

        let error =
            crate::player_transaction::commit_player_transaction(&database, &guard, transaction)
                .await
                .err()
                .expect("the conflicting effect snapshot must roll back the player");
        assert!(matches!(
            error,
            crate::player_transaction::PlayerTransactionError::Failed { failure }
                if matches!(
                    *failure,
                    crate::player_transaction::TransactionFailure::Effects(_)
                )
        ));
        let restored = crate::database::load_player(&database, &player_id)
            .await
            .expect("load restored player")
            .expect("restored player");
        assert_eq!(restored.quests, baseline.quests);
        assert_eq!(restored.mesos, baseline.mesos);
    }
}
