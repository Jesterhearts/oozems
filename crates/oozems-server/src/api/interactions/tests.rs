use std::collections::HashMap;
use std::sync::Arc;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcAnimation;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcShopCurrency;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::Vec2;

use super::quest::QuestSelectionPlan;
use super::quest::QuestTransitionPlan;
use super::quest::active_quest_dialog;
use super::quest::active_quest_for_npc;
use super::quest::completed_restoration_for_npc;
use super::quest::format_gms_deadline;
use super::quest::lost_item_restoration_dialog;
use super::quest::npc_animation_event;
use super::quest::persist_quest_selection;
use super::quest::quest_offer_dialog;
use super::quest::selection_dialog;
use super::quest::validate_quest_npc_animation;
use super::shop::shop_view;
use super::shop::validate_shop_sale;
use super::validate_reach;
use crate::content::QuestActions;
use crate::content::QuestChoice;
use crate::content::QuestCompletionDialogue;
use crate::content::QuestCompletionRequirements;
use crate::content::QuestDefinition;
use crate::content::QuestDialogue;
use crate::content::QuestIncompleteDialogue;
use crate::content::QuestInfo;
use crate::content::QuestItemCondition;
use crate::content::QuestItemExpiration;
use crate::content::QuestItemRequirement;
use crate::content::QuestLostItemDialogue;
use crate::content::QuestQuestionSequence;
use crate::content::QuestQuestionStep;
use crate::content::QuestRestorableItem;
use crate::content::QuestRewardEligibility;
use crate::content::QuestRewardGender;
use crate::content::QuestSelectableItemReward;
use crate::content::QuestStartRequirements;
use crate::interactions::ShopCurrency;
use crate::interactions::ShopDefinition;
use crate::interactions::ShopOffer;
use crate::player_lock::PlayerLocks;
use crate::player_lock::acquire_player;

fn environment(now_unix_ms: u64) -> crate::quests::QuestEnvironment {
    crate::quests::QuestEnvironment {
        now_unix_ms,
        world_id: 0,
    }
}

fn active_dialog(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    scripts: &crate::quest_scripts::QuestScriptCatalog,
    now_unix_ms: u64,
) -> oozems_proto::v1::NpcDialogView {
    active_quest_dialog(
        player,
        &crate::effects::PlayerEffects::default(),
        quest,
        &[quest],
        item_definitions,
        scripts,
        environment(now_unix_ms),
    )
}

#[test]
fn npc_interaction_uses_the_authoritative_player_position() {
    let npc = Npc {
        position: Some(Vec2 { x: 100.0, y: 200.0 }),
        ..Npc::default()
    };
    let nearby = PlayerState {
        position: Some(Vec2 { x: 180.0, y: 250.0 }),
        ..PlayerState::default()
    };
    let far_away = PlayerState {
        position: Some(Vec2 { x: 421.0, y: 200.0 }),
        ..PlayerState::default()
    };

    assert!(validate_reach(&nearby, &npc).is_ok());
    assert!(validate_reach(&far_away, &npc).is_err());
}

#[test]
fn cash_point_shop_view_is_buy_only_on_the_server() {
    let shop = ShopDefinition {
        currency: ShopCurrency::CashPoints,
        offers: vec![ShopOffer {
            item_id: 5_000_001,
            buy_price: 250,
        }],
    };

    let view = shop_view(&shop, "Ooze");

    assert_eq!(
        NpcShopCurrency::try_from(view.currency),
        Ok(NpcShopCurrency::CashPoints)
    );
    assert_eq!(view.offers[0].buy_price, 250);
    assert_eq!(view.currency_name, "Ooze");
    assert!(validate_shop_sale(&shop).is_err());
    assert!(
        validate_shop_sale(&ShopDefinition {
            currency: ShopCurrency::Mesos,
            offers: Vec::new(),
        })
        .is_ok()
    );
    assert_eq!(
        NpcShopCurrency::try_from(
            shop_view(
                &ShopDefinition {
                    currency: ShopCurrency::Mesos,
                    offers: Vec::new(),
                },
                "Ooze",
            )
            .currency
        ),
        Ok(NpcShopCurrency::Mesos)
    );
}

