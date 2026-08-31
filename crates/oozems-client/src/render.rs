use crate::game::Game;
use crate::game_gui;

mod background;
mod cash_shop;
mod cursor;
mod death;
mod hud;
mod interaction;
mod mob;
pub(crate) mod npc;
mod quest_journal;
mod quest_tracker;
mod reactor;
mod skill_info;
mod skillbook;
mod world;

use hud::draw_item_icon_in_region;
use hud::draw_item_quantity_in_region;
use hud::draw_window;
use hud::draw_window_at;
use hud::item_definition;
use hud::item_expiration;
use hud::item_expiration_detail;
use hud::permanent_stack_needs_label;
pub(crate) use world::draw_sprite;
pub(crate) use world::npc_at_point;
pub(crate) use world::player_canvas_position;
pub(crate) use world::sprite_is_visible;
pub(crate) use world::world_layers;

pub fn draw(game: &Game) {
    if game.ui.cash_shop.open {
        hud::draw_cash_shop(game);
    } else {
        world::draw(game);
        hud::draw(game);
    }
    crate::level_up_effect::draw(game);
    death::draw(game);
    cursor::draw(game);
}

pub(crate) fn select_active_buff(
    game: &mut Game,
    point: game_gui::CanvasPoint,
) -> bool {
    skill_info::select_active_buff(game, point)
}

pub(crate) fn active_buff_at_point(
    game: &Game,
    point: game_gui::CanvasPoint,
) -> bool {
    skill_info::active_buff_at_point(game, point)
}

#[cfg(test)]
use hud::inventory_expiration_label;
#[cfg(test)]
use world::LayerPass;
#[cfg(test)]
use world::decoration_frame_index;
#[cfg(test)]
use world::layer_passes;
#[cfg(test)]
use world::portal_frame_index;

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Decoration;
    use oozems_proto::v1::DecorationFrame;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Mob;
    use oozems_proto::v1::Npc;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::PortalFrame;
    use oozems_proto::v1::ReactorSpawnPoint;

    use super::LayerPass;
    use super::decoration_frame_index;
    use super::inventory_expiration_label;
    use super::item_expiration;
    use super::item_expiration_detail;
    use super::layer_passes;
    use super::permanent_stack_needs_label;
    use super::portal_frame_index;
    use super::world_layers;

    #[test]
    fn world_layers_include_each_layer_source_in_order() {
        let map = Map {
            decorations: vec![Decoration {
                layer: 3,
                ..Decoration::default()
            }],
            platforms: vec![Platform {
                layer: 1,
                ..Platform::default()
            }],
            ladders: vec![Ladder {
                layer: 4,
                ..Ladder::default()
            }],
            portals: vec![Portal {
                layer: 2,
                ..Portal::default()
            }],
            mobs: vec![Mob {
                layer: 5,
                ..Mob::default()
            }],
            npcs: vec![Npc {
                layer: 6,
                ..Npc::default()
            }],
            reactor_spawn_points: vec![ReactorSpawnPoint {
                layer: 7,
                ..ReactorSpawnPoint::default()
            }],
            ..Map::default()
        };

        assert_eq!(world_layers(&map), vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn portals_render_after_decorations_on_their_layer() {
        assert_eq!(
            layer_passes(false),
            &[
                LayerPass::Decorations,
                LayerPass::Portals,
                LayerPass::Npcs,
                LayerPass::Reactors,
                LayerPass::Mobs,
            ]
        );
        assert_eq!(
            layer_passes(true),
            &[
                LayerPass::Decorations,
                LayerPass::Portals,
                LayerPass::Npcs,
                LayerPass::Reactors,
                LayerPass::Mobs,
                LayerPass::DroppedItems,
                LayerPass::Player,
                LayerPass::SkillEffects,
            ]
        );
    }

    #[test]
    fn portal_animation_uses_each_frame_delay() {
        let frames = vec![
            PortalFrame {
                delay_ms: 100,
                ..PortalFrame::default()
            },
            PortalFrame {
                delay_ms: 200,
                ..PortalFrame::default()
            },
        ];

        assert_eq!(portal_frame_index(&frames, 99.0), Some(0));
        assert_eq!(portal_frame_index(&frames, 100.0), Some(1));
        assert_eq!(portal_frame_index(&frames, 299.0), Some(1));
        assert_eq!(portal_frame_index(&frames, 300.0), Some(0));
    }

    #[test]
    fn decoration_animation_uses_each_frame_delay() {
        let frames = vec![
            DecorationFrame {
                delay_ms: 130,
                ..DecorationFrame::default()
            },
            DecorationFrame {
                delay_ms: 260,
                ..DecorationFrame::default()
            },
        ];

        assert_eq!(decoration_frame_index(&frames, 129.0), Some(0));
        assert_eq!(decoration_frame_index(&frames, 130.0), Some(1));
        assert_eq!(decoration_frame_index(&frames, 389.0), Some(1));
        assert_eq!(decoration_frame_index(&frames, 390.0), Some(0));
    }

    #[test]
    fn item_expiration_labels_distinguish_deadlines_without_labeling_all_permanent_items() {
        assert_eq!(
            inventory_expiration_label(item_expiration(0, 1_000), false),
            None
        );
        assert_eq!(
            inventory_expiration_label(item_expiration(0, 1_000), true).as_deref(),
            Some("PERM")
        );
        assert_eq!(
            inventory_expiration_label(item_expiration(1_000, 1_000), false).as_deref(),
            Some("EXP")
        );
        assert_eq!(
            inventory_expiration_label(item_expiration(61_000, 1_000), false).as_deref(),
            Some("1m")
        );
        assert_eq!(
            inventory_expiration_label(item_expiration(121_001, 1_000), false).as_deref(),
            Some("3m")
        );
        assert_eq!(
            item_expiration_detail(item_expiration(61_000, 1_000), false).as_deref(),
            Some("1 minute left")
        );

        let stacks = vec![
            InventoryItemStack {
                item_id: 1,
                expires_at_unix_ms: 0,
                ..InventoryItemStack::default()
            },
            InventoryItemStack {
                item_id: 1,
                expires_at_unix_ms: 10_000,
                ..InventoryItemStack::default()
            },
            InventoryItemStack {
                item_id: 2,
                expires_at_unix_ms: 0,
                ..InventoryItemStack::default()
            },
        ];
        assert!(permanent_stack_needs_label(&stacks, &stacks[0]));
        assert!(!permanent_stack_needs_label(&stacks, &stacks[2]));
    }
}
