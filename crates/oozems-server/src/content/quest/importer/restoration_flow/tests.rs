use crate::content::quest::importer::test_support::*;

#[test]
fn lost_item_restoration_requires_an_unambiguous_fixed_start_grant() {
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
    let completion = QuestCompletionRequirements {
        items: vec![QuestItemRequirement {
            item_id: 4_000_000,
            condition: QuestItemCondition::AtLeast(
                NonZeroU32::new(2).expect("positive requirement"),
            ),
        }],
        ..QuestCompletionRequirements::default()
    };

    let unmapped = validate_lost_item_restoration_flow(
        100,
        &completion,
        &QuestActions::default(),
        &QuestActions::default(),
        &dialogue,
    )
    .expect_err("an unmapped lost branch must remain unsupported");
    assert!(matches!(
        unmapped,
        QuestContentError::Unsupported { category, .. }
            if category == "lost-item restoration item mapping"
    ));
    let arbitrary_completion = QuestActions {
        fixed_items: vec![QuestItemDelta {
            item_id: 4_000_000,
            count: 1,
            expiration: None,
        }],
        ..QuestActions::default()
    };
    assert!(matches!(
        validate_lost_item_restoration_flow(
            100,
            &completion,
            &QuestActions::default(),
            &arbitrary_completion,
            &dialogue,
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "lost-item restoration item mapping"
    ));

    let fixed = QuestActions {
        fixed_items: vec![QuestItemDelta {
            item_id: 4_000_000,
            count: 3,
            expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
        }],
        ..QuestActions::default()
    };
    let mapped = validate_lost_item_restoration_flow(
        100,
        &completion,
        &fixed,
        &QuestActions::default(),
        &dialogue,
    )
    .expect("fixed matching start grant");
    assert_eq!(
        mapped,
        vec![crate::content::QuestRestorableItem {
            item_id: 4_000_000,
            target_count: 3,
            expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
            provenance: crate::content::QuestRestorationProvenance::InferredStartGrant,
            eligibility: crate::content::QuestRestorationEligibility {
                owner_state: crate::content::RequiredQuestState::Started,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[],
            },
        }]
    );
    let mapped_without_completion_objective = validate_lost_item_restoration_flow(
        100,
        &QuestCompletionRequirements::default(),
        &fixed,
        &QuestActions::default(),
        &dialogue,
    )
    .expect("a fixed start grant is sufficient provenance");
    assert_eq!(mapped_without_completion_objective, mapped);

    let contradictory_dialogue = QuestDialogue {
        completion: crate::content::QuestCompletionDialogue {
            lost: Some(QuestLostItemDialogue {
                prompt_pages: vec!["Did you lose #t4000001#?".to_owned()],
                success_pages: Vec::new(),
                items: Vec::new(),
            }),
            ..crate::content::QuestCompletionDialogue::default()
        },
        ..QuestDialogue::default()
    };
    assert!(matches!(
        validate_lost_item_restoration_flow(
            100,
            &QuestCompletionRequirements::default(),
            &fixed,
            &QuestActions::default(),
            &contradictory_dialogue,
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "lost-item restoration action ambiguity"
    ));

    let ambiguous_actions = [
        QuestActions {
            conditional_items: vec![QuestConditionalItemReward {
                item_id: 4_000_000,
                count: 2,
                expiration: None,
                eligibility: QuestRewardEligibility::default(),
            }],
            ..QuestActions::default()
        },
        QuestActions {
            weighted_items: vec![QuestWeightedItem {
                item_id: 4_000_000,
                count: 2,
                expiration: None,
                weight: 1,
                eligibility: QuestRewardEligibility::default(),
            }],
            ..QuestActions::default()
        },
        QuestActions {
            selectable_items: vec![QuestSelectableItemReward {
                item_id: 4_000_000,
                count: 2,
                expiration: None,
                eligibility: QuestRewardEligibility::default(),
            }],
            ..QuestActions::default()
        },
    ];
    for actions in ambiguous_actions {
        let error = validate_lost_item_restoration_flow(
            100,
            &completion,
            &actions,
            &QuestActions::default(),
            &dialogue,
        )
        .expect_err("conditional start grants must not become restoration grants");
        assert!(matches!(
            error,
            QuestContentError::Unsupported { category, .. }
                if category == "lost-item restoration action ambiguity"
        ));
    }
}