#[test]
fn quest_npc_animation_is_bound_to_the_interacted_spawn_and_saved_revision() {
    let mut quest = QuestDefinition {
        id: 100,
        name: "Animation".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements {
            npc_id: Some(10),
            ..QuestCompletionRequirements::default()
        },
        start_actions: QuestActions::default(),
        completion_actions: QuestActions {
            npc_animation_action: Some("quest".to_owned()),
            ..QuestActions::default()
        },
        dialogue: QuestDialogue::default(),
        info: QuestInfo::default(),
    };
    let player = PlayerState {
        map_id: 1,
        revision: 9,
        quests: vec![PlayerQuest {
            quest_id: quest.id,
            status: QuestStatus::Started as i32,
            ..PlayerQuest::default()
        }],
        ..PlayerState::default()
    };
    let npc = Npc {
        spawn_id: 7,
        npc_id: 10,
        animations: vec![NpcAnimation {
            name: "quest".to_owned(),
            frames: vec![oozems_proto::v1::NpcFrame {
                delay_ms: 100,
                ..oozems_proto::v1::NpcFrame::default()
            }],
        }],
        ..Npc::default()
    };

    validate_quest_npc_animation(&player, &quest, &npc).expect("matching animation");
    let event = npc_animation_event(
        quest.completion_actions.npc_animation_action.clone(),
        &player,
        &npc,
    )
    .expect("NPC animation event");
    assert_eq!(
        (
            event.map_id,
            event.npc_spawn_id,
            event.npc_id,
            event.action_name.as_str(),
            event.player_revision,
        ),
        (1, 7, 10, "quest", 9)
    );

    quest.completion_actions.npc_animation_action = Some("missing".to_owned());
    assert!(validate_quest_npc_animation(&player, &quest, &npc).is_err());
}

#[tokio::test]
async fn failed_quest_persistence_cannot_produce_an_animation_event() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = crate::database::open_surreal_kv(directory.path(), 0)
        .await
        .expect("open database");
    let player = PlayerState {
        id: "animation-save-failure".to_owned(),
        map_id: 1,
        revision: i64::MAX as u64,
        ..PlayerState::default()
    };
    let npc = Npc {
        spawn_id: 7,
        npc_id: 10,
        ..Npc::default()
    };
    let locks = PlayerLocks::default();
    let guard = acquire_player(&locks, &player.id)
        .await
        .expect("player guard");

    let result = persist_quest_selection(
        &database,
        Arc::new(crate::effects::ActiveEffects::default()),
        Arc::new(crate::recovery::RecoveryTimers::default()),
        &guard,
        QuestSelectionPlan::PersistTransition(Box::new(QuestTransitionPlan {
            original: player.clone(),
            player,
            original_effects: crate::effects::PlayerEffects::default(),
            effects: crate::effects::PlayerEffects::default(),
            animation_action: Some("quest".to_owned()),
            activity: false,
        })),
        &npc,
        1_000,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn failed_restoration_persistence_keeps_the_saved_inventory_unchanged() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = crate::database::open_surreal_kv(directory.path(), 0)
        .await
        .expect("open database");
    let quest = QuestDefinition {
        id: 100,
        name: "Completed restoration".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements {
            npc_id: Some(10),
            ..QuestCompletionRequirements::default()
        },
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue {
            completion: QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Restore?".to_owned()],
                    success_pages: vec!["Restored".to_owned()],
                    items: vec![QuestRestorableItem {
                        item_id: 4_000_000,
                        target_count: 1,
                        expiration: None,
                        provenance:
                            crate::content::QuestRestorationProvenance::AuditedCompletionGrant,
                        eligibility: crate::content::QuestRestorationEligibility {
                            owner_state: crate::content::RequiredQuestState::Completed,
                            required_quests: &[],
                            forbidden_quests: &[],
                            absent_skill_ids: &[],
                            absent_item_ids: &[4_000_000],
                        },
                    }],
                }),
                ..QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        },
        info: QuestInfo::default(),
    };
    let definitions = [ItemDefinition {
        item_id: 4_000_000,
        name: "Quest item".to_owned(),
        stack_max: 10,
        ..ItemDefinition::default()
    }];
    let original = PlayerState {
        id: "restoration-save-failure".to_owned(),
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
        stats: Some(CharacterStats {
            hp: 1,
            max_hp: 1,
            mp: 1,
            max_mp: 1,
            experience_required: 1,
            ..CharacterStats::default()
        }),
        inventory: Some(InventoryState {
            capacity: 1,
            ..InventoryState::default()
        }),
        key_bindings: crate::keymap::default_bindings(),
        quests: vec![PlayerQuest {
            quest_id: quest.id,
            status: QuestStatus::Completed as i32,
            ..PlayerQuest::default()
        }],
        ..PlayerState::default()
    };
    let locks = PlayerLocks::default();
    let guard = acquire_player(&locks, &original.id)
        .await
        .expect("player guard");
    let saved = crate::database::save_player(&database, &guard, &original)
        .await
        .expect("save original player");
    let mut restored =
        crate::quests::restore_lost_quest_items(saved.clone(), &quest, &definitions, 200)
            .expect("stage completed restoration")
            .player;
    restored.revision = i64::MAX as u64;

    assert!(
        persist_quest_selection(
            &database,
            Arc::new(crate::effects::ActiveEffects::default()),
            Arc::new(crate::recovery::RecoveryTimers::default()),
            &guard,
            QuestSelectionPlan::PersistTransition(Box::new(QuestTransitionPlan {
                original: saved,
                player: restored,
                original_effects: crate::effects::PlayerEffects::default(),
                effects: crate::effects::PlayerEffects::default(),
                animation_action: None,
                activity: false,
            },)),
            &Npc::default(),
            200,
        )
        .await
        .is_err()
    );
    let player_id =
        crate::database::PlayerId::parse("restoration-save-failure").expect("valid player ID");
    let loaded = crate::database::load_player(&database, &player_id)
        .await
        .expect("load saved player")
        .expect("saved player");
    assert_eq!(
        crate::items::count_inventory_item(
            loaded.inventory.as_ref().expect("inventory"),
            &definitions,
            4_000_000,
        )
        .expect("count quest item"),
        0
    );
}

