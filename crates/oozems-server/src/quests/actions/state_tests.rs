use crate::quests::test_support::*;

#[test]
fn start_record_actions_are_atomic_and_only_successful_acceptance_writes() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.record_conditions.push(record_condition(
        100,
        vec![QuestRecordPredicate::Equal("gate".to_owned())],
    ));
    quest.start_actions.record_writes.push(QuestRecordWrite {
        quest_id: 100,
        index: 0,
        value: "started".to_owned(),
    });
    let mut player = player(Vec::new(), 1);
    crate::quest_records::set(&mut player, 100, 0, "gate".to_owned()).expect("gate record");
    crate::quest_records::set(&mut player, 100, 7, "stale".to_owned()).expect("stale record");

    let declined = select_choice(
        player.clone(),
        &quest,
        1,
        DECLINE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("decline quest");
    assert_eq!(
        crate::quest_records::get(&declined.player, 100, 0),
        Some("gate")
    );

    let mut unavailable = player.clone();
    crate::quest_records::set(&mut unavailable, 100, 0, "closed".to_owned()).expect("closed gate");
    let unchanged_unavailable = unavailable.clone();
    assert!(matches!(
        select_choice(
            unavailable,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
    assert_eq!(
        crate::quest_records::get(&unchanged_unavailable, 100, 0),
        Some("closed")
    );

    let mut blocked = quest.clone();
    blocked.start_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_B,
        count: 1,
        expiration: None,
    });
    blocked.start_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_C,
        count: 1,
        expiration: None,
    });
    let original = player.clone();
    assert!(
        select_choice(
            player,
            &blocked,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .is_err()
    );
    assert_eq!(crate::quest_records::get(&original, 100, 0), Some("gate"));
    assert_eq!(crate::quest_records::get(&original, 100, 7), Some("stale"));

    let accepted = select_choice(
        original,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("accept record quest");
    assert_eq!(
        crate::quest_records::get(&accepted.player, 100, 0),
        Some("started")
    );
    assert_eq!(crate::quest_records::get(&accepted.player, 100, 7), None);
}

#[test]
fn script_record_action_rolls_back_with_a_failed_merged_action() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.script = Some("record_rollback".to_owned());
    let scripts = script_catalog(
        r#"
                [[scripts]]
                name = "record_rollback"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 1332005
                delta = 1

                [[scripts.actions]]
                type = "set_record"
                quest_id = 900
                index = 7
                value = "written"
            "#,
        &quest,
        &definitions,
    );
    let original = player(Vec::new(), 0);

    assert!(
        select_choice(
            original.clone(),
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            &scripts,
            1_000,
        )
        .is_err()
    );
    assert_eq!(crate::quest_records::get(&original, 900, 7), None);
    assert_eq!(progress(&original, 100), QuestProgress::NotStarted);
}

#[test]
fn quest_skill_changes_use_independent_maxima_without_sp_or_downgrades() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start_actions.skill_changes = vec![
        skill_change(1_121_010, 3, 30, vec![112]),
        skill_change(1_121_002, 10, 15, vec![112]),
    ];
    let mut player = player(Vec::new(), 1);
    player.stats.as_mut().expect("stats").job_id = 112;
    player.skill_points = 7;
    player.learned_skills = vec![
        LearnedSkill {
            skill_id: 1_121_002,
            level: 20,
            master_level: 10,
        },
        LearnedSkill {
            skill_id: 1_121_010,
            level: 5,
            master_level: 20,
        },
    ];

    let accepted = select_choice(
        player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("quest skill changes");

    assert_eq!(accepted.player.skill_points, 7);
    assert_eq!(
        accepted
            .player
            .learned_skills
            .iter()
            .map(|skill| (skill.skill_id, skill.level, skill.master_level))
            .collect::<Vec<_>>(),
        vec![(1_121_002, 20, 15), (1_121_010, 5, 30)]
    );
}

