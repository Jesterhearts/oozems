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
mod skill;
mod wz;

use character::CharacterContent;
use character::CharacterContentError;
pub use config::ContentConfig;
use gui::GuiContent;
use gui::GuiContentError;
use skill::SkillContent;
use skill::SkillContentError;
pub(crate) use wz::WzAsset;
use wz::WzContent;
use wz::WzContentError;

const PLAYER_HALF_WIDTH: f32 = 18.0;

pub struct ContentCatalog {
    characters: Option<CharacterContent>,
    gui: Option<GuiContent>,
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
    Skill(#[from] SkillContentError),
}

impl ContentCatalog {
    pub fn load(
        wz_dir: &Path,
        config: &ContentConfig,
    ) -> Result<Self, ContentError> {
        let wz = WzContent::open(wz_dir, config.npcs.clone())?;
        Ok(Self {
            characters: CharacterContent::open_optional(wz_dir)?,
            gui: GuiContent::open_optional(wz_dir)?,
            skills: SkillContent::open_optional(wz_dir)?,
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
        if let Some(characters) = &self.characters {
            gui.assets.extend(characters.item_assets());
            gui.items = characters.item_definitions();
        }
        gui
    }

    pub fn character_creation_options(&self) -> CharacterCreationOptions {
        self.characters
            .as_ref()
            .map(CharacterContent::creation_options)
            .unwrap_or_default()
    }

    pub fn skill_book(
        &self,
        job_id: u32,
    ) -> Result<SkillBook, ContentError> {
        self.skills
            .as_ref()
            .map(|source| source.skill_book(job_id))
            .transpose()
            .map(|book| {
                book.unwrap_or_else(|| SkillBook {
                    job_id,
                    name: "Skills".to_owned(),
                    ..SkillBook::default()
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

    pub fn item_definitions(&self) -> Vec<ItemDefinition> {
        self.characters
            .as_ref()
            .map(CharacterContent::item_definitions)
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ContentCatalog;
    use super::ContentConfig;

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
            assert!(!athena.frames.is_empty());
            assert!(athena.layer > 0);
            assert!(athena.frames.iter().all(|frame| {
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
            assert_eq!(gui.items.len(), 6);
            assert!(gui.equipment_window.is_some());
            assert!(gui.inventory_window.is_some());
            assert!(gui.items.iter().all(|item| {
                gui.assets
                    .iter()
                    .any(|asset| asset.id == item.icon_asset_id)
            }));
        }
        for descriptor in &map.assets {
            let asset = catalog
                .get_wz_asset(&descriptor.id)
                .expect("map asset should be registered");
            let png = asset.png_bytes().expect("map asset should decode");
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }
}