#[test]
fn offer_dialog_projects_start_questions_without_changing_ordinary_offers() {
    let mut quest = QuestDefinition {
        id: 100,
        name: "Question offer".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements::default(),
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue {
            offer_pages: vec!["Ordinary offer".to_owned()],
            ..QuestDialogue::default()
        },
        info: QuestInfo::default(),
    };

    let ordinary = quest_offer_dialog(&quest);
    assert_eq!(ordinary.pages, vec!["Ordinary offer"]);
    assert_eq!(ordinary.choices.len(), 2);
    assert_eq!(
        ordinary
            .choices
            .iter()
            .map(|choice| NpcDialogChoiceKind::try_from(choice.kind))
            .collect::<Result<Vec<_>, _>>()
            .expect("ordinary choice kinds"),
        vec![
            NpcDialogChoiceKind::AcceptQuest,
            NpcDialogChoiceKind::DeclineQuest,
        ]
    );

    quest.dialogue.start_question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![QuestQuestionStep {
            archive_index: 0,
            prompt: "Pick one".to_owned(),
            choices: vec![
                QuestChoice {
                    id: 0,
                    label: "First".to_owned(),
                },
                QuestChoice {
                    id: 1,
                    label: "Second".to_owned(),
                },
            ],
            correct_choice_id: 1,
            continuation_pages: Vec::new(),
            failure_pages: HashMap::from([(0, vec!["Wrong".to_owned()])]),
        }],
        trailing_pages: Vec::new(),
    });
    let question = quest_offer_dialog(&quest);

    assert_eq!(question.pages, vec!["Pick one"]);
    assert_eq!(
        question
            .choices
            .iter()
            .map(|choice| (choice.choice_id, choice.label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (
                crate::quests::answer_choice_id(crate::quests::QuestQuestionPhase::Start, 0, 0,)
                    .expect("first answer choice"),
                "First",
            ),
            (
                crate::quests::answer_choice_id(crate::quests::QuestQuestionPhase::Start, 0, 1,)
                    .expect("second answer choice"),
                "Second",
            ),
        ]
    );
    assert!(question.choices.iter().all(|choice| {
        NpcDialogChoiceKind::try_from(choice.kind) == Ok(NpcDialogChoiceKind::Answer)
            && choice.choice_id != crate::quests::ACCEPT_CHOICE_ID
            && choice.choice_id != crate::quests::DECLINE_CHOICE_ID
            && choice.choice_id != crate::quests::COMPLETE_CHOICE_ID
            && choice.choice_id != crate::quests::RESTORE_ITEMS_CHOICE_ID
            && choice.choice_id < 0x8000_0000
    }));
}

