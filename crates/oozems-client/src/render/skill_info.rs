use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::SkillDefinition;
use web_sys::CanvasRenderingContext2d;

use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui;
use crate::game_gui::CanvasPoint;

const BUFF_MARGIN: f32 = 8.0;
const BUFF_GAP: f32 = 4.0;
const BUFF_WIDTH: f32 = 36.0;
const BUFF_HEIGHT: f32 = 47.0;
const BUFF_ICON_SIZE: f32 = 32.0;
const TOOLTIP_WIDTH: f32 = 270.0;
const TOOLTIP_PADDING: f32 = 9.0;
const TOOLTIP_LINE_HEIGHT: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct BuffPlacement {
    index: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug, PartialEq, Eq)]
struct SkillInfo {
    title: String,
    level: String,
    description: Option<String>,
    current: Option<String>,
    next: Option<String>,
    remaining: Option<String>,
}

struct TooltipLine {
    text: String,
    color: &'static str,
    font: &'static str,
}

pub(super) fn draw_active_buffs(game: &Game) {
    let placements = buff_placements(game.active_buffs.buffs.len(), game.canvas.width() as f32);
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    for placement in placements {
        let buff = &game.active_buffs.buffs[placement.index];
        draw_buff_background(game, placement);
        if let Some(definition) = skill_definition(game, buff.skill_id) {
            draw_buff_icon(game, definition, placement);
        }
        draw_buff_time(game, buff, placement, now_ms);
    }
}

pub(super) fn draw_hovered_skill(game: &Game) {
    let Some(pointer) = game.pointer else {
        return;
    };
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    let hovered_book_skill = game_gui::hovered_skill(
        *game.gui_state.borrow(),
        &game.gui,
        &game.skill_book,
        pointer,
    )
    .and_then(|skill| {
        skill
            .definition
            .as_ref()
            .map(|definition| (definition, skill.level, None))
    });
    let hovered_buff = hovered_buff(game, pointer).and_then(|buff| {
        skill_definition(game, buff.skill_id).map(|definition| {
            (
                definition,
                buff.skill_level,
                Some(buff.expires_at_unix_ms.saturating_sub(now_ms)),
            )
        })
    });
    let Some((definition, level, remaining_ms)) = hovered_book_skill.or(hovered_buff) else {
        return;
    };
    let info = build_skill_info(definition, level, remaining_ms);
    draw_tooltip(game, pointer, &info);
}

fn draw_buff_background(
    game: &Game,
    placement: BuffPlacement,
) {
    game.context.set_fill_style_str("rgba(6, 13, 20, 0.9)");
    game.context.fill_rect(
        f64::from(placement.x),
        f64::from(placement.y),
        f64::from(placement.width),
        f64::from(placement.height),
    );
    game.context.set_fill_style_str("#b7c8d4");
    game.context.fill_rect(
        f64::from(placement.x + 1.0),
        f64::from(placement.y + 1.0),
        f64::from(placement.width - 2.0),
        f64::from(BUFF_ICON_SIZE + 2.0),
    );
    game.context.set_fill_style_str("#263442");
    game.context.fill_rect(
        f64::from(placement.x + 2.0),
        f64::from(placement.y + 2.0),
        f64::from(BUFF_ICON_SIZE),
        f64::from(BUFF_ICON_SIZE),
    );
}

fn draw_buff_icon(
    game: &Game,
    definition: &SkillDefinition,
    placement: BuffPlacement,
) {
    let Some(image) = ready_image(&game.images, &definition.icon_asset_id) else {
        return;
    };
    let scale = (BUFF_ICON_SIZE / definition.icon_width.max(1.0))
        .min(BUFF_ICON_SIZE / definition.icon_height.max(1.0))
        .min(1.0);
    let width = definition.icon_width * scale;
    let height = definition.icon_height * scale;
    let x = placement.x + 2.0 + (BUFF_ICON_SIZE - width) / 2.0;
    let y = placement.y + 2.0 + (BUFF_ICON_SIZE - height) / 2.0;
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height),
        );
}

fn draw_buff_time(
    game: &Game,
    buff: &ActiveBuff,
    placement: BuffPlacement,
    now_ms: u64,
) {
    let remaining_ms = buff.expires_at_unix_ms.saturating_sub(now_ms);
    game.context.set_fill_style_str("#ffffff");
    game.context.set_font("bold 9px Arial");
    game.context.set_text_align("center");
    let _ = game.context.fill_text_with_max_width(
        &format_short_duration(remaining_ms),
        f64::from(placement.x + placement.width / 2.0),
        f64::from(placement.y + placement.height - 3.0),
        f64::from(placement.width - 2.0),
    );
    game.context.set_text_align("left");
}