#[test]
fn quest_skill_eligibility_uses_exact_jobs_and_beginner_family_bypass() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start_actions.skill_changes = vec![
        skill_change(2_321_003, 1, 10, vec![232]),
        skill_change(1_003, 1, 1, Vec::new()),
    ];
    let mut player = player(Vec::new(), 1);
    player.stats.as_mut().expect("stats").job_id = 112;

    let accepted = select_choice(
        player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("eligible beginner skill");

    assert_eq!(accepted.player.learned_skills.len(), 1);
    assert_eq!(accepted.player.learned_skills[0].skill_id, 1_003);
    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
}

#[test]
fn quest_skill_removal_is_exact_idempotent_and_clears_only_its_binding() {
    let removed_skill_id = 1_007;
    let retained_skill_id = 1_008;
    let mut player = player(Vec::new(), 1);
    player.skill_points = 7;
    player.learned_skills = vec![
        LearnedSkill {
            skill_id: removed_skill_id,
            level: 3,
            master_level: 3,
        },
        LearnedSkill {
            skill_id: retained_skill_id,
            level: 1,
            master_level: 2,
        },
    ];
    player.key_bindings = vec![
        KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: removed_skill_id,
        },
        KeyBinding {
            code: "KeyB".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: retained_skill_id,
        },
        KeyBinding {
            code: "Space".to_owned(),
            action: KeyAction::Jump as i32,
            skill_id: 0,
        },
    ];
    let removal = skill_removal(removed_skill_id, vec![0, 100]);

    super::apply_skill_changes(&mut player, std::slice::from_ref(&removal));
    let once = player.clone();
    super::apply_skill_changes(&mut player, &[removal]);

    assert_eq!(player, once);
    assert_eq!(player.skill_points, 7);
    assert_eq!(
        player
            .learned_skills
            .iter()
            .map(|skill| skill.skill_id)
            .collect::<Vec<_>>(),
        vec![retained_skill_id]
    );
    assert_eq!(
        player
            .key_bindings
            .iter()
            .map(|binding| (binding.code.as_str(), binding.skill_id, binding.action))
            .collect::<Vec<_>>(),
        vec![
            ("KeyB", retained_skill_id, KeyAction::Unspecified as i32),
            ("Space", 0, KeyAction::Jump as i32),
        ]
    );
}

#[test]
fn later_item_and_meso_failures_roll_back_quest_skill_changes() {
    let definitions = item_definitions();
    let original = player(Vec::new(), 1);
    for failure in ["item", "mesos"] {
        let mut quest = quest(100);
        quest.start_actions.skill_changes = vec![skill_change(1_003, 1, 1, Vec::new())];
        if failure == "item" {
            quest.start_actions.fixed_items.push(QuestItemDelta {
                item_id: ITEM_A,
                count: -1,
                expiration: None,
            });
        } else {
            quest.start_actions.money = -1;
        }

        assert!(
            select_choice(
                original.clone(),
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                100,
            )
            .is_err()
        );
        assert!(original.learned_skills.is_empty());
        assert_eq!(progress(&original, quest.id), QuestProgress::NotStarted);
    }
}

#[test]
fn later_item_and_meso_failures_roll_back_skill_and_binding_removal() {
    let definitions = item_definitions();
    let mut original = player(Vec::new(), 1);
    original.learned_skills.push(LearnedSkill {
        skill_id: 1_007,
        level: 3,
        master_level: 3,
    });
    original.key_bindings.push(KeyBinding {
        code: "KeyA".to_owned(),
        action: KeyAction::Unspecified as i32,
        skill_id: 1_007,
    });
    for failure in ["item", "mesos"] {
        let mut quest = quest(100);
        quest.start_actions.skill_changes = vec![skill_removal(1_007, vec![0])];
        if failure == "item" {
            quest.start_actions.fixed_items.push(QuestItemDelta {
                item_id: ITEM_A,
                count: -1,
                expiration: None,
            });
        } else {
            quest.start_actions.money = -1;
        }

        assert!(
            select_choice(
                original.clone(),
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                100,
            )
            .is_err()
        );
        assert_eq!(original.learned_skills[0].skill_id, 1_007);
        assert_eq!(original.key_bindings[0].skill_id, 1_007);
        assert_eq!(progress(&original, quest.id), QuestProgress::NotStarted);
    }
}

