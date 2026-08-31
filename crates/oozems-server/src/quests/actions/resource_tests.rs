use crate::quests::test_support::*;

#[test]
fn acceptance_applies_costs_and_grants_before_starting() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.items.push(item_requirement(ITEM_A, 1));
    quest.start_actions = QuestActions {
        fixed_items: vec![
            QuestItemDelta {
                item_id: ITEM_A,
                count: -1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: ITEM_B,
                count: 2,
                expiration: None,
            },
        ],
        money: -50,
        experience: 2,
        fame: 3,
        next_quest_id: None,
        conditional_items: Vec::new(),
        weighted_items: Vec::new(),
        selectable_items: Vec::new(),
        quest_state_actions: Vec::new(),
        record_writes: Vec::new(),
        skill_changes: Vec::new(),
        buff_item_ids: Vec::new(),
        presentation_npc_id: None,
        npc_animation_action: None,
    };
    let mut player = player(vec![(ITEM_A, 2)], 4);
    player.mesos = 100;

    let accepted = select_choice(
        player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        123_456,
    )
    .expect("accept quest");

    assert_eq!(accepted.player.mesos, 50);
    assert_eq!(item_count(&accepted.player, ITEM_A), 1);
    assert_eq!(item_count(&accepted.player, ITEM_B), 2);
    let stats = accepted.player.stats.expect("stats");
    assert_eq!(stats.experience, 2);
    assert_eq!(stats.fame, 3);
    let entry = &accepted.player.quests[0];
    assert_eq!(entry.status, QuestStatus::Started as i32);
    assert_eq!(entry.accepted_at_unix_ms, 123_456);
    assert_eq!(entry.completed_at_unix_ms, 0);
    assert!(entry.mob_progress.is_empty());
}

#[test]
fn relative_item_expiration_and_completion_use_action_execution_time() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_A,
        count: 1,
        expiration: Some(QuestItemExpiration::RelativeMilliseconds(3_600_000)),
    });
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(quest.id, QuestStatus::Started)
    });

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        10_000,
    )
    .expect("complete with a relative expiration reward");

    assert_eq!(completed.player.quests[0].completed_at_unix_ms, 10_000);
    assert_eq!(
        completed.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
        3_610_000
    );
}

#[test]
fn expired_absolute_reward_does_not_block_quest_completion() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_A,
        count: 1,
        expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200)),
    });
    quest.completion_actions.money = 50;
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("an expired item grant is skipped");

    assert_eq!(
        progress(&completed.player, quest.id),
        QuestProgress::Completed
    );
    assert_eq!(completed.player.mesos, 50);
    assert_eq!(item_count(&completed.player, ITEM_A), 0);
}

#[test]
fn relative_item_expiration_overflow_is_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_A,
        count: 1,
        expiration: Some(QuestItemExpiration::RelativeMilliseconds(2)),
    });
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    assert!(matches!(
        select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            u64::MAX - 1,
        ),
        Err(QuestRuleError::ActionOverflow { quest_id: 100 })
    ));
}

#[test]
fn failed_acceptance_preflight_returns_no_mutated_state() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start_actions.money = -101;
    let mut player = player(Vec::new(), 1);
    player.mesos = 100;
    let unchanged = player.clone();

    assert!(matches!(
        select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InsufficientMesos))
    ));
    assert_eq!(unchanged.mesos, 100);
    assert!(unchanged.quests.is_empty());
}

#[test]
fn script_and_wz_actions_share_atomic_quest_selection() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.script = Some("replace_item".to_owned());
    quest.start_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_B,
        count: 1,
        expiration: None,
    });
    quest.start_actions.npc_animation_action = Some("scripted".to_owned());
    let scripts = script_catalog(
        r#"
                [[scripts]]
                name = "replace_item"
                result_pages = ["Script result"]

                [[scripts.actions]]
                type = "item_delta"
                item_id = 4000000
                delta = -1

                [[scripts.actions]]
                type = "mesos"
                delta = -101
            "#,
        &quest,
        &definitions,
    );
    let mut insufficient = player(vec![(ITEM_A, 1)], 1);
    insufficient.mesos = 100;
    let unchanged = insufficient.clone();

    assert!(matches!(
        select_choice(
            insufficient,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            &scripts,
            100,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InsufficientMesos))
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 1);
    assert_eq!(item_count(&unchanged, ITEM_B), 0);
    assert_eq!(unchanged.mesos, 100);
    assert!(unchanged.quests.is_empty());

    let mut sufficient = unchanged;
    sufficient.mesos = 101;
    let selected = select_choice(
        sufficient,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        &scripts,
        100,
    )
    .expect("combined script and WZ actions");
    assert_eq!(item_count(&selected.player, ITEM_A), 0);
    assert_eq!(item_count(&selected.player, ITEM_B), 1);
    assert_eq!(selected.player.mesos, 0);
    assert_eq!(selected.pages, vec!["Accepted", "Script result"]);
    assert_eq!(selected.npc_animation_action.as_deref(), Some("scripted"));
    assert_eq!(progress(&selected.player, quest.id), QuestProgress::Started);
}

#[test]
fn quest_buff_actions_apply_hp_and_effects_only_after_other_actions_succeed() {
    let definitions = item_definitions();
    let definition = ConsumeEffectDefinition {
        hp: 50,
        speed: 10,
        ..consume_effect(EFFECT_ITEM, None)
    };
    let actions = QuestActions {
        fixed_items: vec![QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: None,
        }],
        buff_item_ids: vec![EFFECT_ITEM],
        ..QuestActions::default()
    };
    let mut effects = PlayerEffects::default();
    let initial_effects = effects.clone();
    let mut invalid_player = player(Vec::new(), 1);
    invalid_player.inventory = None;

    let error = super::apply_actions(
        invalid_player,
        100,
        100,
        100,
        &actions,
        None,
        curve(),
        &definitions,
        &[definition],
        &mut effects,
        crate::skills::LearnedSkillModifiers::default(),
    )
    .expect_err("missing inventory must reject all actions");
    assert!(matches!(
        error,
        QuestRuleError::Item(ItemRuleError::MissingInventory)
    ));
    assert_eq!(effects, initial_effects);

    let mut valid_player = player(Vec::new(), 1);
    let stats = valid_player.stats.as_mut().expect("stats");
    stats.hp = 80;
    stats.max_hp = 100;
    let updated = super::apply_actions(
        valid_player,
        100,
        100,
        100,
        &actions,
        None,
        curve(),
        &definitions,
        &[definition],
        &mut effects,
        crate::skills::LearnedSkillModifiers::default(),
    )
    .expect("valid actions");

    assert_eq!(updated.stats.expect("stats").hp, 100);
    assert_eq!(item_count(&updated, ITEM_A), 1);
    assert!(effects.contains_item(EFFECT_ITEM));
    assert_eq!(effects.projected().modifiers.speed, 10);
}