fn hovered_buff(
    game: &Game,
    point: CanvasPoint,
) -> Option<&ActiveBuff> {
    buff_placements(game.active_buffs.buffs.len(), game.canvas.width() as f32)
        .into_iter()
        .find(|placement| contains(*placement, point))
        .map(|placement| &game.active_buffs.buffs[placement.index])
}

fn buff_placements(
    count: usize,
    viewport_width: f32,
) -> Vec<BuffPlacement> {
    (0..count)
        .map(|index| BuffPlacement {
            index,
            x: viewport_width - BUFF_MARGIN - BUFF_WIDTH - index as f32 * (BUFF_WIDTH + BUFF_GAP),
            y: BUFF_MARGIN,
            width: BUFF_WIDTH,
            height: BUFF_HEIGHT,
        })
        .collect()
}

fn contains(
    placement: BuffPlacement,
    point: CanvasPoint,
) -> bool {
    point.x >= placement.x
        && point.x < placement.x + placement.width
        && point.y >= placement.y
        && point.y < placement.y + placement.height
}

fn skill_definition(
    game: &Game,
    skill_id: u32,
) -> Option<&SkillDefinition> {
    game.skill_book
        .skills
        .iter()
        .filter_map(|skill| skill.definition.as_ref())
        .find(|definition| definition.skill_id == skill_id)
}

fn build_skill_info(
    definition: &SkillDefinition,
    level: u32,
    remaining_ms: Option<u64>,
) -> SkillInfo {
    let current = (level > 0)
        .then(|| level_description(definition, level))
        .flatten();
    let next = definition
        .levels
        .iter()
        .filter(|candidate| candidate.level > level)
        .min_by_key(|candidate| candidate.level)
        .and_then(|candidate| {
            nonempty(candidate.description.clone())
                .map(|description| (candidate.level, description))
        })
        .map(|(next_level, description)| format!("Level {next_level}: {description}"));
    SkillInfo {
        title: definition.name.clone(),
        level: format!("Level {level}/{}", definition.max_level),
        description: nonempty(definition.description.clone()),
        current: current.map(|description| format!("Current: {description}")),
        next: next.map(|description| format!("Next: {description}")),
        remaining: remaining_ms
            .map(|duration| format!("Remaining: {}", format_long_duration(duration))),
    }
}

fn level_description(
    definition: &SkillDefinition,
    level: u32,
) -> Option<String> {
    definition
        .levels
        .iter()
        .find(|candidate| candidate.level == level)
        .and_then(|candidate| nonempty(candidate.description.clone()))
}

fn nonempty(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

fn draw_tooltip(
    game: &Game,
    pointer: CanvasPoint,
    info: &SkillInfo,
) {
    let content_width = TOOLTIP_WIDTH - TOOLTIP_PADDING * 2.0;
    let lines = tooltip_lines(&game.context, info, content_width);
    let height = TOOLTIP_PADDING * 2.0 + lines.len() as f32 * TOOLTIP_LINE_HEIGHT;
    let (x, y) = tooltip_position(
        pointer,
        TOOLTIP_WIDTH,
        height,
        game.canvas.width() as f32,
        game.canvas.height() as f32,
    );

    game.context.set_fill_style_str("rgba(5, 12, 20, 0.94)");
    game.context.fill_rect(
        f64::from(x),
        f64::from(y),
        f64::from(TOOLTIP_WIDTH),
        f64::from(height),
    );
    game.context.set_fill_style_str("#8da1ae");
    game.context.fill_rect(
        f64::from(x + 1.0),
        f64::from(y + 1.0),
        f64::from(TOOLTIP_WIDTH - 2.0),
        1.0,
    );
    for (index, line) in lines.iter().enumerate() {
        game.context.set_fill_style_str(line.color);
        game.context.set_font(line.font);
        let baseline = y + TOOLTIP_PADDING + (index + 1) as f32 * TOOLTIP_LINE_HEIGHT - 3.0;
        let _ = game.context.fill_text(
            &line.text,
            f64::from(x + TOOLTIP_PADDING),
            f64::from(baseline),
        );
    }
}

fn tooltip_lines(
    context: &CanvasRenderingContext2d,
    info: &SkillInfo,
    width: f32,
) -> Vec<TooltipLine> {
    let mut lines = Vec::new();
    push_wrapped(
        &mut lines,
        context,
        &info.title,
        width,
        "#ffffff",
        "bold 12px Arial",
    );
    push_wrapped(
        &mut lines,
        context,
        &info.level,
        width,
        "#aebdca",
        "10px Arial",
    );
    for (text, color) in [
        (info.description.as_deref(), "#ffffff"),
        (info.current.as_deref(), "#f5d36a"),
        (info.next.as_deref(), "#8ee39b"),
        (info.remaining.as_deref(), "#7ed9ff"),
    ] {
        if let Some(text) = text {
            push_wrapped(&mut lines, context, text, width, color, "10px Arial");
        }
    }
    lines
}

fn push_wrapped(
    lines: &mut Vec<TooltipLine>,
    context: &CanvasRenderingContext2d,
    text: &str,
    width: f32,
    color: &'static str,
    font: &'static str,
) {
    context.set_font(font);
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            let fits = context
                .measure_text(&candidate)
                .map_or(true, |metrics| metrics.width() <= f64::from(width));
            if !fits && !line.is_empty() {
                lines.push(TooltipLine {
                    text: line,
                    color,
                    font,
                });
                line = word.to_owned();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            lines.push(TooltipLine {
                text: line,
                color,
                font,
            });
        }
    }
}