#[test]
fn quest_state_action_reset_removes_progress_timestamps_and_owning_record() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer
        .start_actions
        .quest_state_actions
        .push(QuestStateAction {
            quest_id: 200,
            state: QuestStateActionState::NotStarted,
        });
    let mut original = player(Vec::new(), 1);
    original.quests.push(PlayerQuest {
        quest_id: 200,
        status: QuestStatus::Completed as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 9,
        }],
        accepted_at_unix_ms: 100,
        completed_at_unix_ms: 200,
        dialogue_step: 3,
        completion_quiz_passed: true,
    });
    crate::quest_records::set(&mut original, 200, 0, "owned".to_owned())
        .expect("target-owned record");
    crate::quest_records::set(&mut original, 900, 0, "redirected".to_owned())
        .expect("redirected helper record");

    let accepted = select_choice(
        original,
        &producer,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("accept producer");

    assert_eq!(progress(&accepted.player, 200), QuestProgress::NotStarted);
    assert!(
        !accepted
            .player
            .quests
            .iter()
            .any(|entry| entry.quest_id == 200)
    );
    assert_eq!(crate::quest_records::get(&accepted.player, 200, 0), None);
    assert_eq!(
        crate::quest_records::get(&accepted.player, 900, 0),
        Some("redirected")
    );
}

#[test]
fn quest_state_action_start_replaces_stale_progress_and_timestamps() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer
        .start_actions
        .quest_state_actions
        .push(QuestStateAction {
            quest_id: 200,
            state: QuestStateActionState::Started,
        });
    let mut original = player(Vec::new(), 1);
    original.quests.push(PlayerQuest {
        quest_id: 200,
        status: QuestStatus::Completed as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 9,
        }],
        accepted_at_unix_ms: 100,
        completed_at_unix_ms: 200,
        dialogue_step: 3,
        completion_quiz_passed: true,
    });
    crate::quest_records::set(&mut original, 200, 0, "preserved".to_owned())
        .expect("target record");

    let accepted = select_choice(
        original,
        &producer,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("accept producer");
    let target = accepted
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == 200)
        .expect("started target");

    assert_eq!(target.status, QuestStatus::Started as i32);
    assert!(target.mob_progress.is_empty());
    assert_eq!(target.accepted_at_unix_ms, 1_000);
    assert_eq!(target.completed_at_unix_ms, 0);
    assert_eq!(
        crate::quest_records::get(&accepted.player, 200, 0),
        Some("preserved")
    );
}

#[test]
fn quest_state_action_completion_preserves_only_a_valid_acceptance_timestamp() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer.start_actions.quest_state_actions = vec![
        QuestStateAction {
            quest_id: 200,
            state: QuestStateActionState::Completed,
        },
        QuestStateAction {
            quest_id: 300,
            state: QuestStateActionState::Completed,
        },
        QuestStateAction {
            quest_id: 400,
            state: QuestStateActionState::Completed,
        },
    ];
    let mut original = player(Vec::new(), 1);
    original.quests.push(PlayerQuest {
        quest_id: 200,
        status: QuestStatus::Started as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 9,
        }],
        accepted_at_unix_ms: 123,
        completed_at_unix_ms: 0,
        dialogue_step: 3,
        completion_quiz_passed: true,
    });
    original.quests.push(PlayerQuest {
        quest_id: 300,
        status: QuestStatus::Completed as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 5,
        }],
        accepted_at_unix_ms: 456,
        completed_at_unix_ms: 789,
        dialogue_step: 3,
        completion_quiz_passed: true,
    });

    let accepted = select_choice(
        original,
        &producer,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("accept producer");
    let existing = accepted
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == 200)
        .expect("completed existing target");
    let completed = accepted
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == 300)
        .expect("replaced completed target");
    let new = accepted
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == 400)
        .expect("completed new target");

    assert_eq!(existing.status, QuestStatus::Completed as i32);
    assert!(existing.mob_progress.is_empty());
    assert_eq!(existing.accepted_at_unix_ms, 123);
    assert_eq!(existing.completed_at_unix_ms, 1_000);
    assert!(completed.mob_progress.is_empty());
    assert_eq!(completed.accepted_at_unix_ms, 456);
    assert_eq!(completed.completed_at_unix_ms, 1_000);
    assert_eq!(new.status, QuestStatus::Completed as i32);
    assert_eq!(new.accepted_at_unix_ms, 1_000);
    assert_eq!(new.completed_at_unix_ms, 1_000);
}

