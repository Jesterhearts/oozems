use crate::quests::test_support::*;

#[test]
fn availability_enforces_item_job_level_and_quest_prerequisites() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.minimum_level = Some(5);
    quest.start.maximum_level = Some(10);
    quest.start.allowed_jobs = vec![0];
    quest.start.items = vec![item_requirement(ITEM_A, 2)];
    quest.start.quests = vec![QuestStateRequirement {
        quest_id: 99,
        state: RequiredQuestState::Completed,
    }];
    let mut player = player(vec![(ITEM_A, 1)], 4);
    player.level = 5;
    player.quests.push(player_quest(99, QuestStatus::Started));

    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.quests[0].status = QuestStatus::Completed as i32;
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.level = 11;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.level = 5;
    player.stats.as_mut().expect("stats").job_id = 100;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
}

#[test]
fn fame_availability_is_inclusive_and_requires_character_stats() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.minimum_fame = Some(3);
    let mut player = player(Vec::new(), 1);
    player.stats.as_mut().expect("stats").fame = 2;

    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player.stats.as_mut().expect("stats").fame = 3;
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player.stats.as_mut().expect("stats").fame = 4;
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player.stats = None;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));

    quest.start.minimum_fame = Some(0);
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
}

#[test]
fn world_availability_uses_inclusive_configured_bounds_for_manual_and_automatic_start() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.minimum_world_id = Some(2);
    quest.start.maximum_world_id = Some(4);
    let mut player = player(Vec::new(), 1);
    player.stats = None;

    for (world_id, expected) in [(1, false), (2, true), (3, true), (4, true), (5, false)] {
        assert_eq!(
            is_available_in_environment(
                &player,
                &quest,
                &definitions,
                scripts(),
                super::QuestEnvironment {
                    now_unix_ms: 1_000,
                    world_id,
                    learned_skill_modifiers: crate::skills::LearnedSkillModifiers::default(),
                },
            ),
            expected,
            "world {world_id}",
        );
    }

    quest.info.auto_start = true;
    quest.start.minimum_world_id = Some(3);
    quest.start.maximum_world_id = Some(3);
    let blocked = advance_automatic_quests_in_environment(
        player.clone(),
        [&quest],
        curve(),
        &definitions,
        scripts(),
        super::QuestEnvironment {
            now_unix_ms: 1_000,
            world_id: 2,
            learned_skill_modifiers: crate::skills::LearnedSkillModifiers::default(),
        },
    );
    assert!(blocked.started_quest_ids.is_empty());
    let exact = advance_automatic_quests_in_environment(
        player,
        [&quest],
        curve(),
        &definitions,
        scripts(),
        super::QuestEnvironment {
            now_unix_ms: 1_000,
            world_id: 3,
            learned_skill_modifiers: crate::skills::LearnedSkillModifiers::default(),
        },
    );
    assert_eq!(exact.started_quest_ids, vec![quest.id]);
}

#[test]
fn availability_enforces_item_absence_in_inventory_and_equipment() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.items.push(QuestItemRequirement {
        item_id: ITEM_A,
        condition: QuestItemCondition::Absent,
    });
    let mut player = player(Vec::new(), 1);

    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player
        .inventory
        .as_mut()
        .expect("inventory")
        .stacks
        .push(InventoryItemStack {
            item_id: ITEM_A,
            quantity: 1,
            expires_at_unix_ms: 0,
        });
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    let inventory = player.inventory.as_mut().expect("inventory");
    inventory.stacks.clear();
    inventory.equipment.push(EquippedItem {
        item_id: ITEM_A,
        ..EquippedItem::default()
    });
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
}

#[test]
fn availability_requires_equipped_items_in_all_of_and_any_of_lists() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.equipped_items = QuestEquippedItemRequirements {
        all_of: vec![ITEM_B, ITEM_C],
        any_of: Vec::new(),
    };
    let mut player = player(vec![(ITEM_C, 1)], 2);
    let inventory = player.inventory.as_mut().expect("inventory");
    inventory.equipment = vec![
        EquippedItem {
            slot: EquipmentSlot::Top as i32,
            item_id: ITEM_B,
            expires_at_unix_ms: 0,
        },
        EquippedItem {
            slot: EquipmentSlot::Bottom as i32,
            item_id: ITEM_C,
            expires_at_unix_ms: 0,
        },
    ];

    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .retain(|item| item.item_id != ITEM_C);
    assert!(
        !is_available(&player, &quest, &definitions, scripts(), 1_000),
        "an item in the bag does not satisfy an equipped-item requirement",
    );

    quest.start.equipped_items = QuestEquippedItemRequirements {
        all_of: Vec::new(),
        any_of: vec![ITEM_B, ITEM_C],
    };
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .clear();
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
    player.inventory = None;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000,
    ));
}

#[test]
fn availability_enforces_map_and_skill_requirements() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.allowed_map_ids = vec![100, 200];
    quest.start.skills = vec![
        QuestSkillRequirement {
            skill_id: 1_001_004,
            acquired: true,
        },
        QuestSkillRequirement {
            skill_id: 1_001_005,
            acquired: false,
        },
    ];
    let mut player = player(Vec::new(), 1);
    player.map_id = 100;
    player.learned_skills.push(LearnedSkill {
        skill_id: 1_001_004,
        level: 2,
        master_level: 0,
    });

    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.map_id = 300;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.map_id = 100;
    player.learned_skills.push(LearnedSkill {
        skill_id: 1_001_005,
        level: 1,
        master_level: 0,
    });
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
}

#[test]
fn availability_requires_active_effects_absent_effects_and_the_exact_morph() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.effects = vec![
        QuestEffectRequirement {
            item_id: EFFECT_ITEM,
            active: true,
        },
        QuestEffectRequirement {
            item_id: OTHER_EFFECT_ITEM,
            active: false,
        },
    ];
    quest.start.required_morph_id = NonZeroU32::new(40);
    let player = player(Vec::new(), 1);
    let mut effects = PlayerEffects::default();

    assert!(!is_available_with_effects(
        &player,
        &effects,
        &quest,
        &definitions,
        scripts(),
        environment(100),
    ));
    crate::effects::apply_consume_effect(
        PlayerState::default(),
        &mut effects,
        consume_effect(EFFECT_ITEM, Some(40)),
        100,
    );
    assert!(is_available_with_effects(
        &player,
        &effects,
        &quest,
        &definitions,
        scripts(),
        environment(100),
    ));
    crate::effects::apply_consume_effect(
        PlayerState::default(),
        &mut effects,
        consume_effect(OTHER_EFFECT_ITEM, None),
        100,
    );
    assert!(!is_available_with_effects(
        &player,
        &effects,
        &quest,
        &definitions,
        scripts(),
        environment(100),
    ));
}
