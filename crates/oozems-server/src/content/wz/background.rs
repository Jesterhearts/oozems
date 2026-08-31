use std::collections::HashSet;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::MapBackground;
use oozems_proto::v1::MapBackgroundFrame;
use oozems_proto::v1::MapBackgroundMode;
use wz_reader::WzNodeArc;

use super::Bounds;
use super::RawDecorationFrame;
use super::WzContent;
use super::WzContentError;
use super::child;
use super::int_value;
use super::raw_decoration_frame;
use super::resolve_png_frames;
use super::sorted_children;
use super::string_value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawBackgroundMode {
    Regular,
    HorizontalTiling,
    VerticalTiling,
    HorizontalAndVerticalTiling,
    HorizontalMoving,
    VerticalMoving,
    HorizontalMovingWithTiling,
    VerticalMovingWithTiling,
}

pub(super) struct RawBackground {
    frames: Vec<RawDecorationFrame>,
    horizontal_rate: i32,
    vertical_rate: i32,
    repeat_x: u32,
    repeat_y: u32,
    alpha: u32,
    mode: RawBackgroundMode,
    front: bool,
    flip_x: bool,
}

pub(super) fn read_backgrounds(
    root: &WzNodeArc,
    map: &WzNodeArc,
    map_id: u32,
) -> Result<Vec<RawBackground>, WzContentError> {
    let Some(parent) = child(map, "back")? else {
        return Ok(Vec::new());
    };

    let mut backgrounds = Vec::new();
    for node in sorted_children(&parent)? {
        if string_value(&node, "spineAni")?.is_some_and(|name| !name.is_empty()) {
            tracing::warn!(map_id, "skipping unsupported WZ Spine background");
            continue;
        }
        if let Some(background) = read_background(root, &node, map_id)? {
            backgrounds.push(background);
        }
    }
    Ok(backgrounds)
}

fn read_background(
    root: &WzNodeArc,
    node: &WzNodeArc,
    map_id: u32,
) -> Result<Option<RawBackground>, WzContentError> {
    let (Some(background_set), Some(number)) = (string_value(node, "bS")?, int_value(node, "no")?)
    else {
        return Ok(None);
    };
    let animated = int_value(node, "ani")?.unwrap_or_default() != 0;
    let group = if animated { "ani" } else { "back" };
    let path = format!("Back/{background_set}.img/{group}/{number}");
    let sources = resolve_png_frames(root, &path)?;
    if sources.is_empty() {
        tracing::debug!(map_id, %path, "skipping WZ background without a PNG source");
        return Ok(None);
    }

    let x = int_value(node, "x")?.unwrap_or_default();
    let y = int_value(node, "y")?.unwrap_or_default();
    let flip_x = int_value(node, "f")?.unwrap_or_default() != 0;
    let mode_value = int_value(node, "type")?.unwrap_or_default();
    let Some(mode) = parse_mode(mode_value) else {
        tracing::warn!(
            map_id,
            mode = mode_value,
            "skipping WZ background with an unsupported type"
        );
        return Ok(None);
    };
    let frames = sources
        .into_iter()
        .map(|source| raw_decoration_frame(source, x, y, flip_x))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(RawBackground {
        frames,
        horizontal_rate: int_value(node, "rx")?.unwrap_or_default(),
        vertical_rate: int_value(node, "ry")?.unwrap_or_default(),
        repeat_x: nonnegative(int_value(node, "cx")?.unwrap_or_default()),
        repeat_y: nonnegative(int_value(node, "cy")?.unwrap_or_default()),
        alpha: int_value(node, "a")?.unwrap_or(255).clamp(0, 255) as u32,
        mode,
        front: int_value(node, "front")?.unwrap_or_default() != 0,
        flip_x,
    }))
}