#[test]
fn quest_state_action_does_not_run_target_rewards() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer
        .start_actions
        .quest_state_actions
        .push(QuestStateAction {
            quest_id: 200,
            state: QuestStateActionState::Completed,
        });
    let mut target = quest(200);
    target.completion_actions.money = 500;
    target.completion.script = Some("target_script_must_not_run".to_owned());
    target
        .completion_actions
        .selectable_items
        .push(QuestSelectableItemReward {
            item_id: ITEM_A,
            count: 1,
            expiration: None,
            eligibility: QuestRewardEligibility::default(),
        });

    let accepted = select_choice_in_environment(
        player(Vec::new(), 1),
        &producer,
        &[&producer, &target],
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        environment(1_000),
    )
    .expect("accept producer");

    assert_eq!(progress(&accepted.player, 100), QuestProgress::Started);
    assert_eq!(progress(&accepted.player, 200), QuestProgress::Completed);
    assert_eq!(accepted.player.mesos, 0);
}

#[test]
fn quest_state_action_rolls_back_when_a_later_resource_action_fails() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer.start_actions.quest_state_actions = vec![QuestStateAction {
        quest_id: 200,
        state: QuestStateActionState::Completed,
    }];
    producer.start_actions.money = -1;
    let mut original = player(Vec::new(), 1);
    original.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(200, QuestStatus::Started)
    });

    assert!(
        select_choice(
            original.clone(),
            &producer,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .is_err()
    );
    assert_eq!(progress(&original, 100), QuestProgress::NotStarted);
    assert_eq!(progress(&original, 200), QuestProgress::Started);
}

#[test]
fn script_quest_status_action_uses_the_same_state_transform() {
    let definitions = item_definitions();
    let mut producer = quest(100);
    producer.start.script = Some("set_target_started".to_owned());
    let target = quest(200);
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("quest-scripts.toml");
    fs::write(
        &path,
        r#"
                [[scripts]]
                name = "set_target_started"

                [[scripts.actions]]
                type = "set_quest_status"
                quest_id = 200
                state = "started"
            "#,
    )
    .expect("write quest script");
    let scripts =
        QuestScriptCatalog::load(&path, [&producer, &target], &BTreeSet::new(), &definitions)
            .expect("quest status script catalog");
    let mut original = player(Vec::new(), 1);
    original.quests.push(PlayerQuest {
        quest_id: 200,
        status: QuestStatus::Completed as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 9,
        }],
        accepted_at_unix_ms: 100,
        completed_at_unix_ms: 200,
        dialogue_step: 3,
        completion_quiz_passed: true,
    });

    let accepted = select_choice(
        original,
        &producer,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        &scripts,
        1_000,
    )
    .expect("accept scripted producer");
    let target = accepted
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == 200)
        .expect("script-started target");

    assert_eq!(target.status, QuestStatus::Started as i32);
    assert!(target.mob_progress.is_empty());
    assert_eq!(target.accepted_at_unix_ms, 1_000);
    assert_eq!(target.completed_at_unix_ms, 0);
}
