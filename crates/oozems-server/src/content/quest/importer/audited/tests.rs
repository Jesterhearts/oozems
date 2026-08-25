use crate::content::quest::importer::test_support::*;

#[test]
fn selected_mob_metadata_fails_closed_outside_the_audited_records() {
    let info = property("3954");
    add_integer(&info, "selectedMob", 1);
    assert!(matches!(
        super::retain_audited_stray_selected_mob(3_954, &info),
        Err(QuestContentError::Invalid { .. })
    ));

    let unaudited = property("100");
    add_integer(&unaudited, "selectedMob", 1);
    assert!(matches!(
        read_info(100, &unaudited),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "quest info mechanic"
    ));
}

#[test]
fn audited_4944_action_alias_shape_is_exact_and_drift_is_rejected() {
    let action = quest_4944_action();
    validate_audited_4944_action(4_960, &action).expect("exact audited Act/4944 source");

    let completion = child(&action, "1");
    add_integer(&completion, "money", 1);
    assert!(matches!(
        validate_audited_4944_action(4_960, &action),
        Err(QuestContentError::Invalid {
            quest_id: 4_960,
            ..
        })
    ));
}

#[test]
fn quest_10272_exact_negative_removals_ignore_only_the_audited_prop_metadata() {
    let (action, completion_check, say) = quest_10272_sources();
    let corrections = audited_action_corrections(10_272, &action, &completion_check, Some(&say))
        .expect("exact quest 10272 evidence");
    let completion_action = child(&action, "1");
    let imported = read_action_items_with_corrections(
        10_272,
        "1",
        &completion_action,
        &BTreeSet::from([4_032_280, 4_032_283]),
        &BTreeSet::new(),
        &corrections,
    )
    .expect("audited fixed removals");

    assert_eq!(
        imported.fixed,
        vec![
            QuestItemDelta {
                item_id: 4_032_283,
                count: -10,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 4_032_280,
                count: -10,
                expiration: None,
            },
        ]
    );
    assert!(imported.selectable.is_empty());
    assert_eq!(
        imported.retained_fields,
        vec![
            "act/1/item/0/prop=-1".to_owned(),
            "act/1/item/1/prop=-1".to_owned(),
        ]
    );
    let completion = read_completion_requirements(
        10_272,
        &completion_check,
        &QuestInfo::default(),
        &BTreeSet::from([4_032_280, 4_032_283]),
        &BTreeSet::new(),
    )
    .expect("quest 10272 completion requirements");
    assert_eq!(completion.script.as_deref(), Some("q10272e"));
    assert_eq!(
        completion
            .items
            .iter()
            .map(|item| (item.item_id, item.condition))
            .collect::<Vec<_>>(),
        vec![
            (
                4_032_280,
                QuestItemCondition::AtLeast(NonZeroU32::new(10).expect("positive")),
            ),
            (
                4_032_283,
                QuestItemCondition::AtLeast(NonZeroU32::new(10).expect("positive")),
            ),
        ]
    );
}

#[test]
fn quest_10272_audited_item_metadata_fails_closed_on_source_drift() {
    let (action, completion_check, say) = quest_10272_sources();
    add_integer(
        &child(&child(&child(&action, "1"), "item"), "0"),
        "count",
        -9,
    );
    assert!(audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err());

    let (action, completion_check, say) = quest_10272_sources();
    add_integer(&completion_check, "extra", 1);
    assert!(audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err());

    let (action, completion_check, say) = quest_10272_sources();
    add_integer(&child(&child(&say, "0"), "yes"), "extra", 1);
    assert!(audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err());
    assert!(audited_action_corrections(10_272, &action, &completion_check, None).is_err());

    let corrections = audited_action_corrections(100, &action, &completion_check, Some(&say))
        .expect("other quests have no correction");
    assert!(
        read_action_items_with_corrections(
            100,
            "1",
            &child(&action, "1"),
            &BTreeSet::from([4_032_280, 4_032_283]),
            &BTreeSet::new(),
            &corrections,
        )
        .is_err(),
        "negative selectable entries remain invalid for every other quest",
    );
}