fn nonnegative(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn parse_mode(value: i32) -> Option<RawBackgroundMode> {
    match value {
        0 => Some(RawBackgroundMode::Regular),
        1 => Some(RawBackgroundMode::HorizontalTiling),
        2 => Some(RawBackgroundMode::VerticalTiling),
        3 => Some(RawBackgroundMode::HorizontalAndVerticalTiling),
        4 => Some(RawBackgroundMode::HorizontalMoving),
        5 => Some(RawBackgroundMode::VerticalMoving),
        6 => Some(RawBackgroundMode::HorizontalMovingWithTiling),
        7 => Some(RawBackgroundMode::VerticalMovingWithTiling),
        _ => None,
    }
}

pub(super) fn build_backgrounds(
    content: &WzContent,
    source: Vec<RawBackground>,
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<MapBackground>, WzContentError> {
    source
        .into_iter()
        .map(|background| build_background(content, background, bounds, assets, asset_ids))
        .collect()
}

fn build_background(
    content: &WzContent,
    source: RawBackground,
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<MapBackground, WzContentError> {
    let mut frames = Vec::with_capacity(source.frames.len());
    for frame in source.frames {
        let asset = content.register_asset(&frame.sprite.source_path, &frame.sprite.source)?;
        if asset_ids.insert(asset.id.clone()) {
            assets.push(asset.clone());
        }
        frames.push(MapBackgroundFrame {
            asset_id: asset.id,
            x: parallax_base(frame.sprite.x, source.horizontal_rate, bounds.left),
            y: parallax_base(frame.sprite.y, source.vertical_rate, bounds.top),
            width: frame.sprite.width as f32,
            height: frame.sprite.height as f32,
            delay_ms: frame.delay_ms,
        });
    }

    Ok(MapBackground {
        frames,
        horizontal_rate: source.horizontal_rate,
        vertical_rate: source.vertical_rate,
        repeat_x: source.repeat_x,
        repeat_y: source.repeat_y,
        alpha: source.alpha,
        mode: proto_mode(source.mode) as i32,
        front: source.front,
        flip_x: source.flip_x,
    })
}

fn parallax_base(
    sprite_position: i32,
    rate: i32,
    map_origin: i32,
) -> f32 {
    sprite_position as f32 + rate as f32 * map_origin as f32 / 100.0
}

fn proto_mode(mode: RawBackgroundMode) -> MapBackgroundMode {
    match mode {
        RawBackgroundMode::Regular => MapBackgroundMode::Regular,
        RawBackgroundMode::HorizontalTiling => MapBackgroundMode::HorizontalTiling,
        RawBackgroundMode::VerticalTiling => MapBackgroundMode::VerticalTiling,
        RawBackgroundMode::HorizontalAndVerticalTiling => {
            MapBackgroundMode::HorizontalAndVerticalTiling
        }
        RawBackgroundMode::HorizontalMoving => MapBackgroundMode::HorizontalMoving,
        RawBackgroundMode::VerticalMoving => MapBackgroundMode::VerticalMoving,
        RawBackgroundMode::HorizontalMovingWithTiling => {
            MapBackgroundMode::HorizontalMovingWithTiling
        }
        RawBackgroundMode::VerticalMovingWithTiling => MapBackgroundMode::VerticalMovingWithTiling,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::RwLock;

    use oozems_proto::v1::MapBackgroundMode;
    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::property::Vector2D;
    use wz_reader::property::WzPng;
    use wz_reader::property::WzString;

    use super::Bounds;
    use super::RawBackgroundMode;
    use super::WzContent;
    use super::build_backgrounds;
    use super::parallax_base;
    use super::parse_mode;
    use super::proto_mode;
    use super::read_backgrounds;
    use crate::content::config::NpcFilter;

    #[test]
    fn static_and_animated_backgrounds_resolve_frames_in_wz_order() {
        let root = WzNode::from_str("Map.wz", 0, None).into_lock();
        let background_directory = add_branch(&root, "Back");
        let background_set = add_branch(&background_directory, "forest.img");
        let static_group = add_branch(&background_set, "back");
        add_canvas(&static_group, "2", 40, 30, Vector2D(10, 20), 75);
        let animated_group = add_branch(&background_set, "ani");
        let animation = add_branch(&animated_group, "3");
        add_canvas(&animation, "1", 60, 50, Vector2D(2, 4), 150);
        add_canvas(&animation, "0", 50, 40, Vector2D(1, 3), 100);

        let map = WzNode::from_str("map", 0, None).into_lock();
        let instances = add_branch(&map, "back");
        let animated = add_branch(&instances, "1");
        add_background_reference(&animated, 3, true);
        add_integer(&animated, "x", -5);
        add_integer(&animated, "y", 8);
        add_integer(&animated, "type", 6);
        let static_background = add_branch(&instances, "0");
        add_background_reference(&static_background, 2, false);
        add_integer(&static_background, "x", 100);
        add_integer(&static_background, "y", 200);
        add_integer(&static_background, "f", 1);
        add_integer(&static_background, "rx", -40);
        add_integer(&static_background, "ry", -20);
        add_integer(&static_background, "cx", -1);
        add_integer(&static_background, "cy", 70);
        add_integer(&static_background, "a", 128);
        add_integer(&static_background, "front", 1);
        let unsupported = add_branch(&instances, "2");
        add_background_reference(&unsupported, 2, false);
        add_integer(&unsupported, "type", 8);

        let backgrounds = read_backgrounds(&root, &map, 100_000_000).expect("backgrounds");

        assert_eq!(backgrounds.len(), 2);
        let first = &backgrounds[0];
        assert_eq!(first.frames.len(), 1);
        assert_eq!(first.frames[0].sprite.x, 70);
        assert_eq!(first.frames[0].sprite.y, 180);
        assert_eq!(first.frames[0].delay_ms, 75);
        assert_eq!(first.horizontal_rate, -40);
        assert_eq!(first.vertical_rate, -20);
        assert_eq!(first.repeat_x, 0);
        assert_eq!(first.repeat_y, 70);
        assert_eq!(first.alpha, 128);
        assert_eq!(first.mode, RawBackgroundMode::Regular);
        assert!(first.front);
        assert!(first.flip_x);

        let second = &backgrounds[1];
        assert_eq!(second.frames.len(), 2);
        assert_eq!(second.frames[0].sprite.width, 50);
        assert_eq!(second.frames[0].delay_ms, 100);
        assert_eq!(second.frames[1].sprite.width, 60);
        assert_eq!(second.frames[1].delay_ms, 150);
        assert_eq!(second.alpha, 255);
        assert_eq!(second.mode, RawBackgroundMode::HorizontalMovingWithTiling);

        let content = test_content(&root);
        let mut assets = Vec::new();
        let mut asset_ids = HashSet::new();
        let built = build_backgrounds(
            &content,
            backgrounds,
            Bounds {
                left: -1_000,
                top: -500,
                right: 1_000,
                bottom: 500,
            },
            &mut assets,
            &mut asset_ids,
        )
        .expect("built backgrounds");

        assert_eq!(built.len(), 2);
        assert_eq!(built[0].frames[0].x, 470.0);
        assert_eq!(built[0].frames[0].y, 280.0);
        assert_eq!(built[0].alpha, 128);
        assert!(built[0].front);
        assert!(built[0].flip_x);
        assert_eq!(built[1].frames.len(), 2);
        assert_eq!(assets.len(), 3);
        assert_eq!(asset_ids.len(), 3);
        assert_eq!(content.assets.read().expect("assets").len(), 3);
    }

    #[test]
    fn wz_background_modes_have_explicit_protocol_values() {
        let expected = [
            MapBackgroundMode::Regular,
            MapBackgroundMode::HorizontalTiling,
            MapBackgroundMode::VerticalTiling,
            MapBackgroundMode::HorizontalAndVerticalTiling,
            MapBackgroundMode::HorizontalMoving,
            MapBackgroundMode::VerticalMoving,
            MapBackgroundMode::HorizontalMovingWithTiling,
            MapBackgroundMode::VerticalMovingWithTiling,
        ];

        for (value, expected) in expected.into_iter().enumerate() {
            let raw = parse_mode(value as i32).expect("supported mode");
            assert_eq!(proto_mode(raw), expected);
        }
        assert_eq!(parse_mode(8), None);
    }

    #[test]
    fn parallax_base_accounts_for_the_normalized_map_origin() {
        assert_eq!(parallax_base(100, 0, -1_000), 100.0);
        assert_eq!(parallax_base(100, -40, -1_000), 500.0);
        assert_eq!(parallax_base(100, -100, -1_000), 1_100.0);
    }

    #[test]
    fn every_raw_mode_is_covered() {
        let modes = [
            RawBackgroundMode::Regular,
            RawBackgroundMode::HorizontalTiling,
            RawBackgroundMode::VerticalTiling,
            RawBackgroundMode::HorizontalAndVerticalTiling,
            RawBackgroundMode::HorizontalMoving,
            RawBackgroundMode::VerticalMoving,
            RawBackgroundMode::HorizontalMovingWithTiling,
            RawBackgroundMode::VerticalMovingWithTiling,
        ];

        assert!(
            modes
                .into_iter()
                .all(|mode| proto_mode(mode) != MapBackgroundMode::Unspecified)
        );
    }

    fn add_background_reference(
        parent: &WzNodeArc,
        number: i32,
        animated: bool,
    ) {
        let value = WzString::from_str("forest", [0; 4]);
        let background_set = WzNode::from_str("bS", value, Some(parent)).into_lock();
        add(parent, background_set);
        add_integer(parent, "no", number);
        add_integer(parent, "ani", i32::from(animated));
    }

    fn add_canvas(
        parent: &WzNodeArc,
        name: &str,
        width: u32,
        height: u32,
        origin: Vector2D,
        delay_ms: i32,
    ) {
        let mut png = WzPng::default();
        png.width = width;
        png.height = height;
        let canvas = WzNode::from_str(name, png, Some(parent)).into_lock();
        add(parent, Arc::clone(&canvas));
        let origin = WzNode::from_str("origin", origin, Some(&canvas)).into_lock();
        add(&canvas, origin);
        add_integer(&canvas, "delay", delay_ms);
    }

    fn add_integer(
        parent: &WzNodeArc,
        name: &str,
        value: i32,
    ) {
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        add(parent, child);
    }

    fn add_branch(
        parent: &WzNodeArc,
        name: &str,
    ) -> WzNodeArc {
        let child = WzNode::from_str(name, 0, Some(parent)).into_lock();
        add(parent, Arc::clone(&child));
        child
    }

    fn add(
        parent: &WzNodeArc,
        child: WzNodeArc,
    ) {
        parent.write().expect("parent lock").add(&child);
    }

    fn test_content(root: &WzNodeArc) -> WzContent {
        WzContent {
            _base: Arc::clone(root),
            root: Arc::clone(root),
            map_nodes: HashMap::new(),
            map_names: HashMap::new(),
            fingerprint: "test-fingerprint".to_owned(),
            maps: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
            mobs: None,
            npcs: None,
            reactors: None,
            sounds: None,
            npc_filter: NpcFilter::default(),
        }
    }
}
