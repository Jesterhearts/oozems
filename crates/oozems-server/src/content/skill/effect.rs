use std::collections::HashSet;
use std::sync::Arc;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::SkillAnimation;
use oozems_proto::v1::SkillAnimationFrame;
use oozems_proto::v1::SkillAnimationPlacement;
use oozems_proto::v1::SkillEffect;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::SKILL_ARCHIVE;
use super::SkillContent;
use super::SkillContentError;
use super::invalid;
use super::parse_node_id;
use super::required_child;
use super::wz;

const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) fn build(
    content: &SkillContent,
    job_id: u32,
    skill_id: u32,
    level: u32,
) -> Result<SkillEffect, SkillContentError> {
    let skill = find_skill(content, job_id, skill_id)?;
    let level_source = wz::child(&skill, "level")?
        .as_ref()
        .map(|levels| wz::child(levels, &level.to_string()))
        .transpose()?
        .flatten();

    let mut assets = Vec::new();
    let mut animations = Vec::new();
    push_animation(
        content,
        &skill,
        level_source.as_ref(),
        "effect",
        SkillAnimationPlacement::Caster,
        0,
        &mut animations,
        &mut assets,
    )?;
    push_animation(
        content,
        &skill,
        level_source.as_ref(),
        "ball",
        SkillAnimationPlacement::Projectile,
        0,
        &mut animations,
        &mut assets,
    )?;
    let projectile_duration = animations
        .iter()
        .filter(|animation| animation.placement == SkillAnimationPlacement::Projectile as i32)
        .map(animation_duration_ms)
        .max()
        .unwrap_or_default();
    push_animation(
        content,
        &skill,
        level_source.as_ref(),
        "hit",
        SkillAnimationPlacement::Target,
        projectile_duration,
        &mut animations,
        &mut assets,
    )?;

    let mut asset_ids = HashSet::new();
    assets.retain(|asset| asset_ids.insert(asset.id.clone()));
    let sound = build_sound(content, skill_id)?;
    Ok(SkillEffect {
        animations,
        assets,
        sound,
    })
}

fn find_skill(
    content: &SkillContent,
    job_id: u32,
    skill_id: u32,
) -> Result<WzNodeArc, SkillContentError> {
    let Some(job) = content.jobs.get(&job_id) else {
        return invalid(format!("job {job_id} has no WZ skill data"));
    };
    wz::parse(job, format!("{SKILL_ARCHIVE}/{job_id:03}.img"))?;
    let skills = required_child(job, "skill")?;
    for skill in wz::sorted_children(&skills)? {
        if parse_node_id(&skill, "skill")? == skill_id {
            return Ok(skill);
        }
    }
    invalid(format!("job {job_id} does not contain skill {skill_id}"))
}