#[test]
fn audited_act_field_enter_must_exactly_duplicate_check_9866() {
    let action = property("quest");
    let action_phase = property("0");
    let action_maps = property("fieldEnter");
    add_integer(&action_maps, "0", 102_000_000);
    add_child(&action_phase, &action_maps);
    add_child(&action, &action_phase);
    let check = property("0");
    let check_maps = property("fieldEnter");
    add_integer(&check_maps, "0", 102_000_000);
    add_child(&check, &check_maps);

    let (_, retained) = read_action_phase_with_skills(
        9_866,
        &action,
        "0",
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::from([9_866]),
        Some(&check),
    )
    .expect("audited duplicate map metadata");
    assert_eq!(retained, vec!["act/0/fieldEnter"]);

    let mismatched = property("quest");
    let phase = property("0");
    let maps = property("fieldEnter");
    add_integer(&maps, "0", 102_000_001);
    add_child(&phase, &maps);
    add_child(&mismatched, &phase);
    assert!(matches!(
        read_action_phase_with_skills(
            9_866,
            &mismatched,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([9_866]),
            Some(&check),
        ),
        Err(QuestContentError::Invalid { .. })
    ));
    assert!(matches!(
        read_action_phase_with_skills(
            9_865,
            &action,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([9_865]),
            Some(&check),
        ),
        Err(QuestContentError::Unsupported { category, .. }) if category == "map action"
    ));
}

#[test]
fn only_the_two_audited_nested_metadata_trees_are_retained() {
    let checks = property("Check");
    let quest = property("4940");
    add_child(&quest, &property("0"));
    add_child(&quest, &property("1"));
    let nested = property("4961");
    let start = property("0");
    add_integer(&start, "npc", 9_201_077);
    let prerequisites = property("quest");
    let prerequisite = property("0");
    add_integer(&prerequisite, "id", 4_954);
    add_integer(&prerequisite, "state", 1);
    add_child(&prerequisites, &prerequisite);
    add_child(&start, &prerequisites);
    add_child(&nested, &start);
    let completion = property("1");
    add_integer(&completion, "npc", 9_201_077);
    let mobs = property("mob");
    let mob = property("0");
    add_integer(&mob, "id", 9_400_558);
    add_integer(&mob, "count", 50);
    add_child(&mobs, &mob);
    add_child(&completion, &mobs);
    add_child(&nested, &completion);
    add_child(&quest, &nested);
    add_child(&checks, &quest);

    assert_eq!(
        super::validate_check_phase_tree(
            4_940,
            &checks,
            &quest,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .expect("audited nested check"),
        vec!["check/4961"]
    );
    add_integer(&start, "lvmin", 1);
    assert!(matches!(
        super::validate_check_phase_tree(
            4_940,
            &checks,
            &quest,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Invalid { .. })
    ));
    let wrong_quest = property("4941");
    add_child(&wrong_quest, &nested);
    assert!(matches!(
        super::validate_check_phase_tree(
            4_941,
            &checks,
            &wrong_quest,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Unsupported { .. })
    ));

    let info_root = property("QuestInfo");
    let canonical = property("4963");
    add_string(&canonical, "name", "Dinner Fixins'");
    add_child(&info_root, &canonical);
    let info = property("8833");
    let nested = property("4963");
    add_string(&nested, "0", "Status zero");
    add_string(&nested, "1", "Status one");
    add_string(&nested, "2", "Status two");
    add_integer(&nested, "area", 30);
    add_string(&nested, "name", "Spot On");
    add_integer(&nested, "order", 1);
    add_string(&nested, "parent", "Spot On parent");
    add_child(&info, &nested);

    let parsed =
        read_info_with_skills(8_833, &info, &info_root, &BTreeSet::new(), &BTreeMap::new())
            .expect("audited nested quest info");
    assert_eq!(parsed.retained_metadata_fields, vec!["questInfo/4963"]);

    let malformed = property("8833");
    let nested = property("4963");
    add_string(&nested, "0", "Status zero");
    add_string(&nested, "1", "Status one");
    add_string(&nested, "2", "Status two");
    add_integer(&nested, "area", 30);
    add_string(&nested, "name", "Spot On");
    add_integer(&nested, "order", 1);
    add_string(&nested, "parent", "Spot On parent");
    add_integer(&nested, "extra", 1);
    add_child(&malformed, &nested);
    assert!(
        read_info_with_skills(
            8_833,
            &malformed,
            &info_root,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .is_err()
    );
}

#[test]
fn audited_completion_restoration_requires_exact_permanent_fixed_evidence() {
    let dialogue = QuestDialogue {
        completion: crate::content::QuestCompletionDialogue {
            lost: Some(QuestLostItemDialogue {
                prompt_pages: vec!["Did you lose it?".to_owned()],
                success_pages: vec!["Take another.".to_owned()],
                items: Vec::new(),
            }),
            ..crate::content::QuestCompletionDialogue::default()
        },
        ..QuestDialogue::default()
    };
    let exact = QuestActions {
        fixed_items: vec![QuestItemDelta {
            item_id: 4_031_890,
            count: 1,
            expiration: None,
        }],
        ..QuestActions::default()
    };
    let mapped = validate_lost_item_restoration_flow(
        2_208,
        &QuestCompletionRequirements::default(),
        &QuestActions::default(),
        &exact,
        &dialogue,
    )
    .expect("exact audited completion action");
    assert_eq!(mapped.len(), 1);
    assert_eq!(
        mapped[0].provenance,
        crate::content::QuestRestorationProvenance::AuditedCompletionGrant
    );

    for completion_actions in [
        QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: 4_031_890,
                count: 2,
                expiration: None,
            }],
            ..QuestActions::default()
        },
        QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: 4_031_890,
                count: 1,
                expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
            }],
            ..QuestActions::default()
        },
        QuestActions {
            conditional_items: vec![QuestConditionalItemReward {
                item_id: 4_031_890,
                count: 1,
                expiration: None,
                eligibility: QuestRewardEligibility {
                    job_mask: Some(1),
                    gender: None,
                },
            }],
            ..QuestActions::default()
        },
        QuestActions {
            weighted_items: vec![QuestWeightedItem {
                item_id: 4_031_890,
                count: 1,
                expiration: None,
                weight: 1,
                eligibility: QuestRewardEligibility::default(),
            }],
            ..QuestActions::default()
        },
        QuestActions {
            selectable_items: vec![QuestSelectableItemReward {
                item_id: 4_031_890,
                count: 1,
                expiration: None,
                eligibility: QuestRewardEligibility::default(),
            }],
            ..QuestActions::default()
        },
    ] {
        assert!(matches!(
            validate_lost_item_restoration_flow(
                2_208,
                &QuestCompletionRequirements::default(),
                &QuestActions::default(),
                &completion_actions,
                &dialogue,
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "audited lost-item restoration evidence"
        ));
    }
}

