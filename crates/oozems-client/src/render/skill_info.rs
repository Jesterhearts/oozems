use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::SkillDefinition;
use oozems_proto::v1::active_buff;
use web_sys::CanvasRenderingContext2d;

use crate::assets::ready_image;
use crate::game::Game;
use crate::game::buffs::TrackedBuff;
use crate::game_gui;
use crate::game_gui::CanvasPoint;

const BUFF_MARGIN: f32 = 8.0;
const BUFF_GAP: f32 = 4.0;
const BUFF_WIDTH: f32 = 36.0;
const BUFF_HEIGHT: f32 = 47.0;
const BUFF_ICON_SIZE: f32 = 32.0;
const MAX_SCALED_BUFF_ICON_SIZE: f32 = 96.0;
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
    icon_size: f32,
    timer_font_size: f32,
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
    spans: Vec<TooltipSpan>,
    font: &'static str,
}

struct TooltipSpan {
    text: String,
    color: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
struct WzTextFragment {
    text: String,
    highlighted: bool,
}

pub(super) fn draw_active_buffs(game: &Game) {
    let placements = buff_placements(
        game.active_buffs.buffs.len(),
        game.canvas.width() as f32,
        game.canvas.client_width() as f32,
    );
    let now_ms = game.frame_time_ms;
    for placement in placements {
        let buff = &game.active_buffs.buffs[placement.index];
        draw_buff_background(game, placement);
        match buff.source {
            Some(active_buff::Source::ItemId(item_id)) => {
                if let Some(definition) = item_definition(game, item_id) {
                    draw_buff_asset(
                        game,
                        &definition.icon_asset_id,
                        definition.icon_width,
                        definition.icon_height,
                        placement,
                    );
                }
            }
            Some(active_buff::Source::SourceSkillId(skill_id)) => {
                if let Some(definition) = skill_definition(game, skill_id) {
                    draw_buff_icon(game, definition, placement);
                }
            }
            None => {
                if let Some(definition) = skill_definition(game, buff.skill_id) {
                    draw_buff_icon(game, definition, placement);
                }
            }
        }
        draw_buff_time(game, buff, placement, now_ms);
    }
}

pub(super) fn draw_hovered_skill(game: &Game) {
    let hovered = game.pointer.and_then(|pointer| {
        let book = game_gui::hovered_skill(
            *game.gui_state.borrow(),
            &game.gui,
            &game.skill_book,
            pointer,
        )
        .and_then(|skill| {
            skill.definition.as_ref().map(|definition| {
                build_skill_info(definition, skill.level, skill.master_level, None)
            })
        });
        book.or_else(|| {
            hovered_buff(game, pointer).and_then(|buff| buff_info(game, buff, game.frame_time_ms))
        })
        .map(|info| (pointer, info))
    });
    let selected = game.selected_buff.and_then(|key| {
        let placements = active_buff_placements(game);
        placements.into_iter().find_map(|placement| {
            let buff = &game.active_buffs.buffs[placement.index];
            (buff.key() == key).then(|| {
                let point = CanvasPoint {
                    x: placement.x + placement.width / 2.0,
                    y: placement.y + placement.height,
                };
                buff_info(game, buff, game.frame_time_ms).map(|info| (point, info))
            })?
        })
    });
    let Some((pointer, info)) = hovered.or(selected) else {
        return;
    };
    draw_tooltip(game, pointer, &info);
}

pub(super) fn select_active_buff(
    game: &mut Game,
    point: CanvasPoint,
) -> bool {
    let selected = active_buff_placements(game)
        .into_iter()
        .find(|placement| contains(*placement, point))
        .map(|placement| game.active_buffs.buffs[placement.index].key());
    let Some(selected) = selected else {
        game.selected_buff = None;
        game.pointer = None;
        return false;
    };
    game.selected_buff = toggled_buff_selection(game.selected_buff, selected);
    game.pointer = game.selected_buff.map(|_| point);
    true
}

fn toggled_buff_selection(
    current: Option<crate::game::buffs::BuffKey>,
    tapped: crate::game::buffs::BuffKey,
) -> Option<crate::game::buffs::BuffKey> {
    (current != Some(tapped)).then_some(tapped)
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
        f64::from(placement.icon_size + 2.0),
    );
    game.context.set_fill_style_str("#263442");
    game.context.fill_rect(
        f64::from(placement.x + 2.0),
        f64::from(placement.y + 2.0),
        f64::from(placement.icon_size),
        f64::from(placement.icon_size),
    );
}