#[test]
fn pending_start_decision_projects_normal_accept_and_decline_choices() {
    let quest = QuestDefinition {
        id: 100,
        name: "Question offer".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements::default(),
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue::default(),
        info: QuestInfo::default(),
    };

    let dialog = selection_dialog(
        &PlayerState::default(),
        &quest,
        &[],
        vec!["Passed".to_owned()],
        Some(crate::quests::QuestNextInteraction::StartDecision),
    );

    assert_eq!(dialog.pages, vec!["Passed"]);
    assert_eq!(
        dialog
            .choices
            .iter()
            .map(|choice| (choice.choice_id, NpcDialogChoiceKind::try_from(choice.kind)))
            .collect::<Vec<_>>(),
        vec![
            (
                crate::quests::ACCEPT_CHOICE_ID,
                Ok(NpcDialogChoiceKind::AcceptQuest),
            ),
            (
                crate::quests::DECLINE_CHOICE_ID,
                Ok(NpcDialogChoiceKind::DeclineQuest),
            ),
        ]
    );
}

#[test]
fn pending_active_interaction_has_priority_over_the_lowest_quest_id() {
    let ordinary = QuestDefinition {
        id: 100,
        name: "Ordinary".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements {
            npc_id: Some(2),
            ..QuestCompletionRequirements::default()
        },
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue::default(),
        info: QuestInfo::default(),
    };
    let pending = QuestDefinition {
        id: 200,
        name: "Pending".to_owned(),
        ..ordinary.clone()
    };
    let mut player = PlayerState {
        quests: vec![
            PlayerQuest {
                quest_id: ordinary.id,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            },
            PlayerQuest {
                quest_id: pending.id,
                status: QuestStatus::Started as i32,
                dialogue_step: 2,
                ..PlayerQuest::default()
            },
        ],
        ..PlayerState::default()
    };
    let quests = [&ordinary, &pending];

    assert_eq!(
        active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
        Some(pending.id)
    );
    player.quests[1].dialogue_step = 0;
    assert_eq!(
        active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
        Some(ordinary.id)
    );
    player.quests[1].completion_quiz_passed = true;
    assert_eq!(
        active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
        Some(pending.id)
    );
}

#[test]
fn completed_restoration_is_npc_bound_and_follows_active_quest_priority() {
    let active = QuestDefinition {
        id: 100,
        name: "Active".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements {
            npc_id: Some(2),
            ..QuestCompletionRequirements::default()
        },
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue::default(),
        info: QuestInfo::default(),
    };
    let completed = QuestDefinition {
        id: 200,
        name: "Completed restoration".to_owned(),
        dialogue: QuestDialogue {
            completion: QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Restore the lost item?".to_owned()],
                    success_pages: vec!["Restored".to_owned()],
                    items: vec![QuestRestorableItem {
                        item_id: 4_000_000,
                        target_count: 1,
                        expiration: None,
                        provenance:
                            crate::content::QuestRestorationProvenance::AuditedCompletionGrant,
                        eligibility: crate::content::QuestRestorationEligibility {
                            owner_state: crate::content::RequiredQuestState::Completed,
                            required_quests: &[],
                            forbidden_quests: &[],
                            absent_skill_ids: &[],
                            absent_item_ids: &[4_000_000],
                        },
                    }],
                }),
                ..QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        },
        ..active.clone()
    };
    let definitions = [ItemDefinition {
        item_id: 4_000_000,
        name: "Quest item".to_owned(),
        stack_max: 10,
        ..ItemDefinition::default()
    }];
    let player = PlayerState {
        inventory: Some(InventoryState {
            capacity: 1,
            ..InventoryState::default()
        }),
        quests: vec![
            PlayerQuest {
                quest_id: active.id,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            },
            PlayerQuest {
                quest_id: completed.id,
                status: QuestStatus::Completed as i32,
                ..PlayerQuest::default()
            },
        ],
        ..PlayerState::default()
    };
    let quests = [&active, &completed];

    assert_eq!(
        active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
        Some(active.id),
        "an active quest remains the interaction selected first"
    );
    assert_eq!(
        completed_restoration_for_npc(&player, &quests, 2, &definitions, 200).map(|quest| quest.id),
        Some(completed.id)
    );
    assert!(
        completed_restoration_for_npc(&player, &quests, 3, &definitions, 200).is_none(),
        "the completed dialogue is available only at its completion NPC"
    );

    let completed_only = [&completed];
    let selected = completed_restoration_for_npc(&player, &completed_only, 2, &definitions, 200)
        .expect("completed restoration before repeat/start offers");
    let dialog = lost_item_restoration_dialog(&player, selected, &definitions, 200)
        .expect("completed restoration dialog");
    assert_eq!(dialog.quest_id, completed.id);
    assert_eq!(
        dialog.choices[0].choice_id,
        crate::quests::RESTORE_ITEMS_CHOICE_ID
    );
}