#[test]
fn quest_3310_exception_requires_its_exact_item_dialogue_and_output_objective() {
    let dialogue = QuestDialogue {
        completion: crate::content::QuestCompletionDialogue {
            lost: Some(QuestLostItemDialogue {
                prompt_pages: vec!["Did you lose #t4031698#?".to_owned()],
                success_pages: Vec::new(),
                items: Vec::new(),
            }),
            ..crate::content::QuestCompletionDialogue::default()
        },
        ..QuestDialogue::default()
    };
    let completion = QuestCompletionRequirements {
        items: vec![QuestItemRequirement {
            item_id: 4_031_709,
            condition: QuestItemCondition::AtLeast(
                NonZeroU32::new(1).expect("positive reactor output"),
            ),
        }],
        ..QuestCompletionRequirements::default()
    };
    let mapped = validate_lost_item_restoration_flow(
        3_310,
        &completion,
        &QuestActions::default(),
        &QuestActions::default(),
        &dialogue,
    )
    .expect("exact 3310 reactor-device exception");
    assert_eq!(mapped[0].item_id, 4_031_698);
    assert_eq!(
        mapped[0].provenance,
        crate::content::QuestRestorationProvenance::AuditedReactorDevice
    );

    let wrong_dialogue = QuestDialogue {
        completion: crate::content::QuestCompletionDialogue {
            lost: Some(QuestLostItemDialogue {
                prompt_pages: vec!["Did you lose #t4031709#?".to_owned()],
                success_pages: Vec::new(),
                items: Vec::new(),
            }),
            ..crate::content::QuestCompletionDialogue::default()
        },
        ..QuestDialogue::default()
    };
    assert!(
        validate_lost_item_restoration_flow(
            3_310,
            &completion,
            &QuestActions::default(),
            &QuestActions::default(),
            &wrong_dialogue,
        )
        .is_err()
    );
    assert!(
        validate_lost_item_restoration_flow(
            3_310,
            &QuestCompletionRequirements::default(),
            &QuestActions::default(),
            &QuestActions::default(),
            &dialogue,
        )
        .is_err()
    );
}