fn tooltip_position(
    pointer: CanvasPoint,
    width: f32,
    height: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32) {
    let maximum_x = (viewport_width - width - 4.0).max(4.0);
    let x = (pointer.x + 12.0).clamp(4.0, maximum_x);
    let below = pointer.y + 12.0;
    let y = if below + height <= viewport_height - 4.0 {
        below
    } else {
        pointer.y - height - 12.0
    };
    (x, y.clamp(4.0, (viewport_height - height - 4.0).max(4.0)))
}

fn format_short_duration(duration_ms: u64) -> String {
    let seconds = duration_ms.saturating_add(999) / 1_000;
    if seconds >= 60 {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn format_long_duration(duration_ms: u64) -> String {
    let seconds = duration_ms.saturating_add(999) / 1_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::SkillLevelDefinition;

    use super::*;

    #[test]
    fn skill_info_includes_current_next_and_remaining_details() {
        let definition = SkillDefinition {
            name: "Haste".to_owned(),
            description: "Improves movement.".to_owned(),
            max_level: 20,
            levels: vec![
                SkillLevelDefinition {
                    level: 2,
                    description: "Speed +4".to_owned(),
                    ..SkillLevelDefinition::default()
                },
                SkillLevelDefinition {
                    level: 3,
                    description: "Speed +6".to_owned(),
                    ..SkillLevelDefinition::default()
                },
            ],
            ..SkillDefinition::default()
        };

        let info = build_skill_info(&definition, 2, Some(61_000));

        assert_eq!(info.title, "Haste");
        assert_eq!(info.level, "Level 2/20");
        assert_eq!(info.current.as_deref(), Some("Current: Speed +4"));
        assert_eq!(info.next.as_deref(), Some("Next: Level 3: Speed +6"));
        assert_eq!(info.remaining.as_deref(), Some("Remaining: 1:01"));
    }

    #[test]
    fn unlearned_skill_shows_the_first_level_as_next() {
        let definition = SkillDefinition {
            name: "Recovery".to_owned(),
            max_level: 3,
            levels: vec![SkillLevelDefinition {
                level: 1,
                description: "Recover 10 HP".to_owned(),
                ..SkillLevelDefinition::default()
            }],
            ..SkillDefinition::default()
        };

        let info = build_skill_info(&definition, 0, None);

        assert_eq!(info.current, None);
        assert_eq!(info.next.as_deref(), Some("Next: Level 1: Recover 10 HP"));
    }

    #[test]
    fn buff_icons_are_placed_from_right_to_left() {
        let placements = buff_placements(2, 800.0);

        assert_eq!(placements[0].x, 756.0);
        assert_eq!(placements[1].x, 716.0);
        assert!(contains(placements[0], CanvasPoint { x: 760.0, y: 20.0 }));
    }

    #[test]
    fn durations_round_up_to_avoid_showing_zero_early() {
        assert_eq!(format_short_duration(1), "1s");
        assert_eq!(format_short_duration(60_001), "1:01");
        assert_eq!(format_long_duration(120_000), "2:00");
    }

    #[test]
    fn tooltip_is_kept_inside_the_viewport() {
        let position = tooltip_position(
            CanvasPoint { x: 790.0, y: 590.0 },
            270.0,
            120.0,
            800.0,
            600.0,
        );

        assert_eq!(position, (526.0, 458.0));
    }
}