fn draw_buff_icon(
    game: &Game,
    definition: &SkillDefinition,
    placement: BuffPlacement,
) {
    draw_buff_asset(
        game,
        &definition.icon_asset_id,
        definition.icon_width,
        definition.icon_height,
        placement,
    );
}

fn draw_buff_asset(
    game: &Game,
    asset_id: &str,
    icon_width: f32,
    icon_height: f32,
    placement: BuffPlacement,
) {
    let Some(image) = ready_image(&game.images, asset_id) else {
        return;
    };
    let scale =
        (placement.icon_size / icon_width.max(1.0)).min(placement.icon_size / icon_height.max(1.0));
    let width = icon_width * scale;
    let height = icon_height * scale;
    let x = placement.x + 2.0 + (placement.icon_size - width) / 2.0;
    let y = placement.y + 2.0 + (placement.icon_size - height) / 2.0;
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
    buff: &TrackedBuff,
    placement: BuffPlacement,
    now_ms: f64,
) {
    let remaining_ms = buff.remaining_ms(now_ms);
    game.context.set_fill_style_str("#ffffff");
    game.context
        .set_font(&format!("bold {}px Arial", placement.timer_font_size));
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
) -> Option<&TrackedBuff> {
    active_buff_placements(game)
        .into_iter()
        .find(|placement| contains(*placement, point))
        .map(|placement| &game.active_buffs.buffs[placement.index])
}

fn buff_placements(
    count: usize,
    viewport_width: f32,
    client_width: f32,
) -> Vec<BuffPlacement> {
    let scale = if client_width > 0.0 && viewport_width > 0.0 {
        client_width / viewport_width
    } else {
        1.0
    };
    let inverse_scale = 1.0 / scale.max(0.01);
    let icon_size =
        (BUFF_ICON_SIZE * inverse_scale).clamp(BUFF_ICON_SIZE, MAX_SCALED_BUFF_ICON_SIZE);
    let margin = (BUFF_MARGIN * inverse_scale).clamp(BUFF_MARGIN, 20.0);
    let gap = (BUFF_GAP * inverse_scale).clamp(BUFF_GAP, 12.0);
    let width = icon_size + ((BUFF_WIDTH - BUFF_ICON_SIZE) * inverse_scale).clamp(4.0, 12.0);
    let timer_height = ((BUFF_HEIGHT - BUFF_ICON_SIZE) * inverse_scale).clamp(15.0, 45.0);
    let height = icon_size + timer_height;
    let timer_font_size = (9.0 * inverse_scale).clamp(9.0, 27.0);
    let columns = (((viewport_width - margin * 2.0 + gap) / (width + gap)).floor() as usize).max(1);
    (0..count)
        .map(|index| {
            let column = index % columns;
            let row = index / columns;
            BuffPlacement {
                index,
                x: viewport_width - margin - width - column as f32 * (width + gap),
                y: margin + row as f32 * (height + gap),
                width,
                height,
                icon_size,
                timer_font_size,
            }
        })
        .collect()
}

