use std::path::Path;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterCreationOptions;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::Map;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::SkillEffect;
use thiserror::Error;

mod character;
mod config;
mod gui;
mod item;
mod morph;
mod quest;
mod skill;
mod wz;

use character::CharacterContent;
use character::CharacterContentError;
pub use config::ContentConfig;
use gui::GuiContent;
use gui::GuiContentError;
pub(crate) use item::ConsumeEffectDefinition;
use item::ItemContent;
use item::ItemContentError;
pub(crate) use item::MonsterBookCardDefinition;
use morph::MorphContent;
use morph::MorphContentError;
use quest::QuestContent;
use quest::QuestContentError;
pub(crate) use quest::model::*;
pub(crate) use skill::AuthoritativeSkillDefinition;
pub(crate) use skill::SkillBookContext;
use skill::SkillContent;
use skill::SkillContentError;
pub(crate) use wz::WzAsset;
use wz::WzContent;
use wz::WzContentError;

const PLAYER_HALF_WIDTH: f32 = 18.0;

pub struct ContentCatalog {
    characters: Option<CharacterContent>,
    gui: Option<GuiContent>,
    items: Option<ItemContent>,
    morphs: Option<MorphContent>,
    quests: Option<QuestContent>,
    skills: Option<SkillContent>,
    wz: WzContent,
}