fn push_animation(
    content: &SkillContent,
    skill: &WzNodeArc,
    level_source: Option<&WzNodeArc>,
    node_name: &str,
    placement: SkillAnimationPlacement,
    start_delay_ms: u32,
    animations: &mut Vec<SkillAnimation>,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<(), SkillContentError> {
    let source = find_animation_source(skill, level_source, node_name)?;
    let Some(source) = source else {
        return Ok(());
    };
    let frame_nodes = find_frame_sequence(&source, 0)?;
    if frame_nodes.is_empty() {
        return Ok(());
    }

    let mut frames = Vec::with_capacity(frame_nodes.len());
    for node in frame_nodes {
        let source_path = wz::node_path(&node)?;
        let descriptor = content.register_asset(&source_path, &node)?;
        frames.push(build_frame(&node, descriptor.id.clone())?);
        assets.push(descriptor);
    }
    animations.push(SkillAnimation {
        placement: placement as i32,
        frames,
        start_delay_ms,
    });
    Ok(())
}

fn find_animation_source(
    skill: &WzNodeArc,
    level_source: Option<&WzNodeArc>,
    node_name: &str,
) -> Result<Option<WzNodeArc>, SkillContentError> {
    if let Some(source) = level_source
        && let Some(node) = find_named_child(source, node_name)?
    {
        return Ok(Some(node));
    }
    if let Some(common) = wz::child(skill, "common")?
        && let Some(node) = find_named_child(&common, node_name)?
    {
        return Ok(Some(node));
    }
    find_named_child(skill, node_name)
}

fn find_named_child(
    parent: &WzNodeArc,
    requested_name: &str,
) -> Result<Option<WzNodeArc>, SkillContentError> {
    let children = wz::sorted_children(parent)?;
    for child in &children {
        if wz::node_name(child)?.eq_ignore_ascii_case(requested_name) {
            return Ok(Some(Arc::clone(child)));
        }
    }
    for child in children {
        if wz::node_name(&child)?
            .to_ascii_lowercase()
            .starts_with(requested_name)
        {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn find_frame_sequence(
    node: &WzNodeArc,
    depth: usize,
) -> Result<Vec<WzNodeArc>, SkillContentError> {
    if is_png(node)? {
        return Ok(vec![Arc::clone(node)]);
    }
    if depth >= 8 {
        return Ok(Vec::new());
    }

    let numbered_children = wz::sorted_children(node)?
        .into_iter()
        .filter(|child| wz::node_name(child).is_ok_and(|name| name.parse::<u32>().is_ok()))
        .collect::<Vec<_>>();
    let mut frames = Vec::new();
    for child in &numbered_children {
        if is_png(child)? {
            frames.push(Arc::clone(child));
        }
    }
    if !frames.is_empty() {
        return Ok(frames);
    }
    for child in numbered_children {
        let frames = find_frame_sequence(&child, depth + 1)?;
        if !frames.is_empty() {
            return Ok(frames);
        }
    }
    Ok(Vec::new())
}

fn is_png(node: &WzNodeArc) -> Result<bool, SkillContentError> {
    Ok(node
        .read()
        .map_err(|_| super::lock_error("skill animation frame"))?
        .try_as_png()
        .is_some())
}

fn build_frame(
    node: &WzNodeArc,
    asset_id: String,
) -> Result<SkillAnimationFrame, SkillContentError> {
    let (width, height) = {
        let read = node
            .read()
            .map_err(|_| super::lock_error("skill animation geometry"))?;
        let png = read
            .try_as_png()
            .ok_or_else(|| SkillContentError::Invalid {
                message: format!("{} is not a PNG frame", read.get_full_path()),
            })?;
        (png.width as f32, png.height as f32)
    };
    let Vector2D(origin_x, origin_y) = wz::child(node, "origin")?
        .as_ref()
        .map(wz::vector_value)
        .transpose()?
        .flatten()
        .unwrap_or(Vector2D(0, 0));
    let delay_ms = wz::int_value(node, "delay")?
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_FRAME_DELAY_MS);
    Ok(SkillAnimationFrame {
        asset_id,
        width,
        height,
        origin_x: origin_x as f32,
        origin_y: origin_y as f32,
        delay_ms,
    })
}

fn build_sound(
    content: &SkillContent,
    skill_id: u32,
) -> Result<Option<AssetDescriptor>, SkillContentError> {
    Ok(content
        .sounds
        .as_ref()
        .map(|sounds| sounds.skill_use(skill_id))
        .transpose()?
        .flatten())
}

fn animation_duration_ms(animation: &SkillAnimation) -> u32 {
    animation.frames.iter().fold(0, |duration, frame| {
        duration.saturating_add(frame.delay_ms.max(1))
    })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::SkillAnimation;
    use oozems_proto::v1::SkillAnimationFrame;

    use super::animation_duration_ms;

    #[test]
    fn animation_duration_sums_nonzero_frame_delays() {
        let animation = SkillAnimation {
            frames: vec![
                SkillAnimationFrame {
                    delay_ms: 90,
                    ..SkillAnimationFrame::default()
                },
                SkillAnimationFrame {
                    delay_ms: 0,
                    ..SkillAnimationFrame::default()
                },
            ],
            ..SkillAnimation::default()
        };

        assert_eq!(animation_duration_ms(&animation), 91);
    }
}
