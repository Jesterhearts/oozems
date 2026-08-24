use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillBook;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlayerDomains {
    pub stats: bool,
    pub quests: bool,
    pub inventory: bool,
    pub progression: bool,
    pub skills: bool,
    pub key_bindings: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlayerInstallation {
    pub domains: PlayerDomains,
    pub visible_appearance_changed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct AppearanceRefresh {
    pub identity: AppearanceIdentity,
    pub appearance: CharacterAppearance,
    pub equipment: Vec<EquippedItem>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct AppearanceIdentity {
    gender: i32,
    skin_id: u32,
    face_id: u32,
    hair_id: u32,
    equipment: Vec<(i32, u32)>,
}

impl PlayerDomains {
    pub const FULL: Self = Self {
        stats: true,
        quests: true,
        inventory: true,
        progression: true,
        skills: true,
        key_bindings: true,
    };
    pub const STATS_AND_QUESTS: Self = Self {
        stats: true,
        quests: true,
        inventory: false,
        progression: false,
        skills: false,
        key_bindings: false,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlayerRevisions {
    pub stats: u64,
    pub quests: u64,
    pub inventory: u64,
    pub progression: u64,
    pub skills: u64,
    pub key_bindings: u64,
    pub appearance_assets: u64,
}

impl PlayerRevisions {
    pub fn new(revision: u64) -> Self {
        Self {
            stats: revision,
            quests: revision,
            inventory: revision,
            progression: revision,
            skills: revision,
            key_bindings: revision,
            appearance_assets: revision,
        }
    }
}

pub(super) fn revision_is_eligible(
    applied: u64,
    received: u64,
) -> bool {
    received >= applied
}

pub(super) fn install_revision(
    applied: &mut u64,
    received: u64,
) -> bool {
    if !revision_is_eligible(*applied, received) {
        return false;
    }
    *applied = received;
    true
}

pub(super) fn install_player_update(
    current: &mut PlayerState,
    revisions: &mut PlayerRevisions,
    mut update: PlayerState,
    domains: PlayerDomains,
) -> PlayerInstallation {
    let previous_appearance = visible_appearance_identity(current);
    let revision = update.revision;
    current.revision = current.revision.max(revision);
    let mut installed = PlayerDomains::default();

    if domains.stats && update.stats.is_some() && install_revision(&mut revisions.stats, revision) {
        current.stats = update.stats.take();
        installed.stats = true;
    }
    if domains.quests && install_revision(&mut revisions.quests, revision) {
        current.quests = std::mem::take(&mut update.quests);
        current.quest_records = std::mem::take(&mut update.quest_records);
        current.monster_book_cards = std::mem::take(&mut update.monster_book_cards);
        installed.quests = true;
    }
    if domains.inventory
        && update.appearance.is_some()
        && update.inventory.is_some()
        && install_revision(&mut revisions.inventory, revision)
    {
        current.appearance = update.appearance.take();
        current.inventory = update.inventory.take();
        installed.inventory = true;
    }
    if domains.progression && install_revision(&mut revisions.progression, revision) {
        current.level = update.level;
        current.mesos = update.mesos;
        installed.progression = true;
    }
    if domains.skills && install_revision(&mut revisions.skills, revision) {
        current.skill_points = update.skill_points;
        current.learned_skills = std::mem::take(&mut update.learned_skills);
        installed.skills = true;
    }
    if domains.key_bindings && install_revision(&mut revisions.key_bindings, revision) {
        current.key_bindings = std::mem::take(&mut update.key_bindings);
        installed.key_bindings = true;
    }

    PlayerInstallation {
        visible_appearance_changed: installed.inventory
            && previous_appearance != visible_appearance_identity(current),
        domains: installed,
    }
}

pub(super) fn appearance_refresh(player: &PlayerState) -> Option<AppearanceRefresh> {
    let appearance = player.appearance.clone()?;
    let equipment = player.inventory.as_ref()?.equipment.clone();
    Some(AppearanceRefresh {
        identity: appearance_identity(&appearance, &equipment),
        appearance,
        equipment,
        revision: player.revision,
    })
}

pub(super) fn visible_appearance_identity(player: &PlayerState) -> Option<AppearanceIdentity> {
    Some(appearance_identity(
        player.appearance.as_ref()?,
        &player.inventory.as_ref()?.equipment,
    ))
}

fn appearance_identity(
    appearance: &CharacterAppearance,
    equipment: &[EquippedItem],
) -> AppearanceIdentity {
    let mut equipment = equipment
        .iter()
        .map(|equipped| (equipped.slot, equipped.item_id))
        .collect::<Vec<_>>();
    equipment.sort_unstable();
    AppearanceIdentity {
        gender: appearance.gender,
        skin_id: appearance.skin_id,
        face_id: appearance.face_id,
        hair_id: appearance.hair_id,
        equipment,
    }
}

pub(super) fn appearance_assets_are_eligible(
    current_identity: Option<&AppearanceIdentity>,
    request: &AppearanceRefresh,
    inventory_revision: u64,
    appearance_assets_revision: u64,
) -> bool {
    current_identity == Some(&request.identity)
        && request.revision >= inventory_revision
        && revision_is_eligible(appearance_assets_revision, request.revision)
}

pub(super) fn synchronize_skill_book(
    skill_book: &mut SkillBook,
    player: &PlayerState,
) {
    skill_book.available_points = player.skill_points;
    for skill in &mut skill_book.skills {
        let Some(definition) = skill.definition.as_ref() else {
            continue;
        };
        skill.level = player
            .learned_skills
            .iter()
            .find(|learned| learned.skill_id == definition.skill_id)
            .map_or(0, |learned| learned.level);
        skill.master_level = player
            .learned_skills
            .iter()
            .find(|learned| learned.skill_id == definition.skill_id)
            .map_or(0, |learned| learned.master_level);
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::MonsterBookCard;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerSkill;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestRecord;
    use oozems_proto::v1::QuestRecordEntry;
    use oozems_proto::v1::SkillBook;
    use oozems_proto::v1::SkillDefinition;

    use super::PlayerDomains;
    use super::PlayerRevisions;
    use super::appearance_assets_are_eligible;
    use super::appearance_refresh;
    use super::install_player_update;
    use super::revision_is_eligible;
    use super::synchronize_skill_book;

    #[test]
    fn skill_book_synchronization_copies_level_and_mastery() {
        let mut book = SkillBook {
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 2_321_003,
                    max_level: 30,
                    ..SkillDefinition::default()
                }),
                ..PlayerSkill::default()
            }],
            ..SkillBook::default()
        };
        let player = PlayerState {
            skill_points: 3,
            learned_skills: vec![oozems_proto::v1::LearnedSkill {
                skill_id: 2_321_003,
                level: 10,
                master_level: 15,
            }],
            ..PlayerState::default()
        };

        synchronize_skill_book(&mut book, &player);

        assert_eq!(book.available_points, 3);
        assert_eq!(
            (book.skills[0].level, book.skills[0].master_level),
            (10, 15)
        );
    }

    #[test]
    fn skill_book_synchronization_clears_removed_skill_levels() {
        let mut book = SkillBook {
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 1_007,
                    max_level: 3,
                    ..SkillDefinition::default()
                }),
                level: 3,
                master_level: 3,
            }],
            ..SkillBook::default()
        };

        synchronize_skill_book(&mut book, &PlayerState::default());

        assert_eq!((book.skills[0].level, book.skills[0].master_level), (0, 0));
    }

    #[test]
    fn full_updates_remove_skills_and_key_bindings_without_stale_reinstallation() {
        let target_binding = KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 1_007,
        };
        let retained_binding = KeyBinding {
            code: "Space".to_owned(),
            action: KeyAction::Jump as i32,
            skill_id: 0,
        };
        let learned = oozems_proto::v1::LearnedSkill {
            skill_id: 1_007,
            level: 3,
            master_level: 3,
        };
        let mut player = PlayerState {
            revision: 1,
            learned_skills: vec![learned.clone()],
            key_bindings: vec![target_binding.clone(), retained_binding.clone()],
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::new(1);

        let removed = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 2,
                key_bindings: vec![retained_binding.clone()],
                ..PlayerState::default()
            },
            PlayerDomains::FULL,
        );

        assert!(removed.domains.skills);
        assert!(removed.domains.key_bindings);
        assert!(player.learned_skills.is_empty());
        assert_eq!(player.key_bindings, vec![retained_binding.clone()]);
        assert_eq!(revisions.skills, 2);
        assert_eq!(revisions.key_bindings, 2);

        let stale = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 1,
                learned_skills: vec![learned],
                key_bindings: vec![target_binding, retained_binding.clone()],
                ..PlayerState::default()
            },
            PlayerDomains::FULL,
        );

        assert!(!stale.domains.skills);
        assert!(!stale.domains.key_bindings);
        assert!(player.learned_skills.is_empty());
        assert_eq!(player.key_bindings, vec![retained_binding]);
    }

    #[test]
    fn pending_local_key_bindings_can_defer_an_equal_revision_update() {
        let local_binding = KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Jump as i32,
            skill_id: 0,
        };
        let server_binding = KeyBinding {
            code: "Space".to_owned(),
            action: KeyAction::Jump as i32,
            skill_id: 0,
        };
        let mut player = PlayerState {
            revision: 5,
            key_bindings: vec![local_binding.clone()],
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::new(5);
        let mut deferred_domains = PlayerDomains::FULL;
        deferred_domains.key_bindings = false;

        let deferred = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 5,
                key_bindings: vec![server_binding.clone()],
                ..PlayerState::default()
            },
            deferred_domains,
        );

        assert!(!deferred.domains.key_bindings);
        assert_eq!(player.key_bindings, vec![local_binding]);

        let acknowledged = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 6,
                key_bindings: vec![server_binding.clone()],
                ..PlayerState::default()
            },
            PlayerDomains::FULL,
        );

        assert!(acknowledged.domains.key_bindings);
        assert_eq!(player.key_bindings, vec![server_binding]);
    }

    #[test]
    fn progression_updates_accept_newer_and_equal_revisions_but_reject_older_revisions() {
        let mut player = PlayerState {
            revision: 5,
            mesos: 10,
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::new(5);
        let progression = PlayerDomains {
            progression: true,
            ..PlayerDomains::default()
        };

        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 6,
                    mesos: 20,
                    ..PlayerState::default()
                },
                progression,
            )
            .domains
            .progression
        );
        assert_eq!((player.revision, player.mesos), (6, 20));

        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 6,
                    mesos: 30,
                    ..PlayerState::default()
                },
                progression,
            )
            .domains
            .progression
        );
        assert_eq!((player.revision, player.mesos), (6, 30));

        assert!(
            !install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 5,
                    mesos: 40,
                    ..PlayerState::default()
                },
                progression,
            )
            .domains
            .progression
        );
        assert_eq!((player.revision, player.mesos), (6, 30));
        assert_eq!(revisions.progression, 6);
    }

    #[test]
    fn reordered_quest_updates_keep_the_newest_revision() {
        let mut player = PlayerState::default();
        let newer = vec![PlayerQuest {
            quest_id: 200,
            ..PlayerQuest::default()
        }];
        let older = vec![PlayerQuest {
            quest_id: 100,
            ..PlayerQuest::default()
        }];
        let mut revisions = PlayerRevisions::default();
        let quests = PlayerDomains {
            quests: true,
            ..PlayerDomains::default()
        };

        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 2,
                    quests: newer.clone(),
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );
        assert!(
            !install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 1,
                    quests: older,
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );

        assert_eq!(player.revision, 2);
        assert_eq!(player.quests, newer);
        assert_eq!(revisions.quests, 2);
    }

    #[test]
    fn quest_records_follow_the_authoritative_quest_revision_gate() {
        let mut player = PlayerState::default();
        let mut revisions = PlayerRevisions::default();
        let quests = PlayerDomains {
            quests: true,
            ..PlayerDomains::default()
        };
        let newer_record = QuestRecord {
            quest_id: 200,
            entries: vec![QuestRecordEntry {
                index: 7,
                value: "newer".to_owned(),
            }],
        };

        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 2,
                    quest_records: vec![newer_record.clone()],
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );
        assert!(
            !install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 1,
                    quest_records: vec![QuestRecord {
                        quest_id: 100,
                        entries: Vec::new(),
                    }],
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );

        assert_eq!(player.quest_records, vec![newer_record]);
        assert_eq!(revisions.quests, 2);
    }

    #[test]
    fn monster_book_cards_follow_the_authoritative_quest_revision_gate() {
        let mut player = PlayerState::default();
        let mut revisions = PlayerRevisions::default();
        let quests = PlayerDomains {
            quests: true,
            ..PlayerDomains::default()
        };
        let newer = vec![MonsterBookCard {
            card_item_id: 2_380_000,
            count: 2,
        }];

        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 2,
                    monster_book_cards: newer.clone(),
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );
        assert!(
            !install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 1,
                    monster_book_cards: vec![MonsterBookCard {
                        card_item_id: 2_380_000,
                        count: 1,
                    }],
                    ..PlayerState::default()
                },
                quests,
            )
            .domains
            .quests
        );

        assert_eq!(player.monster_book_cards, newer);
        assert_eq!(revisions.quests, 2);
    }

    #[test]
    fn newer_sparse_stats_and_quests_do_not_suppress_older_inventory() {
        let mut player = PlayerState {
            appearance: Some(CharacterAppearance::default()),
            inventory: Some(InventoryState::default()),
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::default();
        let inventory = InventoryState {
            item_ids: vec![4_001_000],
            ..InventoryState::default()
        };

        let sparse = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 3,
                stats: Some(CharacterStats {
                    hp: 50,
                    ..CharacterStats::default()
                }),
                quests: vec![PlayerQuest {
                    quest_id: 100,
                    ..PlayerQuest::default()
                }],
                ..PlayerState::default()
            },
            PlayerDomains::STATS_AND_QUESTS,
        );
        let installed_inventory = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 2,
                appearance: Some(CharacterAppearance::default()),
                inventory: Some(inventory.clone()),
                ..PlayerState::default()
            },
            PlayerDomains {
                inventory: true,
                ..PlayerDomains::default()
            },
        );

        assert!(sparse.domains.stats);
        assert!(sparse.domains.quests);
        assert!(installed_inventory.domains.inventory);
        assert_eq!(player.inventory, Some(inventory));
        assert_eq!(revisions.stats, 3);
        assert_eq!(revisions.quests, 3);
        assert_eq!(revisions.inventory, 2);
    }

    #[test]
    fn delayed_stale_equipment_cannot_overwrite_newer_inventory() {
        let older_inventory = InventoryState {
            equipment: vec![EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: 1_040_002,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        };
        let newer_inventory = InventoryState {
            equipment: vec![EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: 1_040_010,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        };
        let mut player = PlayerState {
            revision: 1,
            appearance: Some(CharacterAppearance::default()),
            inventory: Some(InventoryState::default()),
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::new(1);
        let inventory_domain = PlayerDomains {
            inventory: true,
            ..PlayerDomains::default()
        };

        assert!(revision_is_eligible(revisions.inventory, 2));
        assert!(
            install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 3,
                    appearance: Some(CharacterAppearance::default()),
                    inventory: Some(newer_inventory.clone()),
                    ..PlayerState::default()
                },
                inventory_domain,
            )
            .domains
            .inventory
        );
        assert!(
            !install_player_update(
                &mut player,
                &mut revisions,
                PlayerState {
                    revision: 2,
                    appearance: Some(CharacterAppearance::default()),
                    inventory: Some(older_inventory),
                    ..PlayerState::default()
                },
                inventory_domain,
            )
            .domains
            .inventory
        );

        assert_eq!(player.inventory, Some(newer_inventory));
        assert_eq!(revisions.inventory, 3);
    }

    #[test]
    fn generic_newer_inventory_detects_visible_equipment_changes_only() {
        let mut player = PlayerState {
            revision: 1,
            appearance: Some(CharacterAppearance::default()),
            inventory: Some(InventoryState {
                equipment: vec![EquippedItem {
                    slot: EquipmentSlot::Top as i32,
                    item_id: 1_040_002,
                    expires_at_unix_ms: 100,
                }],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };
        let mut revisions = PlayerRevisions::new(1);
        let inventory = PlayerDomains {
            inventory: true,
            ..PlayerDomains::default()
        };
        let timestamp_only = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 2,
                appearance: Some(CharacterAppearance::default()),
                inventory: Some(InventoryState {
                    equipment: vec![EquippedItem {
                        slot: EquipmentSlot::Top as i32,
                        item_id: 1_040_002,
                        expires_at_unix_ms: 200,
                    }],
                    ..InventoryState::default()
                }),
                ..PlayerState::default()
            },
            inventory,
        );
        assert!(timestamp_only.domains.inventory);
        assert!(!timestamp_only.visible_appearance_changed);

        let pruned = install_player_update(
            &mut player,
            &mut revisions,
            PlayerState {
                revision: 3,
                appearance: Some(CharacterAppearance::default()),
                inventory: Some(InventoryState::default()),
                ..PlayerState::default()
            },
            inventory,
        );
        assert!(pruned.domains.inventory);
        assert!(pruned.visible_appearance_changed);
    }

    #[test]
    fn stale_async_appearance_assets_are_rejected() {
        let player = PlayerState {
            revision: 3,
            appearance: Some(CharacterAppearance::default()),
            inventory: Some(InventoryState {
                equipment: vec![EquippedItem {
                    slot: EquipmentSlot::Top as i32,
                    item_id: 1_040_010,
                    expires_at_unix_ms: 500,
                }],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };
        let current = appearance_refresh(&player).expect("current appearance");
        let mut stale_revision = current.clone();
        stale_revision.revision = 2;
        assert!(!appearance_assets_are_eligible(
            Some(&current.identity),
            &stale_revision,
            3,
            1,
        ));

        let mut stale_identity = current.clone();
        stale_identity.identity.equipment[0].1 = 1_040_002;
        assert!(!appearance_assets_are_eligible(
            Some(&current.identity),
            &stale_identity,
            3,
            1,
        ));
        assert!(appearance_assets_are_eligible(
            Some(&current.identity),
            &current,
            3,
            3,
        ));
    }
}
