use std::collections::HashSet;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::Ladder;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Portal;
use oozems_proto::v1::PortalFrame;
use wz_reader::WzNodeArc;

use super::Bounds;
use super::RawSprite;
use super::WzContent;
use super::WzContentError;
use super::child;
use super::find_png_descendant;
use super::int_value;
use super::raw_sprite;
use super::sorted_children;
use super::string_value;

const VISIBLE_PORTAL_KIND: u32 = 2;
const DEFAULT_PORTAL_FRAME_DELAY_MS: u32 = 100;

pub(super) struct RawLadder {
    pub(super) x: i32,
    pub(super) y1: i32,
    pub(super) y2: i32,
    is_ladder: bool,
    upper_floor: bool,
    layer: i32,
}

pub(super) struct RawPortal {
    pub(super) name: String,
    pub(super) x: i32,
    pub(super) y: i32,
    target_map_id: u32,
    target_name: String,
    kind: u32,
    pub(super) frames: Vec<RawPortalFrame>,
}

pub(super) struct RawPortalFrame {
    pub(super) sprite: RawSprite,
    delay_ms: u32,
}

pub(super) fn read_ladders(map: &WzNodeArc) -> Result<Vec<RawLadder>, WzContentError> {
    let Some(parent) = child(map, "ladderRope")? else {
        return Ok(Vec::new());
    };

    let mut ladders = Vec::new();
    for node in sorted_children(&parent)? {
        if let Some(ladder) = read_ladder(&node)? {
            ladders.push(ladder);
        }
    }
    Ok(ladders)
}

fn read_ladder(node: &WzNodeArc) -> Result<Option<RawLadder>, WzContentError> {
    let (Some(x), Some(y1), Some(y2)) = (
        int_value(node, "x")?,
        int_value(node, "y1")?,
        int_value(node, "y2")?,
    ) else {
        return Ok(None);
    };

    Ok(Some(RawLadder {
        x,
        y1,
        y2,
        is_ladder: int_value(node, "l")?.unwrap_or_default() != 0,
        upper_floor: int_value(node, "uf")?.unwrap_or_default() != 0,
        layer: int_value(node, "page")?.unwrap_or_default(),
    }))
}

pub(super) fn read_portals(
    root: &WzNodeArc,
    map: &WzNodeArc,
) -> Result<Vec<RawPortal>, WzContentError> {
    let Some(parent) = child(map, "portal")? else {
        return Ok(Vec::new());
    };

    let mut portals = Vec::new();
    for node in sorted_children(&parent)? {
        let Some(mut portal) = read_portal(&node)? else {
            continue;
        };
        if portal.kind == VISIBLE_PORTAL_KIND {
            portal.frames = read_visible_portal_frames(root, portal.x, portal.y)?;
        }
        portals.push(portal);
    }
    Ok(portals)
}

fn read_portal(node: &WzNodeArc) -> Result<Option<RawPortal>, WzContentError> {
    let (Some(name), Some(x), Some(y), Some(target_map_id), Some(target_name), Some(kind)) = (
        string_value(node, "pn")?,
        int_value(node, "x")?,
        int_value(node, "y")?,
        int_value(node, "tm")?,
        string_value(node, "tn")?,
        int_value(node, "pt")?,
    ) else {
        return Ok(None);
    };

    Ok(Some(RawPortal {
        name,
        x,
        y,
        target_map_id: u32::try_from(target_map_id).unwrap_or(u32::MAX),
        target_name,
        kind: u32::try_from(kind).unwrap_or_default(),
        frames: Vec::new(),
    }))
}

fn read_visible_portal_frames(
    root: &WzNodeArc,
    anchor_x: i32,
    anchor_y: i32,
) -> Result<Vec<RawPortalFrame>, WzContentError> {
    let Some(parent) = child_at_path(root, "MapHelper.img/portal/game/pv")? else {
        return Ok(Vec::new());
    };

    let mut frames = Vec::new();
    for node in sorted_children(&parent)? {
        let Some(source) = find_png_descendant(&node, 0)? else {
            continue;
        };
        let delay_ms = int_value(&source, "delay")?
            .and_then(|value| u32::try_from(value).ok())
            .filter(|delay| *delay > 0)
            .unwrap_or(DEFAULT_PORTAL_FRAME_DELAY_MS);
        frames.push(RawPortalFrame {
            sprite: raw_sprite(source, anchor_x, anchor_y, false)?,
            delay_ms,
        });
    }
    Ok(frames)
}

fn child_at_path(
    root: &WzNodeArc,
    path: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    match root
        .read()
        .map_err(|_| super::lock_error("WZ feature asset path"))?
        .at_path_parsed(path)
    {
        Ok(node) => Ok(Some(node)),
        Err(wz_reader::node::Error::NodeNotFound) => Ok(None),
        Err(source) => Err(WzContentError::Parse {
            context: path.to_owned(),
            source,
        }),
    }
}