#[test]
fn active_ordinary_quest_shows_progress_then_complete_choice() {
    let quest = QuestDefinition {
        id: 100,
        name: "Collection".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements {
            items: vec![QuestItemRequirement {
                item_id: 4_000_000,
                condition: QuestItemCondition::AtLeast(
                    std::num::NonZeroU32::new(2).expect("positive requirement"),
                ),
            }],
            ..QuestCompletionRequirements::default()
        },
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue {
            completion: QuestCompletionDialogue {
                pages: vec!["You have everything.".to_owned()],
                incomplete: QuestIncompleteDialogue {
                    item_pages: vec!["Bring me the shells.".to_owned()],
                    ..QuestIncompleteDialogue::default()
                },
                ..QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        },
        info: QuestInfo::default(),
    };
    let definitions = vec![ItemDefinition {
        item_id: 4_000_000,
        name: "Blue Snail Shell".to_owned(),
        stack_max: 10,
        ..ItemDefinition::default()
    }];
    let mut player = PlayerState {
        inventory: Some(InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: 4_000_000,
                quantity: 1,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        }),
        quests: vec![PlayerQuest {
            quest_id: 100,
            status: QuestStatus::Started as i32,
            ..PlayerQuest::default()
        }],
        ..PlayerState::default()
    };
    let scripts = crate::quest_scripts::QuestScriptCatalog::default();

    let incomplete = active_dialog(&player, &quest, &definitions, &scripts, 200);
    assert!(incomplete.choices.is_empty());
    assert_eq!(incomplete.pages[0], "Bring me the shells.");
    assert!(incomplete.pages[1].contains("Blue Snail Shell: 1/2"));

    player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
    let ready = active_dialog(&player, &quest, &definitions, &scripts, 200);
    assert_eq!(ready.pages, vec!["You have everything."]);
    assert_eq!(ready.choices.len(), 1);
    assert_eq!(
        NpcDialogChoiceKind::try_from(ready.choices[0].kind),
        Ok(NpcDialogChoiceKind::CompleteQuest)
    );
}

#[test]
fn selectable_reward_dialog_uses_authoritative_names_counts_and_eligibility() {
    let warrior_mask = 1 << 1;
    let quest = QuestDefinition {
        id: 100,
        name: "Choice".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements::default(),
        start_actions: QuestActions::default(),
        completion_actions: QuestActions {
            selectable_items: vec![
                QuestSelectableItemReward {
                    item_id: 1_332_005,
                    count: 2,
                    expiration: Some(QuestItemExpiration::RelativeMilliseconds(3_600_000)),
                    eligibility: QuestRewardEligibility {
                        job_mask: Some(warrior_mask),
                        gender: Some(QuestRewardGender::Male),
                    },
                },
                QuestSelectableItemReward {
                    item_id: 1_332_007,
                    count: 1,
                    expiration: None,
                    eligibility: QuestRewardEligibility {
                        job_mask: Some(warrior_mask),
                        gender: Some(QuestRewardGender::Female),
                    },
                },
                QuestSelectableItemReward {
                    item_id: 4_000_000,
                    count: 3,
                    expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
                        1_893_484_800_000,
                    )),
                    eligibility: QuestRewardEligibility {
                        job_mask: Some(warrior_mask),
                        gender: Some(QuestRewardGender::Male),
                    },
                },
            ],
            ..QuestActions::default()
        },
        dialogue: QuestDialogue {
            completion: QuestCompletionDialogue {
                pages: vec!["Choose one.".to_owned()],
                ..QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        },
        info: QuestInfo::default(),
    };
    let definitions = vec![
        ItemDefinition {
            item_id: 1_332_005,
            name: "Dagger A".to_owned(),
            stack_max: 1,
            ..ItemDefinition::default()
        },
        ItemDefinition {
            item_id: 1_332_007,
            name: "Dagger B".to_owned(),
            stack_max: 1,
            ..ItemDefinition::default()
        },
        ItemDefinition {
            item_id: 4_000_000,
            name: "Blue Snail Shell".to_owned(),
            stack_max: 10,
            ..ItemDefinition::default()
        },
    ];
    let mut player = PlayerState {
        stats: Some(CharacterStats {
            job_id: 112,
            ..CharacterStats::default()
        }),
        appearance: Some(CharacterAppearance {
            gender: CharacterGender::Male as i32,
            ..CharacterAppearance::default()
        }),
        quests: vec![PlayerQuest {
            quest_id: quest.id,
            status: QuestStatus::Started as i32,
            ..PlayerQuest::default()
        }],
        ..PlayerState::default()
    };
    let scripts = crate::quest_scripts::QuestScriptCatalog::default();

    let dialog = active_dialog(&player, &quest, &definitions, &scripts, 200);

    assert_eq!(
        dialog
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Dagger A x2 (expires 60 minutes after receipt)",
            "Blue Snail Shell x3 (expires 2030-01-01 00:00 PST (GMS))",
        ]
    );
    assert!(dialog.choices.iter().all(|choice| {
        NpcDialogChoiceKind::try_from(choice.kind) == Ok(NpcDialogChoiceKind::SelectQuestReward)
    }));
    assert!(dialog.choices.iter().all(|choice| {
        choice.choice_id != crate::quests::ACCEPT_CHOICE_ID
            && choice.choice_id != crate::quests::DECLINE_CHOICE_ID
            && choice.choice_id != crate::quests::COMPLETE_CHOICE_ID
            && Some(choice.choice_id)
                != crate::quests::answer_choice_id(crate::quests::QuestQuestionPhase::Start, 0, 0)
    }));

    player.stats.as_mut().expect("stats").job_id = 212;
    let blocked = active_dialog(&player, &quest, &definitions, &scripts, 200);
    assert!(blocked.choices.is_empty());
    assert!(blocked.pages.last().is_some_and(|page| {
        page == "No selectable quest reward is available for your character."
    }));
}

