use std::path::Path;
use std::time::Duration;

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
mod effect;
mod gui;
mod item;
mod morph;
mod quest;
mod skill;
mod sound;
mod wz;

use character::CharacterContent;
use character::CharacterContentError;
pub use config::ContentConfig;
use effect::EffectContent;
use effect::EffectContentError;
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
use sound::SoundContent;
pub(crate) use wz::WzAsset;
use wz::WzContent;
use wz::WzContentError;

const PLAYER_HALF_WIDTH: f32 = 18.0;

pub struct ContentCatalog {
    characters: Option<CharacterContent>,
    effects: Option<EffectContent>,
    gui: Option<GuiContent>,
    items: Option<ItemContent>,
    morphs: Option<MorphContent>,
    quests: Option<QuestContent>,
    skills: Option<SkillContent>,
    sounds: Option<std::sync::Arc<SoundContent>>,
    wz: WzContent,
}

#[derive(Debug, Error)]
pub enum ContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error(transparent)]
    Character(#[from] CharacterContentError),
    #[error(transparent)]
    Effect(#[from] EffectContentError),
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
        gui_layout_dir: &Path,
        config: &ContentConfig,
    ) -> Result<Self, ContentError> {
        let sounds = SoundContent::open_optional(wz_dir)?;
        let wz = WzContent::open(wz_dir, config.npcs.clone(), sounds.clone())?;
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
        let skills = SkillContent::open_optional(wz_dir, sounds.clone())?;
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
            effects: EffectContent::open_optional(wz_dir)?,
            gui: GuiContent::open_optional(wz_dir, gui_layout_dir)?,
            items,
            morphs,
            quests,
            skills,
            sounds,
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
                self.effects
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
            .or_else(|| {
                self.sounds
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
    }

    pub fn game_gui(
        &self,
        item_ids: &std::collections::BTreeSet<u32>,
    ) -> Result<GameGui, ContentError> {
        let mut gui = self
            .gui
            .as_ref()
            .map(GuiContent::game_gui)
            .unwrap_or_default();
        if let Some(items) = &self.items {
            let (definitions, assets) = items.gui_projection(item_ids)?;
            gui.assets.extend(assets);
            gui.items = definitions;
            tracing::info!(
                item_count = gui.items.len(),
                asset_count = gui.assets.len(),
                "game GUI payload ready"
            );
        }
        if let Some(effects) = &self.effects {
            let (frames, assets) = effects.tomb_projection();
            gui.death_tomb_frames = frames;
            gui.assets.extend(assets);
            let (frames, assets) = effects.level_up_projection();
            gui.level_up_frames = frames;
            gui.assets.extend(assets);
        }
        Ok(gui)
    }

    pub fn character_creation_options(&self) -> CharacterCreationOptions {
        let mut options = self
            .characters
            .as_ref()
            .map(CharacterContent::creation_options)
            .unwrap_or_default();
        options.equipment = crate::items::starter_equipment_options(self.item_definition_slice());
        options
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

    pub(crate) fn mob_definitions(
        &self,
        mob_ids: &std::collections::BTreeSet<u32>,
    ) -> Vec<oozems_proto::v1::MobDefinition> {
        self.wz.mob_definitions(mob_ids)
    }

    pub(crate) fn consume_effect_definitions(&self) -> Vec<ConsumeEffectDefinition> {
        self.items
            .as_ref()
            .map(ItemContent::consume_effect_definitions)
            .unwrap_or_default()
    }

    pub(crate) fn consume_effect_definition(
        &self,
        item_id: u32,
    ) -> Option<ConsumeEffectDefinition> {
        self.items
            .as_ref()
            .and_then(|items| items.consume_effect_definition(item_id))
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

    pub(crate) fn quest(
        &self,
        quest_id: u32,
    ) -> Option<&QuestDefinition> {
        self.quests.as_ref()?.get(quest_id)
    }

    pub(crate) fn quest_definitions(&self) -> impl Iterator<Item = &QuestDefinition> {
        // QuestContent uses a BTreeMap so every consumer observes quest-ID order.
        self.quests.iter().flat_map(|quests| quests.definitions())
    }

    pub(crate) fn quest_script_reference_names(
        &self
    ) -> Option<&std::collections::BTreeSet<String>> {
        self.quests
            .as_ref()
            .map(QuestContent::script_reference_names)
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

    pub fn basic_attack_reach(
        &self,
        equipment: &[EquippedItem],
    ) -> Option<crate::attacks::AttackReach> {
        self.characters.as_ref()?.basic_attack_reach(equipment)
    }

    pub fn basic_attack_duration(
        &self,
        appearance: &CharacterAppearance,
    ) -> Result<Option<Duration>, ContentError> {
        self.characters
            .as_ref()
            .map(|source| source.basic_attack_duration(appearance))
            .transpose()
            .map(|duration| duration.flatten())
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
    use super::ContentCatalog;
    use super::ContentConfig;

    #[test]
    fn map_archive_is_required() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let error = ContentCatalog::load(
            directory.path(),
            &directory.path().join("gui"),
            &ContentConfig::default(),
        )
        .err()
        .expect("missing Map.wz must fail");

        assert!(error.to_string().contains("Map.wz is required"));
    }
}