pub(super) fn build_ladders(
    source: Vec<RawLadder>,
    bounds: Bounds,
) -> Vec<Ladder> {
    source
        .into_iter()
        .map(|ladder| Ladder {
            x: (ladder.x - bounds.left) as f32,
            top: (ladder.y1.min(ladder.y2) - bounds.top) as f32,
            bottom: (ladder.y1.max(ladder.y2) - bounds.top) as f32,
            is_ladder: ladder.is_ladder,
            upper_floor: ladder.upper_floor,
            layer: ladder.layer,
        })
        .collect()
}

pub(super) fn build_portals(
    content: &WzContent,
    source: Vec<RawPortal>,
    platforms: &[Platform],
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<Portal>, WzContentError> {
    source
        .into_iter()
        .map(|portal| build_portal(content, portal, platforms, bounds, assets, asset_ids))
        .collect()
}

fn build_portal(
    content: &WzContent,
    source: RawPortal,
    platforms: &[Platform],
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Portal, WzContentError> {
    let mut frames = Vec::with_capacity(source.frames.len());
    for frame in source.frames {
        let asset = content.register_asset(&frame.sprite.source_path, &frame.sprite.source)?;
        if asset_ids.insert(asset.id.clone()) {
            assets.push(asset.clone());
        }
        frames.push(PortalFrame {
            asset_id: asset.id,
            x: (frame.sprite.x - bounds.left) as f32,
            y: (frame.sprite.y - bounds.top) as f32,
            width: frame.sprite.width as f32,
            height: frame.sprite.height as f32,
            delay_ms: frame.delay_ms,
        });
    }

    let x = (source.x - bounds.left) as f32;
    let y = (source.y - bounds.top) as f32;
    Ok(Portal {
        name: source.name,
        x,
        y,
        target_map_id: source.target_map_id,
        target_name: source.target_name,
        kind: source.kind,
        frames,
        layer: attached_platform_layer(platforms, x, y),
    })
}

fn attached_platform_layer(
    platforms: &[Platform],
    portal_x: f32,
    portal_y: f32,
) -> i32 {
    platforms
        .iter()
        .filter_map(|platform| {
            let surface = platform_surface_at_x(platform, portal_x)?;
            Some((platform.layer, (surface - portal_y).abs(), surface))
        })
        .min_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then_with(|| right.2.total_cmp(&left.2))
        })
        .map_or(0, |(layer, _, _)| layer)
}

fn platform_surface_at_x(
    platform: &Platform,
    x: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if !(minimum_x..=maximum_x).contains(&x) {
        return None;
    }
    let width = platform.end_x - platform.x;
    if width.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / width;
    Some(platform.y + progress * (platform.end_y - platform.y))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Platform;
    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::property::WzString;

    use super::Bounds;
    use super::attached_platform_layer;
    use super::build_ladders;
    use super::read_ladder;
    use super::read_portal;

    #[test]
    fn ladder_fields_become_bounded_collision_data() {
        let node = WzNode::from_str("0", 0, None).into_lock();
        for (name, value) in [
            ("x", 120),
            ("y1", 80),
            ("y2", 240),
            ("l", 1),
            ("uf", 1),
            ("page", 3),
        ] {
            add(
                &node,
                WzNode::from_str(name, value, Some(&node)).into_lock(),
            );
        }

        let raw = read_ladder(&node).expect("read ladder").expect("ladder");
        let ladders = build_ladders(
            vec![raw],
            Bounds {
                left: -20,
                top: -40,
                right: 500,
                bottom: 500,
            },
        );

        assert_eq!(ladders[0].x, 140.0);
        assert_eq!(ladders[0].top, 120.0);
        assert_eq!(ladders[0].bottom, 280.0);
        assert!(ladders[0].is_ladder);
        assert!(ladders[0].upper_floor);
        assert_eq!(ladders[0].layer, 3);
    }

    #[test]
    fn portal_fields_preserve_the_direct_destination() {
        let node = WzNode::from_str("0", 0, None).into_lock();
        for (name, value) in [("x", 120), ("y", 240), ("tm", 100_010_000), ("pt", 2)] {
            add(
                &node,
                WzNode::from_str(name, value, Some(&node)).into_lock(),
            );
        }
        for (name, value) in [("pn", "east00"), ("tn", "west00")] {
            add(
                &node,
                WzNode::from_str(name, WzString::from_str(value, [0; 4]), Some(&node)).into_lock(),
            );
        }

        let portal = read_portal(&node).expect("read portal").expect("portal");

        assert_eq!(portal.name, "east00");
        assert_eq!(portal.target_map_id, 100_010_000);
        assert_eq!(portal.target_name, "west00");
        assert_eq!(portal.kind, 2);
    }

    #[test]
    fn portal_uses_the_layer_of_its_nearest_supporting_platform() {
        let platforms = vec![
            Platform {
                x: 100.0,
                y: 100.0,
                end_x: 300.0,
                end_y: 100.0,
                layer: 1,
                ..Platform::default()
            },
            Platform {
                x: 100.0,
                y: 300.0,
                end_x: 300.0,
                end_y: 300.0,
                layer: 3,
                ..Platform::default()
            },
        ];

        assert_eq!(attached_platform_layer(&platforms, 200.0, 290.0), 3);
    }

    fn add(
        parent: &WzNodeArc,
        child: WzNodeArc,
    ) {
        parent.write().expect("parent lock").add(&child);
    }
}