fn active_buff_placements(game: &Game) -> Vec<BuffPlacement> {
    buff_placements(
        game.active_buffs.buffs.len(),
        game.canvas.width() as f32,
        game.canvas.client_width() as f32,
    )
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

fn item_definition(
    game: &Game,
    item_id: u32,
) -> Option<&ItemDefinition> {
    game.gui
        .items
        .iter()
        .find(|definition| definition.item_id == item_id)
}

fn buff_info(
    game: &Game,
    buff: &TrackedBuff,
    now_ms: f64,
) -> Option<SkillInfo> {
    let remaining_ms = buff.remaining_ms(now_ms);
    match buff.source {
        Some(active_buff::Source::ItemId(item_id)) => {
            let definition = item_definition(game, item_id)?;
            Some(SkillInfo {
                title: definition.name.clone(),
                level: "Item effect".to_owned(),
                description: nonempty(definition.description.clone()),
                current: buff_modifier_description(buff),
                next: None,
                remaining: Some(format!("Remaining: {}", format_long_duration(remaining_ms))),
            })
        }
        Some(active_buff::Source::SourceSkillId(skill_id)) => {
            skill_definition(game, skill_id).map(|definition| {
                build_skill_info(definition, buff.skill_level, 0, Some(remaining_ms))
            })
        }
        None => skill_definition(game, buff.skill_id).map(|definition| {
            build_skill_info(definition, buff.skill_level, 0, Some(remaining_ms))
        }),
    }
}

fn buff_modifier_description(buff: &ActiveBuff) -> Option<String> {
    let mut modifiers = Vec::new();
    for (label, value) in [
        ("Weapon attack", buff.weapon_attack),
        ("Magic attack", buff.magic_attack),
        ("Weapon defense", buff.weapon_defense),
        ("Magic defense", buff.magic_defense),
        ("Accuracy", buff.accuracy),
        ("Avoidability", buff.avoidability),
        ("Speed", buff.speed_bonus),
        ("Jump", buff.jump_bonus),
    ] {
        if value != 0 {
            modifiers.push(format!("{label} {value:+}"));
        }
    }
    if buff.morph_id > 0 {
        modifiers.push(format!("Morph {}", buff.morph_id));
    }
    (!modifiers.is_empty()).then(|| modifiers.join(", "))
}

fn build_skill_info(
    definition: &SkillDefinition,
    level: u32,
    master_level: u32,
    remaining_ms: Option<u64>,
) -> SkillInfo {
    let maximum_level = if master_level > 0 {
        definition.max_level.min(master_level)
    } else {
        definition.max_level
    };
    let current = (level > 0)
        .then(|| level_description(definition, level))
        .flatten();
    let next = definition
        .levels
        .iter()
        .filter(|candidate| candidate.level > level && candidate.level <= maximum_level)
        .min_by_key(|candidate| candidate.level)
        .and_then(|candidate| {
            nonempty(candidate.description.clone())
                .map(|description| (candidate.level, description))
        })
        .map(|(next_level, description)| format!("Level {next_level}: {description}"));
    SkillInfo {
        title: definition.name.clone(),
        level: format!("Level {level}/{maximum_level}"),
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
    let client_width = game.canvas.client_width() as f32;
    let display_scale = if client_width > 0.0 {
        (game.canvas.width() as f32 / client_width).clamp(1.0, 3.0)
    } else {
        1.0
    };
    game.context.save();
    if game
        .context
        .scale(f64::from(display_scale), f64::from(display_scale))
        .is_err()
    {
        game.context.restore();
        return;
    }
    let pointer = CanvasPoint {
        x: pointer.x / display_scale,
        y: pointer.y / display_scale,
    };
    let content_width = TOOLTIP_WIDTH - TOOLTIP_PADDING * 2.0;
    let lines = tooltip_lines(&game.context, info, content_width);
    let height = TOOLTIP_PADDING * 2.0 + lines.len() as f32 * TOOLTIP_LINE_HEIGHT;
    let (x, y) = tooltip_position(
        pointer,
        TOOLTIP_WIDTH,
        height,
        game.canvas.width() as f32 / display_scale,
        game.canvas.height() as f32 / display_scale,
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
        game.context.set_font(line.font);
        let baseline = y + TOOLTIP_PADDING + (index + 1) as f32 * TOOLTIP_LINE_HEIGHT - 3.0;
        let mut text_x = x + TOOLTIP_PADDING;
        for span in &line.spans {
            game.context.set_fill_style_str(span.color);
            let _ = game
                .context
                .fill_text(&span.text, f64::from(text_x), f64::from(baseline));
            text_x += measured_width(&game.context, &span.text);
        }
    }
    game.context.restore();
}

fn tooltip_lines(
    context: &CanvasRenderingContext2d,
    info: &SkillInfo,
    width: f32,
) -> Vec<TooltipLine> {
    let mut lines = Vec::new();
    push_wz_wrapped(
        &mut lines,
        context,
        &info.title,
        width,
        "#ffffff",
        "#ffffff",
        "bold 12px Arial",
    );
    push_wz_wrapped(
        &mut lines,
        context,
        &info.level,
        width,
        "#aebdca",
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
            push_wz_wrapped(
                &mut lines,
                context,
                text,
                width,
                color,
                "#ffcc66",
                "10px Arial",
            );
        }
    }
    lines
}

fn push_wz_wrapped(
    lines: &mut Vec<TooltipLine>,
    context: &CanvasRenderingContext2d,
    text: &str,
    width: f32,
    base_color: &'static str,
    highlight_color: &'static str,
    font: &'static str,
) {
    context.set_font(font);
    let mut line = StyledLine::default();
    let mut pending_space = false;
    for fragment in parse_wz_text(text) {
        let color = if fragment.highlighted {
            highlight_color
        } else {
            base_color
        };
        push_styled_fragment(
            lines,
            context,
            &mut line,
            &mut pending_space,
            &fragment.text,
            color,
            font,
            width,
        );
    }
    finish_styled_line(lines, &mut line, font);
}

#[derive(Default)]
struct StyledLine {
    spans: Vec<TooltipSpan>,
    width: f32,
}

fn push_styled_fragment(
    lines: &mut Vec<TooltipLine>,
    context: &CanvasRenderingContext2d,
    line: &mut StyledLine,
    pending_space: &mut bool,
    text: &str,
    color: &'static str,
    font: &'static str,
    maximum_width: f32,
) {
    let mut piece = String::new();
    for character in text.chars() {
        if character == '\n' {
            push_styled_piece(
                lines,
                context,
                line,
                pending_space,
                &mut piece,
                color,
                font,
                maximum_width,
            );
            finish_styled_line(lines, line, font);
            *pending_space = false;
        } else if character.is_whitespace() {
            push_styled_piece(
                lines,
                context,
                line,
                pending_space,
                &mut piece,
                color,
                font,
                maximum_width,
            );
            *pending_space = true;
        } else {
            piece.push(character);
        }
    }
    push_styled_piece(
        lines,
        context,
        line,
        pending_space,
        &mut piece,
        color,
        font,
        maximum_width,
    );
}

fn push_styled_piece(
    lines: &mut Vec<TooltipLine>,
    context: &CanvasRenderingContext2d,
    line: &mut StyledLine,
    pending_space: &mut bool,
    piece: &mut String,
    color: &'static str,
    font: &'static str,
    maximum_width: f32,
) {
    if piece.is_empty() {
        return;
    }
    let separator = *pending_space && !line.spans.is_empty();
    let separator_width = if separator {
        measured_width(context, " ")
    } else {
        0.0
    };
    let piece_width = measured_width(context, piece);
    if !line.spans.is_empty() && line.width + separator_width + piece_width > maximum_width {
        finish_styled_line(lines, line, font);
    }
    let prefix = if separator && !line.spans.is_empty() {
        " "
    } else {
        ""
    };
    append_span(&mut line.spans, color, &format!("{prefix}{piece}"));
    line.width += measured_width(context, prefix) + piece_width;
    piece.clear();
    *pending_space = false;
}

fn append_span(
    spans: &mut Vec<TooltipSpan>,
    color: &'static str,
    text: &str,
) {
    if let Some(last) = spans.last_mut().filter(|span| span.color == color) {
        last.text.push_str(text);
    } else {
        spans.push(TooltipSpan {
            text: text.to_owned(),
            color,
        });
    }
}

fn finish_styled_line(
    lines: &mut Vec<TooltipLine>,
    line: &mut StyledLine,
    font: &'static str,
) {
    if !line.spans.is_empty() {
        lines.push(TooltipLine {
            spans: std::mem::take(&mut line.spans),
            font,
        });
    }
    line.width = 0.0;
}

fn measured_width(
    context: &CanvasRenderingContext2d,
    text: &str,
) -> f32 {
    context.measure_text(text).map_or_else(
        |_| text.chars().count() as f32 * 6.0,
        |metrics| metrics.width() as f32,
    )
}

fn parse_wz_text(text: &str) -> Vec<WzTextFragment> {
    let mut fragments = Vec::new();
    let mut buffer = String::new();
    let mut highlighted = false;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '#' {
            buffer.push(character);
            continue;
        }
        push_wz_fragment(&mut fragments, &mut buffer, highlighted);
        highlighted = chars.next_if_eq(&'c').is_some();
    }
    push_wz_fragment(&mut fragments, &mut buffer, highlighted);
    fragments
}