#[test]
fn absolute_reward_deadlines_use_the_gms_timezone() {
    assert_eq!(
        format_gms_deadline(1_893_484_800_000).as_deref(),
        Some("2030-01-01 00:00 PST (GMS)")
    );
    assert_eq!(
        format_gms_deadline(1_909_119_600_000).as_deref(),
        Some("2030-07-01 00:00 PDT (GMS)")
    );
}

#[test]
fn active_quest_with_a_missing_start_item_shows_the_lost_interaction() {
    let mut quest = QuestDefinition {
        id: 100,
        name: "Lost item".to_owned(),
        start: QuestStartRequirements::default(),
        completion: QuestCompletionRequirements::default(),
        start_actions: QuestActions::default(),
        completion_actions: QuestActions::default(),
        dialogue: QuestDialogue {
            completion: QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose the shells?".to_owned()],
                    success_pages: vec!["Take these replacements.".to_owned()],
                    items: vec![QuestRestorableItem {
                        item_id: 4_000_000,
                        target_count: 2,
                        expiration: None,
                        provenance: crate::content::QuestRestorationProvenance::InferredStartGrant,
                        eligibility: crate::content::QuestRestorationEligibility {
                            owner_state: crate::content::RequiredQuestState::Started,
                            required_quests: &[],
                            forbidden_quests: &[],
                            absent_skill_ids: &[],
                            absent_item_ids: &[],
                        },
                    }],
                }),
                ..QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        },
        info: QuestInfo::default(),
    };
    let definitions = vec![ItemDefinition {
        item_id: 4_000_000,
        name: "Blue Snail Shell".to_owned(),
        stack_max: 10,
        ..ItemDefinition::default()
    }];
    let player = PlayerState {
        inventory: Some(InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: 4_000_000,
                quantity: 1,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        }),
        quests: vec![PlayerQuest {
            quest_id: quest.id,
            status: QuestStatus::Started as i32,
            ..PlayerQuest::default()
        }],
        ..PlayerState::default()
    };

    let dialog = active_dialog(
        &player,
        &quest,
        &definitions,
        &crate::quest_scripts::QuestScriptCatalog::default(),
        200,
    );

    assert_eq!(dialog.pages, vec!["Did you lose the shells?"]);
    assert_eq!(dialog.choices.len(), 1);
    assert_eq!(
        dialog.choices[0].choice_id,
        crate::quests::RESTORE_ITEMS_CHOICE_ID
    );
    assert_eq!(dialog.choices[0].label, "Restore items");
    assert_eq!(
        NpcDialogChoiceKind::try_from(dialog.choices[0].kind),
        Ok(NpcDialogChoiceKind::RestoreQuestItems)
    );

    quest
        .dialogue
        .completion
        .lost
        .as_mut()
        .expect("lost interaction")
        .items[0]
        .expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
    let expired = active_dialog(
        &player,
        &quest,
        &definitions,
        &crate::quest_scripts::QuestScriptCatalog::default(),
        200,
    );
    assert_eq!(expired.choices.len(), 1);
    assert_eq!(
        expired.choices[0].choice_id,
        crate::quests::COMPLETE_CHOICE_ID
    );
}