#[derive(Debug, Error)]
pub enum ContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error(transparent)]
    Character(#[from] CharacterContentError),
    #[error(transparent)]
    Gui(#[from] GuiContentError),
    #[error(transparent)]
    Item(#[from] ItemContentError),
    #[error(transparent)]
    Morph(#[from] MorphContentError),
    #[error(transparent)]
    Quest(#[from] QuestContentError),
    #[error(transparent)]
    Skill(#[from] SkillContentError),
}

impl ContentCatalog {
    pub fn load(
        wz_dir: &Path,
        config: &ContentConfig,
    ) -> Result<Self, ContentError> {
        let wz = WzContent::open(wz_dir, config.npcs.clone())?;
        let characters = CharacterContent::open_optional(wz_dir)?;
        let mut items = ItemContent::load(wz_dir, characters.as_ref())?;
        let item_source_ids = items
            .as_ref()
            .map(ItemContent::source_ids)
            .unwrap_or_default();
        let equipment_source_ids = items
            .as_ref()
            .map(ItemContent::equipment_source_ids)
            .unwrap_or_default();
        let skills = SkillContent::open_optional(wz_dir)?;
        let skill_source_ids = skills
            .as_ref()
            .map(SkillContent::authoritative_skill_ids)
            .unwrap_or_default();
        let skill_source_names = skills
            .as_ref()
            .map(SkillContent::authoritative_skill_names)
            .transpose()?
            .unwrap_or_default();
        let consume_effect_ids = items
            .as_ref()
            .map(ItemContent::consume_effect_ids)
            .unwrap_or_default();
        let monster_book_card_ids = items
            .as_ref()
            .map(ItemContent::monster_book_card_ids)
            .unwrap_or_default();
        let morphs = if items.as_ref().is_some_and(|items| {
            items
                .consume_effect_definitions()
                .iter()
                .any(|effect| effect.morph_id.is_some())
        }) {
            Some(MorphContent::load(wz_dir)?)
        } else {
            None
        };
        let morph_ids = morphs
            .as_ref()
            .map(|morphs| morphs.ids().collect())
            .unwrap_or_default();
        let quests = QuestContent::open_optional(
            wz_dir,
            config.quest_ids.as_ref(),
            &item_source_ids,
            &equipment_source_ids,
            &consume_effect_ids,
            &monster_book_card_ids,
            &morph_ids,
            &skill_source_ids,
            &skill_source_names,
        )?;
        if let Some(items) = items.as_mut() {
            let quest_item_ids = quests
                .as_ref()
                .map(QuestContent::item_reference_ids)
                .cloned()
                .unwrap_or_default();
            items.materialize_eager(&quest_item_ids)?;
        }
        Ok(Self {
            characters,
            gui: GuiContent::open_optional(wz_dir)?,
            items,
            morphs,
            quests,
            skills,
            wz,
        })
    }

    pub fn get_map(
        &self,
        map_id: u32,
    ) -> Result<Option<Map>, ContentError> {
        if !self.wz.contains_map(map_id) {
            return Ok(None);
        }
        self.wz.get_map(map_id).map(Some).map_err(Into::into)
    }

    pub fn get_wz_asset(
        &self,
        asset_id: &str,
    ) -> Option<std::sync::Arc<WzAsset>> {
        self.wz
            .get_asset(asset_id)
            .or_else(|| {
                self.characters
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
            .or_else(|| {
                self.items
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
            .or_else(|| {
                self.morphs
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
            .or_else(|| {
                self.gui
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
            .or_else(|| {
                self.skills
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
    }

    pub fn game_gui(&self) -> GameGui {
        let mut gui = self
            .gui
            .as_ref()
            .map(GuiContent::game_gui)
            .unwrap_or_default();
        if let Some(items) = &self.items {
            gui.assets.extend(items.descriptor_clones_for_gui());
            gui.items = items.definition_clones_for_gui();
            tracing::info!(
                item_count = gui.items.len(),
                asset_count = gui.assets.len(),
                "game GUI payload ready"
            );
        }
        gui
    }

    pub fn character_creation_options(&self) -> CharacterCreationOptions {
        self.characters
            .as_ref()
            .map(CharacterContent::creation_options)
            .unwrap_or_default()
    }

    pub(crate) fn skill_book_context(
        &self,
        player: &oozems_proto::v1::PlayerState,
    ) -> Result<SkillBookContext, ContentError> {
        let job_id = player.stats.as_ref().map_or(0, |stats| stats.job_id);
        self.skills
            .as_ref()
            .map(|source| source.skill_book_context(job_id, &player.learned_skills))
            .transpose()
            .map(|context| {
                context.unwrap_or_else(|| SkillBookContext {
                    book: SkillBook {
                        job_id,
                        name: "Skills".to_owned(),
                        ..SkillBook::default()
                    },
                    authoritative_skills: Vec::new(),
                })
            })
            .map_err(Into::into)
    }

    pub fn skill_effect(
        &self,
        job_id: u32,
        skill_id: u32,
        level: u32,
    ) -> Result<SkillEffect, ContentError> {
        self.skills
            .as_ref()
            .map(|source| source.skill_effect(job_id, skill_id, level))
            .transpose()
            .map(|effect| effect.unwrap_or_default())
            .map_err(Into::into)
    }

    pub(crate) fn item_definition_slice(&self) -> &[ItemDefinition] {
        self.items
            .as_ref()
            .map(ItemContent::eager_definition_slice)
            .unwrap_or_default()
    }

    pub(crate) fn consume_effect_definitions(&self) -> Vec<ConsumeEffectDefinition> {
        self.items
            .as_ref()
            .map(ItemContent::consume_effect_definitions)
            .unwrap_or_default()
    }

    pub(crate) fn monster_book_card_ids(&self) -> std::collections::BTreeSet<u32> {
        self.items
            .as_ref()
            .map(ItemContent::monster_book_card_ids)
            .unwrap_or_default()
    }

    pub(crate) fn monster_book_card(
        &self,
        item_id: u32,
    ) -> Option<MonsterBookCardDefinition> {
        self.items
            .as_ref()
            .and_then(|items| items.monster_book_card(item_id))
    }

    pub fn morph_definition(
        &self,
        morph_id: u32,
    ) -> Option<oozems_proto::v1::MorphDefinition> {
        self.morphs
            .as_ref()
            .and_then(|morphs| morphs.definition(morph_id))
    }

    pub(crate) fn project_item_definitions(
        &mut self,
        item_ids: &std::collections::BTreeSet<u32>,
    ) -> Result<(), ContentError> {
        if item_ids.is_empty() {
            return Ok(());
        }
        let items = self
            .items
            .as_mut()
            .ok_or_else(|| ItemContentError::Invalid {
                message: "item definitions were requested without an item source index".to_owned(),
            })?;
        items.materialize_additional_eager(item_ids)?;
        Ok(())
    }

    pub(crate) fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ContentError> {
        self.items
            .as_ref()
            .map(|items| items.definition(item_id))
            .transpose()
            .map(Option::flatten)
            .map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn indexed_item_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.items.iter().flat_map(|items| items.source_id_iter())
    }

    pub(crate) fn quest(
        &self,
        quest_id: u32,
    ) -> Option<&QuestDefinition> {
        self.quests.as_ref()?.get(quest_id)
    }

    pub(crate) fn quest_definitions(&self) -> impl Iterator<Item = &QuestDefinition> {
        self.quests.iter().flat_map(|quests| quests.definitions())
    }

    #[cfg(test)]
    pub(crate) fn quest_load_report(&self) -> Option<&quest::QuestLoadReport> {
        self.quests.as_ref().map(QuestContent::report)
    }

    #[cfg(test)]
    pub(crate) fn quest_item_reference_ids(&self) -> impl Iterator<Item = u32> {
        self.quests
            .iter()
            .flat_map(|quests| quests.item_reference_ids().iter().copied())
    }

    pub(crate) fn quests_for_npc(
        &self,
        npc_id: u32,
    ) -> Vec<&QuestDefinition> {
        self.quests
            .as_ref()
            .map(|quests| {
                quests
                    .definitions()
                    .filter(|quest| {
                        quest.start.npc_id == Some(npc_id)
                            || quest.completion.npc_id == Some(npc_id)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn supports_character(
        &self,
        appearance: &CharacterAppearance,
    ) -> bool {
        self.characters
            .as_ref()
            .is_some_and(|source| source.supports(appearance))
    }

    pub fn get_character_sprites(
        &self,
        appearance: &CharacterAppearance,
        equipment: &[EquippedItem],
    ) -> Result<Option<CharacterSpriteSet>, ContentError> {
        self.characters
            .as_ref()
            .map(|source| source.get_sprites(appearance, equipment))
            .transpose()
            .map(|sprites| sprites.flatten())
            .map_err(Into::into)
    }
}

impl crate::items::ItemDefinitionLookup for ContentCatalog {
    fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, crate::items::ItemRuleError> {
        ContentCatalog::item_definition(self, item_id).map_err(|error| {
            crate::items::ItemRuleError::DefinitionLoad {
                item_id,
                message: error.to_string(),
            }
        })
    }

    fn monster_book_card(
        &self,
        item_id: u32,
    ) -> Option<MonsterBookCardDefinition> {
        ContentCatalog::monster_book_card(self, item_id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashSet;
    use std::path::Path;

    use super::ContentCatalog;
    use super::ContentConfig;
    use super::QuestItemCondition;
    use super::QuestItemDelta;
    use super::QuestRecordPredicate;
    use super::QuestRestorationProvenance;
    use super::QuestSkillOperation;
    use super::QuestStateActionState;
    use super::QuestWeekday;
    use super::RequiredQuestState;

    #[test]
    fn map_archive_is_required() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let error = ContentCatalog::load(directory.path(), &ContentConfig::default())
            .err()
            .expect("missing Map.wz must fail");

        assert!(error.to_string().contains("Map.wz is required"));
    }

    #[test]
    fn local_wz_sample_loads_when_present() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !wz_dir.join("Map.wz").exists() {
            return;
        }

        let catalog = ContentCatalog::load(
            &wz_dir,
            &ContentConfig::load(&manifest_dir.join("../../config/content.toml"))
                .expect("content configuration should be valid"),
        )
        .expect("sample WZ archives should be valid");
        let gameplay =
            crate::gameplay::GameplayConfig::load(&manifest_dir.join("../../config/gameplay.toml"))
                .expect("gameplay configuration should be valid");
        let starter_map = catalog
            .get_map(gameplay.initial_map_id)
            .expect("starter map lookup should succeed")
            .expect("Mushroom Town should exist");
        if wz_dir.join("Quest.wz").exists() {
            let mut equipment_check_quest_ids = catalog
                .quest_definitions()
                .filter(|quest| {
                    !quest.start.equipped_items.all_of.is_empty()
                        || !quest.start.equipped_items.any_of.is_empty()
                        || !quest.completion.equipped_items.all_of.is_empty()
                        || !quest.completion.equipped_items.any_of.is_empty()
                })
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            equipment_check_quest_ids.sort_unstable();
            assert_eq!(
                equipment_check_quest_ids,
                vec![
                    3_307, 3_308, 3_309, 3_310, 3_312, 3_313, 3_314, 3_315, 9_999, 10_021, 10_060,
                    10_218
                ],
                "all 12 compatible equipment-check quests must load",
            );
            let equipment_quest = catalog
                .quest(10_021)
                .expect("all-of and any-of equipment quest");
            assert_eq!(
                equipment_quest.start.equipped_items.all_of,
                vec![1_002_800, 1_032_058, 1_102_174, 1_072_366, 1_082_245]
            );
            assert_eq!(
                equipment_quest.start.equipped_items.any_of,
                vec![1_052_167, 1_052_166]
            );
            let quiz = catalog.quest(1_009).expect("Rain's compatible quiz");
            assert_eq!(
                quiz.dialogue
                    .question
                    .as_ref()
                    .and_then(|question| question.steps.first())
                    .map(|step| step.choices.len()),
                Some(4)
            );
            let quest = catalog.quest(1_007).expect("Biggs's ordinary item quest");
            assert_eq!(
                quest
                    .completion
                    .items
                    .iter()
                    .map(|item| {
                        let count = match item.condition {
                            QuestItemCondition::Absent => 0,
                            QuestItemCondition::AtLeast(count) => count.get(),
                        };
                        (item.item_id, count)
                    })
                    .collect::<Vec<_>>(),
                vec![(4_000_000, 5), (4_000_001, 1)]
            );
            assert_eq!(quest.completion_actions.experience, 50);
            assert_eq!(quest.completion_actions.weighted_items.len(), 2);
            assert_eq!(
                quest
                    .completion_actions
                    .fixed_items
                    .iter()
                    .map(|item| (item.item_id, item.count))
                    .collect::<Vec<_>>(),
                vec![(4_000_000, -5), (4_000_001, -1)]
            );
            assert_eq!(
                catalog
                    .quest(1_030)
                    .expect("map-gated quest")
                    .start
                    .allowed_map_ids,
                vec![50_000]
            );
            let mut item_absence_quest_ids = catalog
                .quest_definitions()
                .filter(|quest| {
                    quest
                        .start
                        .items
                        .iter()
                        .chain(&quest.completion.items)
                        .any(|item| item.condition == QuestItemCondition::Absent)
                })
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            item_absence_quest_ids.sort_unstable();
            eprintln!("compatible item-absence quests: {item_absence_quest_ids:?}");
            assert_eq!(
                item_absence_quest_ids,
                vec![
                    4_522, 4_523, 8_220, 8_228, 8_850, 8_851, 8_852, 8_853, 8_854, 8_871, 9_951,
                    10_230, 28_103,
                ]
            );
            let no_op = catalog.quest(4_930).expect("zero-count action quest");
            assert!(
                no_op
                    .completion_actions
                    .fixed_items
                    .iter()
                    .all(|item| item.item_id != 4_031_447)
            );
            assert!(
                catalog
                    .quest(2_077)
                    .expect("mutually exclusive quest path")
                    .start
                    .quests
                    .iter()
                    .any(|requirement| requirement.state == RequiredQuestState::NotStarted)
            );
            let scheduled = catalog.quest(8_251).expect("scheduled repeat quest");
            assert_eq!(scheduled.start.repeat.interval_ms, Some(0));
            assert!(
                scheduled
                    .start
                    .repeat
                    .days_of_week
                    .contains(&QuestWeekday::Sunday)
            );
            assert_eq!(
                catalog
                    .quest(1_018)
                    .expect("quest with an omitted action-item count")
                    .completion_actions
                    .fixed_items,
                vec![QuestItemDelta {
                    item_id: 4_000_142,
                    count: 1,
                    expiration: None,
                }],
                "the archive's omitted item count must default to one"
            );
            assert_eq!(
                catalog
                    .quest(9_984)
                    .expect("quest with conflicting Act requirement copies")
                    .start
                    .available_until
                    .as_ref()
                    .map(|calendar| calendar.source.as_str()),
                Some("2009061623"),
                "Check.img must remain authoritative over Act.img's conflicting 2008052600 end"
            );
            for quest_id in [20_504, 20_508, 20_511, 20_512] {
                let dialogue = &catalog
                    .quest(quest_id)
                    .unwrap_or_else(|| panic!("manual Act-dialogue quest {quest_id}"))
                    .dialogue;
                assert!(
                    !dialogue.offer_pages.is_empty()
                        && !dialogue.accepted_pages.is_empty()
                        && !dialogue.declined_pages.is_empty(),
                    "manual quest {quest_id} must retain its Act dialogue fallback"
                );
            }
            assert!(
                !catalog
                    .quest(2_003)
                    .expect("quest with null status text")
                    .info
                    .status_text
                    .contains_key(&0)
            );
            let mut selectable_quest_ids = catalog
                .quests
                .as_ref()
                .expect("quest content")
                .definitions()
                .filter(|quest| !quest.completion_actions.selectable_items.is_empty())
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            selectable_quest_ids.sort_unstable();
            eprintln!("compatible selectable-reward quests: {selectable_quest_ids:?}");
            assert_eq!(
                selectable_quest_ids,
                vec![
                    2_001, 2_047, 2_094, 2_095, 2_119, 2_337, 3_220, 3_233, 3_379, 3_414, 3_607,
                    3_608, 3_611, 3_946, 4_510, 4_511, 6_017, 6_023, 6_024, 6_026, 8_025, 8_246,
                    9_130, 9_380, 9_381, 9_382, 9_383, 9_384, 9_706, 9_862, 9_865, 9_869, 9_871,
                    10_005, 10_322, 10_405, 10_445,
                ],
                "every otherwise representable prop -1 quest must load"
            );
            let mut lost_item_quest_ids = catalog
                .quests
                .as_ref()
                .expect("quest content")
                .definitions()
                .filter(|quest| quest.dialogue.completion.lost.is_some())
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            lost_item_quest_ids.sort_unstable();
            eprintln!("compatible lost-item restoration quests: {lost_item_quest_ids:?}");
            assert!(
                lost_item_quest_ids.len() > 100
                    && [1_001, 1_034, 2_100, 4_437, 10_430]
                        .iter()
                        .all(|quest_id| lost_item_quest_ids.contains(quest_id)),
                "prompt-only and result-bearing lost-item interactions must load when safely \
                 mapped"
            );
            let restored_without_completion_items = [
                3_001, 3_002, 3_114, 3_302, 3_839, 3_926, 3_929, 3_940, 4_013, 4_103, 6_015, 6_016,
                6_017, 6_018, 6_019, 6_020, 6_021, 6_022, 6_023, 6_024, 6_025, 6_026, 6_027, 6_132,
                6_230, 9_173, 9_183, 9_193, 9_943,
            ];
            assert!(
                restored_without_completion_items
                    .iter()
                    .all(|quest_id| lost_item_quest_ids.contains(quest_id))
            );
            for (quest_id, item_id, provenance) in [
                (
                    2_208,
                    4_031_890,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_007,
                    4_031_291,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_008,
                    4_031_303,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_009,
                    4_031_292,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_010,
                    4_031_293,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_944,
                    4_031_771,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_960,
                    4_031_771,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_946,
                    4_031_767,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_953,
                    4_031_768,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    4_954,
                    4_031_772,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    6_263,
                    4_031_450,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
                (
                    6_273,
                    4_001_109,
                    QuestRestorationProvenance::AuditedCompletionGrant,
                ),
            ] {
                let quest = catalog
                    .quest(quest_id)
                    .unwrap_or_else(|| panic!("audited lost restoration quest {quest_id}"));
                let lost = quest
                    .dialogue
                    .completion
                    .lost
                    .as_ref()
                    .expect("audited completion lost dialogue");
                assert!(!lost.prompt_pages.is_empty());
                assert_eq!(lost.items.len(), 1);
                assert_eq!(
                    (
                        lost.items[0].item_id,
                        lost.items[0].target_count,
                        lost.items[0].expiration,
                        lost.items[0].provenance,
                    ),
                    (item_id, 1, None, provenance),
                );

                let completion_actions = quest
                    .completion_actions
                    .fixed_items
                    .iter()
                    .filter(|action| action.item_id == item_id)
                    .collect::<Vec<_>>();
                if provenance == QuestRestorationProvenance::AuditedCompletionGrant {
                    assert_eq!(completion_actions.len(), 1);
                    assert_eq!(completion_actions[0].count, 1);
                    assert_eq!(completion_actions[0].expiration, None);
                    assert!(
                        quest
                            .completion_actions
                            .conditional_items
                            .iter()
                            .all(|action| action.item_id != item_id)
                            && quest
                                .completion_actions
                                .weighted_items
                                .iter()
                                .all(|action| action.item_id != item_id)
                            && quest
                                .completion_actions
                                .selectable_items
                                .iter()
                                .all(|action| action.item_id != item_id)
                    );
                } else {
                    assert!(completion_actions.is_empty());
                    assert_eq!(
                        quest
                            .completion
                            .items
                            .iter()
                            .map(|requirement| (requirement.item_id, requirement.condition))
                            .collect::<Vec<_>>(),
                        vec![(
                            4_031_709,
                            QuestItemCondition::AtLeast(
                                std::num::NonZeroU32::new(1).expect("positive reactor output")
                            )
                        )],
                        "quest 3310 completes with the device produced by reactor 2619000",
                    );
                }
            }
            let mut selected_skill_quests = catalog
                .quest_definitions()
                .filter_map(|quest| {
                    quest
                        .info
                        .selected_skill
                        .as_ref()
                        .map(|skill| (quest.id, skill.id.get(), skill.name.clone()))
                })
                .collect::<Vec<_>>();
            selected_skill_quests.sort_by_key(|(quest_id, _, _)| *quest_id);
            assert_eq!(
                selected_skill_quests
                    .iter()
                    .map(|(quest_id, skill_id, _)| (*quest_id, *skill_id))
                    .collect::<Vec<_>>(),
                vec![
                    (2_415, 1_001_004),
                    (2_416, 2_001_005),
                    (2_417, 3_001_004),
                    (2_418, 4_001_334),
                    (2_419, 5_001_001),
                    (2_420, 4_001_344),
                    (2_421, 5_001_003),
                ]
            );
            assert!(
                selected_skill_quests
                    .iter()
                    .all(|(_, _, name)| name.as_ref().is_some_and(|name| !name.is_empty()))
            );
            let duplicated_dialogue = catalog
                .quest(3_077)
                .expect("audited duplicate-dialogue quest");
            let duplicated_question = duplicated_dialogue
                .dialogue
                .start_question
                .as_ref()
                .expect("quest 3077 start question");
            assert_eq!(duplicated_question.steps.len(), 2);
            assert!(
                duplicated_question
                    .steps
                    .iter()
                    .all(|step| step.choices.len() == 1)
            );
            let aliased_action = catalog.quest(4_960).expect("audited missing-Act quest");
            assert_eq!(aliased_action.id, 4_960);
            assert_eq!(aliased_action.start.minimum_level, None);
            assert_eq!(aliased_action.completion_actions.experience, 8_000);
            assert_eq!(
                aliased_action
                    .completion_actions
                    .fixed_items
                    .iter()
                    .map(|item| (item.item_id, item.count))
                    .collect::<Vec<_>>(),
                vec![
                    (4_031_771, 1),
                    (2_022_247, -20),
                    (2_022_248, -20),
                    (2_022_249, -20),
                    (2_022_250, -20),
                    (2_022_251, 5),
                ]
            );
            let malformed_item_metadata = catalog
                .quest(10_272)
                .expect("audited malformed item-metadata quest");
            assert_eq!(
                malformed_item_metadata.completion.script.as_deref(),
                Some("q10272e")
            );
            assert_eq!(
                malformed_item_metadata
                    .completion_actions
                    .fixed_items
                    .iter()
                    .map(|item| (item.item_id, item.count))
                    .collect::<Vec<_>>(),
                vec![(4_032_283, -10), (4_032_280, -10)]
            );
            assert!(
                malformed_item_metadata
                    .completion_actions
                    .selectable_items
                    .is_empty()
            );
            let start_question_count = catalog
                .quests
                .as_ref()
                .expect("quest content")
                .definitions()
                .filter(|quest| quest.dialogue.start_question.is_some())
                .count();
            assert_eq!(
                start_question_count, 375,
                "every compatible start question must load"
            );
            let missing_answer_quest_ids = [
                2_300, 2_301, 2_302, 2_303, 2_304, 2_305, 2_306, 2_307, 2_308, 2_309, 2_310, 10_329,
            ];
            assert_eq!(
                missing_answer_quest_ids
                    .iter()
                    .map(|quest_id| {
                        let steps = &catalog
                            .quest(*quest_id)
                            .unwrap_or_else(|| panic!("one-choice question quest {quest_id}"))
                            .dialogue
                            .start_question
                            .as_ref()
                            .expect("typed one-choice start question")
                            .steps;
                        assert!(steps.iter().all(|step| step.choices.len() == 1));
                        steps.len()
                    })
                    .sum::<usize>(),
                14,
                "the archive has exactly 14 one-choice start steps with omitted answers"
            );
            assert!(
                catalog
                    .quest(21_703)
                    .expect("completion ask without a choice")
                    .dialogue
                    .question
                    .is_none()
            );
            assert!(
                catalog
                    .quest(21_766)
                    .expect("start ask without a choice")
                    .dialogue
                    .start_question
                    .is_none()
            );
            let quest_2015 = catalog
                .quest(2_015)
                .expect("quest 2015 sparse-ID question")
                .dialogue
                .question
                .as_ref()
                .expect("quest 2015 question");
            let sparse_step = &quest_2015.steps[3];
            assert_eq!(
                sparse_step
                    .choices
                    .iter()
                    .map(|choice| choice.id)
                    .collect::<Vec<_>>(),
                vec![0, 1, 3]
            );
            assert_eq!(sparse_step.correct_choice_id, 0);
            let mut sparse_failure_ids = sparse_step
                .failure_pages
                .keys()
                .copied()
                .collect::<Vec<_>>();
            sparse_failure_ids.sort_unstable();
            assert_eq!(sparse_failure_ids, vec![1, 3]);
            let quest_10431 = catalog.quest(10_431).expect("record quest 10431");
            assert_eq!(quest_10431.start.record_conditions[0].quest_id, 10_430);
            assert_eq!(
                quest_10431.start.record_conditions[0].alternatives.len(),
                64
            );
            assert!(matches!(
                &quest_10431.start.record_conditions[0].alternatives[0],
                QuestRecordPredicate::Equal(value) if value == "1"
            ));
            let quest_2236 = catalog.quest(2_236).expect("record quest 2236");
            assert!(matches!(
                &quest_2236.completion.record_conditions[0].alternatives[..],
                [QuestRecordPredicate::Equal(value)] if value == "111111"
            ));
            assert_eq!(quest_2236.start_actions.record_writes[0].value, "000000");
            assert_eq!(
                catalog
                    .quest(3_421)
                    .expect("record quest 3421")
                    .start_actions
                    .record_writes[0]
                    .value,
                "000000"
            );
            let quest_4482 = catalog.quest(4_482).expect("record quest 4482");
            assert_eq!(quest_4482.completion.record_conditions[0].quest_id, 4_482);
            assert!(matches!(
                &quest_4482.completion.record_conditions[0].alternatives[..],
                [QuestRecordPredicate::Equal(value)] if value == "3000"
            ));
            assert!(matches!(
                &catalog
                    .quest(10_445)
                    .expect("record quest 10445")
                    .start
                    .record_conditions[0]
                    .alternatives[..],
                [QuestRecordPredicate::AtMost(254)]
            ));
            let quest_28288 = catalog.quest(28_288).expect("inert Act NPC quest");
            assert_eq!(
                quest_28288.start_actions.presentation_npc_id,
                Some(9_201_142)
            );
            assert_eq!(quest_28288.start.npc_id, Some(9_201_028));
            assert_eq!(quest_28288.completion.npc_id, Some(9_201_028));
            for (quest_id, action_name, npc_id) in [
                (2_165, "quest", 1_092_003),
                (2_166, "quest", 1_092_003),
                (2_168, "quest", 1_092_003),
                (2_186, "quest", 1_094_001),
                (2_207, "quest", 1_092_004),
                (2_278, "action", 1_052_124),
                (3_110, "act3110", 2_020_012),
                (3_340, "act3340", 2_111_016),
                (9_803, "act9803", 2_012_023),
                (9_804, "act9803", 2_012_023),
                (9_805, "act9803", 2_012_023),
            ] {
                let quest = catalog
                    .quest(quest_id)
                    .unwrap_or_else(|| panic!("npcAct quest {quest_id}"));
                assert_eq!(quest.completion.npc_id, Some(npc_id));
                assert_eq!(
                    quest.completion_actions.npc_animation_action.as_deref(),
                    Some(action_name)
                );
                assert!(!quest.info.auto_complete && !quest.info.auto_pre_complete);
            }
            let report = catalog.quest_load_report().expect("quest load report");
            let skill_action_quests = catalog
                .quest_definitions()
                .filter(|quest| {
                    !quest.start_actions.skill_changes.is_empty()
                        || !quest.completion_actions.skill_changes.is_empty()
                })
                .count();
            assert_eq!(skill_action_quests, 62);
            let master_only = &catalog
                .quest(6_121)
                .expect("master-only skill quest")
                .completion_actions
                .skill_changes[0];
            assert_eq!(master_only.skill_id, 2_321_003);
            assert_eq!(
                master_only.operation,
                QuestSkillOperation::Grant {
                    skill_level: 0,
                    master_level: 15,
                }
            );
            assert_eq!(master_only.job_ids, vec![232]);
            let quest_6034 = catalog.quest(6_034).expect("audited quest 6034");
            let removal = &quest_6034.start_actions.skill_changes[0];
            assert_eq!(removal.skill_id, 1_007);
            assert_eq!(removal.operation, QuestSkillOperation::Remove);
            assert!(removal.job_ids.is_empty());
            assert!(!quest_6034.start.allowed_jobs.is_empty());
            assert_eq!(quest_6034.completion_actions.next_quest_id, Some(6_035));
            let question = quest_6034
                .dialogue
                .question
                .as_ref()
                .expect("audited quest 6034 completion question");
            assert_eq!(question.steps.len(), 1);
            assert_eq!(question.steps[0].archive_index, 0);
            assert_eq!(
                question.steps[0]
                    .choices
                    .iter()
                    .map(|choice| choice.id)
                    .collect::<Vec<_>>(),
                vec![0, 1]
            );
            assert_eq!(question.steps[0].correct_choice_id, 0);
            assert!(question.steps[0].failure_pages.is_empty());
            assert_eq!(question.trailing_pages.len(), 1);
            assert!(
                question.trailing_pages[0]
                    .to_ascii_lowercase()
                    .contains("illegible")
            );
            let mut quest_state_action_quests = catalog
                .quest_definitions()
                .filter(|quest| {
                    !quest.start_actions.quest_state_actions.is_empty()
                        || !quest.completion_actions.quest_state_actions.is_empty()
                })
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            quest_state_action_quests.sort_unstable();
            assert_eq!(
                quest_state_action_quests,
                vec![
                    2_101, 2_145, 2_199, 2_200, 2_201, 2_202, 2_203, 2_206, 3_081, 3_082, 3_335,
                    3_528, 3_537, 3_642, 3_946, 4_308, 6_034, 20_301,
                ],
                "all 18 otherwise compatible archive quest-state action quests must load",
            );
            let quest_2201 = catalog.quest(2_201).expect("two-target state action quest");
            let quest_2201_actions = quest_2201
                .start_actions
                .quest_state_actions
                .iter()
                .chain(&quest_2201.completion_actions.quest_state_actions)
                .collect::<Vec<_>>();
            assert_eq!(
                quest_2201_actions
                    .iter()
                    .map(|action| action.quest_id)
                    .collect::<Vec<_>>(),
                vec![2_199, 2_200],
            );
            assert!(
                quest_2201_actions
                    .iter()
                    .all(|action| action.state == QuestStateActionState::Completed)
            );
            for blocked in [6_000, 6_012, 6_029] {
                assert!(
                    catalog.quest(blocked).is_none(),
                    "strict skill edge quest {blocked} must remain blocked"
                );
            }
            let check_quests = catalog
                .quest_definitions()
                .filter(|quest| {
                    quest.start.minimum_fame.is_some()
                        || quest.start.minimum_world_id.is_some()
                        || quest.start.maximum_world_id.is_some()
                        || quest.completion.minimum_mesos.is_some()
                        || quest.completion.minimum_completed_quest_count.is_some()
                        || quest.completion.available_from.is_some()
                        || quest.completion.available_until.is_some()
                })
                .map(|quest| quest.id)
                .collect::<Vec<_>>();
            assert_eq!(check_quests.len(), 42);
            assert_eq!(
                [
                    (
                        "pop",
                        catalog
                            .quest_definitions()
                            .filter(|quest| quest.start.minimum_fame.is_some())
                            .count(),
                    ),
                    (
                        "world",
                        catalog
                            .quest_definitions()
                            .filter(|quest| {
                                quest.start.minimum_world_id.is_some()
                                    || quest.start.maximum_world_id.is_some()
                            })
                            .count(),
                    ),
                    (
                        "endmeso",
                        catalog
                            .quest_definitions()
                            .filter(|quest| quest.completion.minimum_mesos.is_some())
                            .count(),
                    ),
                    (
                        "questComplete",
                        catalog
                            .quest_definitions()
                            .filter(|quest| quest
                                .completion
                                .minimum_completed_quest_count
                                .is_some())
                            .count(),
                    ),
                    (
                        "completion calendar",
                        catalog
                            .quest_definitions()
                            .filter(|quest| {
                                quest.completion.available_from.is_some()
                                    || quest.completion.available_until.is_some()
                            })
                            .count(),
                    ),
                ],
                [
                    ("pop", 3),
                    ("world", 29),
                    ("endmeso", 7),
                    ("questComplete", 1),
                    ("completion calendar", 2),
                ]
            );
            for (quest_id, expected_card_ids) in [
                (
                    29_016,
                    vec![2_382_040, 2_382_049, 2_383_002, 2_383_005, 2_383_008],
                ),
                (
                    29_017,
                    vec![2_383_036, 2_384_003, 2_383_037, 2_383_038, 2_388_008],
                ),
                (
                    29_018,
                    vec![2_384_004, 2_384_014, 2_384_018, 2_385_000, 2_385_020],
                ),
                (
                    29_019,
                    vec![2_386_000, 2_386_004, 2_386_010, 2_387_002, 2_387_003],
                ),
            ] {
                let requirements = &catalog
                    .quest(quest_id)
                    .unwrap_or_else(|| panic!("Monster Book quest {quest_id}"))
                    .completion
                    .monster_book;
                assert_eq!(
                    requirements
                        .cards
                        .iter()
                        .map(|card| card.card_item_id)
                        .collect::<Vec<_>>(),
                    expected_card_ids,
                );
                assert!(
                    requirements.cards.iter().all(|card| {
                        card.minimum_count == Some(1) && card.maximum_count.is_none()
                    })
                );
                assert_eq!(requirements.minimum_unique_cards, None);
                assert_eq!(requirements.maximum_unique_cards, None);
            }
            let collector = &catalog
                .quest(29_512)
                .expect("Monster Book unique-card quest")
                .completion
                .monster_book;
            assert!(collector.cards.is_empty());
            assert_eq!(collector.minimum_unique_cards, Some(30));
            assert_eq!(collector.maximum_unique_cards, None);

            assert_eq!(report.compatible_quests, 2_766);
            assert_eq!(
                report.compatible_quests + report.unsupported_reasons.values().sum::<usize>(),
                2_825,
                "every numeric quest ID in Check, Act, Say, or QuestInfo must be classified"
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("selectable item reward")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("start dialogue interaction")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("unreachable start question")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("quiz with selectable reward")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("lost-item restoration dialogue")
            );
            assert!(!report.unsupported_reasons.contains_key("info check"));
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("quest progress action")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("unknown action field")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("quest state action")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("skill removal action")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("expiring equipment item action")
            );
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("item expiry action")
            );
            for field in ["act/0/0", "act/0/yes", "act/0/no", "act/0/end"] {
                assert!(
                    report.retained_metadata_fields.contains_key(field),
                    "retained Act field {field} must be auditable"
                );
            }
            assert!(
                !report
                    .unsupported_reasons
                    .contains_key("lost-item restoration branch structure")
            );
            assert_eq!(report.retained_metadata_fields.get("act/0/npc"), Some(&80));
            assert_eq!(
                report
                    .retained_metadata_fields
                    .iter()
                    .filter(|(path, _)| path.contains("/item/")
                        && (path.ends_with("/name") || path.ends_with("/var")))
                    .map(|(path, count)| (path.clone(), *count))
                    .collect::<BTreeMap<_, _>>(),
                BTreeMap::from([
                    ("act/1/item/0/var".to_owned(), 4),
                    ("act/1/item/1/var".to_owned(), 8),
                    ("act/1/item/10/var".to_owned(), 4),
                    ("act/1/item/11/var".to_owned(), 2),
                    ("act/1/item/12/var".to_owned(), 3),
                    ("act/1/item/13/var".to_owned(), 3),
                    ("act/1/item/14/var".to_owned(), 2),
                    ("act/1/item/15/var".to_owned(), 2),
                    ("act/1/item/2/name".to_owned(), 1),
                    ("act/1/item/2/var".to_owned(), 8),
                    ("act/1/item/3/name".to_owned(), 2),
                    ("act/1/item/3/var".to_owned(), 10),
                    ("act/1/item/4/name".to_owned(), 2),
                    ("act/1/item/4/var".to_owned(), 7),
                    ("act/1/item/5/name".to_owned(), 2),
                    ("act/1/item/5/var".to_owned(), 5),
                    ("act/1/item/6/var".to_owned(), 6),
                    ("act/1/item/7/var".to_owned(), 5),
                    ("act/1/item/8/var".to_owned(), 4),
                    ("act/1/item/9/var".to_owned(), 4),
                ]),
                "every accepted inert item field must remain auditable by its exact path",
            );
            for field in [
                "check/4961",
                "questInfo/4963",
                "act/0/fieldEnter",
                "act/1/item/0/prop=-1",
                "act/1/item/1/prop=-1",
            ] {
                assert_eq!(
                    report.retained_metadata_fields.get(field),
                    Some(&1),
                    "audited field {field} must be retained exactly once"
                );
            }
            assert_eq!(report.retained_metadata_fields.get("selectedMob"), Some(&3));
            for quest_id in [3_077, 4_940, 4_960, 8_833, 9_866, 10_272] {
                assert!(catalog.quest(quest_id).is_some());
            }
            assert_eq!(
                report.unsupported_reasons,
                BTreeMap::from([
                    ("invalid quest data".to_owned(), 27),
                    ("map action".to_owned(), 1),
                    ("map protection item effect".to_owned(), 1),
                    ("party check".to_owned(), 1),
                    ("pet check".to_owned(), 15),
                    ("quest info mechanic".to_owned(), 11),
                    ("unknown check field".to_owned(), 1),
                    ("unknown skill reference".to_owned(), 2),
                ])
            );
            let quest_item_ids = catalog.quest_item_reference_ids().collect::<Vec<_>>();
            eprintln!(
                "compatible quests: {}; quest item references: {}; unsupported reasons: {:?}",
                report.compatible_quests,
                quest_item_ids.len(),
                report.unsupported_reasons
            );
            assert_eq!(quest_item_ids.len(), 2_555);
            for item_id in quest_item_ids {
                assert!(
                    catalog
                        .item_definition(item_id)
                        .unwrap_or_else(|error| panic!("load quest item {item_id}: {error}"))
                        .is_some(),
                    "quest item {item_id} should resolve"
                );
            }
            assert!(
                report.compatible_quests > 100,
                "compatible quest count {} should be substantially above 5",
                report.compatible_quests
            );
            let curve = crate::experience::ExperienceCurves::load(
                &manifest_dir.join("../../config/xp-curves.toml"),
            )
            .expect("XP curves");
            let scripts = crate::quest_scripts::QuestScriptCatalog::default();
            let mut quest_definitions = catalog.quest_definitions().collect::<Vec<_>>();
            quest_definitions.sort_by_key(|quest| quest.id);
            let player = oozems_proto::v1::PlayerState {
                id: "wz-quest-1007".to_owned(),
                level: curve.default_curve().max_level(),
                stats: Some(oozems_proto::v1::CharacterStats {
                    job_id: 0,
                    experience_required: 100,
                    ..oozems_proto::v1::CharacterStats::default()
                }),
                inventory: Some(oozems_proto::v1::InventoryState {
                    capacity: 4,
                    stacks: vec![
                        oozems_proto::v1::InventoryItemStack {
                            item_id: 1_042_003,
                            quantity: 1,
                            expires_at_unix_ms: 0,
                        },
                        oozems_proto::v1::InventoryItemStack {
                            item_id: 4_000_000,
                            quantity: 5,
                            expires_at_unix_ms: 0,
                        },
                        oozems_proto::v1::InventoryItemStack {
                            item_id: 4_000_001,
                            quantity: 1,
                            expires_at_unix_ms: 0,
                        },
                    ],
                    ..oozems_proto::v1::InventoryState::default()
                }),
                ..oozems_proto::v1::PlayerState::default()
            };
            let mut effects = crate::effects::PlayerEffects::default();
            let consume_effects = catalog.consume_effect_definitions();
            let script_blocked_player = oozems_proto::v1::PlayerState {
                id: "wz-quest-10272".to_owned(),
                level: curve.default_curve().max_level(),
                stats: Some(oozems_proto::v1::CharacterStats::default()),
                inventory: Some(oozems_proto::v1::InventoryState {
                    capacity: 2,
                    stacks: vec![
                        oozems_proto::v1::InventoryItemStack {
                            item_id: 4_032_280,
                            quantity: 10,
                            expires_at_unix_ms: 0,
                        },
                        oozems_proto::v1::InventoryItemStack {
                            item_id: 4_032_283,
                            quantity: 10,
                            expires_at_unix_ms: 0,
                        },
                    ],
                    ..oozems_proto::v1::InventoryState::default()
                }),
                quests: vec![oozems_proto::v1::PlayerQuest {
                    quest_id: 10_272,
                    status: oozems_proto::v1::QuestStatus::Started as i32,
                    ..oozems_proto::v1::PlayerQuest::default()
                }],
                ..oozems_proto::v1::PlayerState::default()
            };
            let mut script_effects = crate::effects::PlayerEffects::default();
            assert!(matches!(
                crate::quests::select_choice(
                    script_blocked_player,
                    &mut script_effects,
                    malformed_item_metadata,
                    &quest_definitions,
                    9_000_021,
                    crate::quests::COMPLETE_CHOICE_ID,
                    curve.default_curve(),
                    catalog.item_definition_slice(),
                    &consume_effects,
                    &scripts,
                    crate::quests::QuestEnvironment {
                        now_unix_ms: 1_000,
                        world_id: gameplay.world_id,
                    },
                ),
                Err(crate::quests::QuestRuleError::ScriptRequired {
                    quest_id: 10_272,
                    phase: crate::quest_scripts::QuestScriptPhase::Completion,
                    script,
                }) if script == "q10272e"
            ));
            let accepted = crate::quests::select_choice(
                player,
                &mut effects,
                quest,
                &quest_definitions,
                20_002,
                crate::quests::ACCEPT_CHOICE_ID,
                curve.default_curve(),
                catalog.item_definition_slice(),
                &consume_effects,
                &scripts,
                crate::quests::QuestEnvironment {
                    now_unix_ms: 1_000,
                    world_id: gameplay.world_id,
                },
            )
            .expect("accept WZ quest 1007");
            let completed = crate::quests::select_choice(
                accepted.player,
                &mut effects,
                quest,
                &quest_definitions,
                20_002,
                crate::quests::COMPLETE_CHOICE_ID,
                curve.default_curve(),
                catalog.item_definition_slice(),
                &consume_effects,
                &scripts,
                crate::quests::QuestEnvironment {
                    now_unix_ms: 2_000,
                    world_id: gameplay.world_id,
                },
            )
            .expect("complete WZ quest 1007")
            .player;
            let inventory = completed.inventory.as_ref().expect("inventory");
            assert_eq!(
                crate::items::count_item_quantity(&inventory.stacks, 4_000_000),
                Ok(0)
            );
            assert_eq!(
                crate::items::count_item_quantity(&inventory.stacks, 4_000_001),
                Ok(0)
            );
            let dagger_count = crate::items::count_item_quantity(&inventory.stacks, 1_332_005)
                .expect("old gladius count")
                + crate::items::count_item_quantity(&inventory.stacks, 1_332_007)
                    .expect("fruit knife count");
            assert_eq!(dagger_count, 1);
            assert_eq!(completed.stats.expect("stats").experience, 50);
        }
        assert_eq!(starter_map.name, "Mushroom Town");
        assert!(starter_map.portals.iter().any(|portal| portal.kind == 0));
        let snail_garden = catalog
            .get_map(20_000)
            .expect("adjacent map lookup should succeed")
            .expect("Snail Garden should exist");
        let entrance = snail_garden
            .portals
            .iter()
            .find(|portal| portal.name == "in00")
            .expect("Mushroom Town entrance portal");
        let movement_bounds = snail_garden
            .movement_bounds
            .as_ref()
            .expect("Snail Garden movement bounds");
        assert!(movement_bounds.left <= entrance.x);
        assert!(entrance.x <= movement_bounds.right);
        let map = catalog
            .get_map(100_000_000)
            .expect("WZ map lookup should succeed")
            .expect("Henesys should exist");

        assert_eq!(map.name, "Henesys");
        assert!(
            map.platforms
                .iter()
                .any(|platform| platform.x == platform.end_x)
        );
        assert!(map.platforms.iter().any(|platform| platform.layer == 0));
        assert!(map.platforms.iter().any(|platform| platform.layer == 1));
        assert!(!map.decorations.is_empty());
        assert!(
            map.decorations
                .iter()
                .any(|decoration| decoration.layer > 1)
        );
        assert!(map.decorations.iter().any(|decoration| {
            decoration.frames.len() == 9
                && decoration.frames.iter().all(|frame| frame.delay_ms == 130)
        }));
        assert!(map.decorations.iter().all(|decoration| {
            decoration
                .frames
                .iter()
                .all(|frame| map.assets.iter().any(|asset| asset.id == frame.asset_id))
        }));
        assert!(!map.ladders.is_empty());
        assert!(map.portals.iter().any(|portal| {
            portal.name == "east00"
                && portal.target_map_id == 100_010_000
                && portal.target_name == "west00"
                && portal.layer == 1
                && !portal.frames.is_empty()
        }));
        if wz_dir.join("Npc.wz").exists() {
            assert!(!map.npcs.iter().any(|npc| npc.npc_id == 9_000_017));
            let athena = map
                .npcs
                .iter()
                .find(|npc| npc.npc_id == 1_012_000)
                .expect("Athena Pierce NPC");
            assert!(athena.position.is_some());
            let frames = athena
                .animations
                .iter()
                .find(|animation| animation.name == "stand")
                .or_else(|| athena.animations.first())
                .expect("Athena Pierce animation")
                .frames
                .as_slice();
            assert!(!frames.is_empty());
            assert!(athena.layer > 0);
            assert!(frames.iter().all(|frame| {
                frame.delay_ms > 0 && map.assets.iter().any(|asset| asset.id == frame.asset_id)
            }));
        }
        if wz_dir.join("Mob.wz").exists() {
            let mob_map = catalog
                .get_map(100_010_000)
                .expect("mob map lookup should succeed")
                .expect("Henesys Hunting Ground should exist");
            let movement_bounds = mob_map
                .movement_bounds
                .as_ref()
                .expect("WZ movement bounds");
            assert!(movement_bounds.left > 0.0);
            assert!(movement_bounds.right < mob_map.width as f32);
            assert!(movement_bounds.left < movement_bounds.right);
            assert!(
                mob_map
                    .mob_spawn_points
                    .iter()
                    .any(|spawn| spawn.mob_id == 100_101 && spawn.position.is_some())
            );
            let slime = mob_map
                .mob_definitions
                .iter()
                .find(|definition| definition.id == 100_101)
                .expect("slime definition");
            assert_eq!(slime.level, 2);
            assert_eq!(slime.max_hp, 15);
            assert!(slime.animations.iter().any(|animation| {
                animation.name == "move"
                    && animation.frames.len() == 4
                    && animation.frames.iter().all(|frame| frame.delay_ms == 120)
            }));
            assert!(
                slime
                    .animations
                    .iter()
                    .flat_map(|animation| &animation.frames)
                    .all(|frame| mob_map
                        .assets
                        .iter()
                        .any(|asset| asset.id == frame.asset_id))
            );
        }
        if wz_dir.join("Character.wz").exists() && wz_dir.join("UI.wz").exists() {
            let gui = catalog.game_gui();
            eprintln!(
                "game GUI item payload: {} items, {} total assets",
                gui.items.len(),
                gui.assets.len()
            );
            assert!(gui.items.len() >= 6);
            assert!(gui.equipment_window.is_some());
            assert!(gui.inventory_window.is_some());
            let asset_ids = gui
                .assets
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<HashSet<_>>();
            assert!(
                gui.items
                    .iter()
                    .all(|item| { asset_ids.contains(item.icon_asset_id.as_str()) })
            );
            assert!(
                gui.items
                    .iter()
                    .all(|item| { catalog.get_wz_asset(&item.icon_asset_id).is_some() })
            );
            assert_eq!(
                gui.items
                    .iter()
                    .filter(|item| item.appearance_supported)
                    .count(),
                6
            );
        }
        assert!(
            catalog
                .item_definition(u32::MAX)
                .expect("unsupported item lookup should not fail")
                .is_none()
        );
        for descriptor in &map.assets {
            let asset = catalog
                .get_wz_asset(&descriptor.id)
                .expect("map asset should be registered");
            let png = asset.png_bytes().expect("map asset should decode");
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }
}