fn push_wz_fragment(
    fragments: &mut Vec<WzTextFragment>,
    buffer: &mut String,
    highlighted: bool,
) {
    if buffer.is_empty() {
        return;
    }
    fragments.push(WzTextFragment {
        text: std::mem::take(buffer),
        highlighted,
    });
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

        let info = build_skill_info(&definition, 2, 0, Some(61_000));

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

        let info = build_skill_info(&definition, 0, 0, None);

        assert_eq!(info.current, None);
        assert_eq!(info.next.as_deref(), Some("Next: Level 1: Recover 10 HP"));
    }

    #[test]
    fn tooltip_displays_the_player_mastery_cap() {
        let definition = SkillDefinition {
            name: "Genesis".to_owned(),
            max_level: 30,
            levels: vec![SkillLevelDefinition {
                level: 11,
                description: "More damage".to_owned(),
                ..SkillLevelDefinition::default()
            }],
            ..SkillDefinition::default()
        };

        let info = build_skill_info(&definition, 10, 10, None);

        assert_eq!(info.level, "Level 10/10");
        assert_eq!(info.next, None);
    }

    #[test]
    fn buff_icons_are_placed_from_right_to_left() {
        let placements = buff_placements(2, 800.0, 800.0);

        assert_eq!(placements[0].x, 756.0);
        assert_eq!(placements[1].x, 716.0);
        assert!(contains(placements[0], CanvasPoint { x: 760.0, y: 20.0 }));
    }

    #[test]
    fn narrow_css_canvases_enlarge_wrap_and_hit_test_buff_icons() {
        let placements = buff_placements(12, 1_024.0, 320.0);

        assert!(placements[0].icon_size > BUFF_ICON_SIZE);
        assert!(
            placements
                .iter()
                .any(|placement| placement.y > placements[0].y)
        );
        let first = placements[0];
        assert!(contains(
            first,
            CanvasPoint {
                x: first.x + first.width / 2.0,
                y: first.y + first.height / 2.0,
            }
        ));
    }

    #[test]
    fn tapping_a_selected_buff_again_clears_the_mobile_selection() {
        let key = crate::game::buffs::BuffKey::Item(2_000_000);

        assert_eq!(toggled_buff_selection(None, key), Some(key));
        assert_eq!(toggled_buff_selection(Some(key), key), None);
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

    #[test]
    fn wz_color_markers_become_highlighted_text_fragments() {
        let fragments = parse_wz_text("Enables recovery.\n#cTerms between skills: 10 min.#");

        assert_eq!(
            fragments,
            vec![
                WzTextFragment {
                    text: "Enables recovery.\n".to_owned(),
                    highlighted: false,
                },
                WzTextFragment {
                    text: "Terms between skills: 10 min.".to_owned(),
                    highlighted: true,
                },
            ]
        );
    }

    #[test]
    fn unterminated_wz_color_marker_highlights_to_the_end() {
        let fragments = parse_wz_text("Required Skill: #cLevel 5");

        assert_eq!(fragments[0].text, "Required Skill: ");
        assert!(!fragments[0].highlighted);
        assert_eq!(fragments[1].text, "Level 5");
        assert!(fragments[1].highlighted);
    }

    #[test]
    fn punctuation_after_a_wz_marker_remains_outside_the_highlight() {
        let fragments = parse_wz_text("Use when #cthe energy is charged#.");

        assert_eq!(fragments[1].text, "the energy is charged");
        assert!(fragments[1].highlighted);
        assert_eq!(fragments[2].text, ".");
        assert!(!fragments[2].highlighted);
    }
}
