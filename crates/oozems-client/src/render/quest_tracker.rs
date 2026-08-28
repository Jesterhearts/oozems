use oozems_proto::v1::QuestTrackerEntry;

use crate::game::Game;

const PANEL_MARGIN: f64 = 16.0;
const PANEL_WIDTH: f64 = 250.0;
const PANEL_PADDING: f64 = 10.0;
const HEADER_HEIGHT: f64 = 20.0;
const QUEST_TITLE_HEIGHT: f64 = 19.0;
const OBJECTIVE_HEIGHT: f64 = 16.0;
const SUMMARY_HEIGHT: f64 = 16.0;
const MAX_QUESTS: usize = 4;
const MAX_OBJECTIVES: usize = 4;

pub(super) fn draw(
    game: &Game,
    active_buff_bottom: Option<f64>,
) {
    let entries = crate::quest_tracker::active_entries(
        &game.ui.gui,
        &game.player.state,
        &game.player.active_buffs,
    );
    if entries.is_empty() || game.ui.interaction.is_open() {
        return;
    }

    let viewport_width = f64::from(game.surface.canvas.width());
    let viewport_height = f64::from(game.surface.canvas.height());
    let width = PANEL_WIDTH.min((viewport_width - PANEL_MARGIN * 2.0).max(0.0));
    if width <= PANEL_PADDING * 2.0 {
        return;
    }
    let y = active_buff_bottom.map_or(PANEL_MARGIN, |bottom| bottom + PANEL_MARGIN);
    let fixed_height = PANEL_PADDING * 2.0 + HEADER_HEIGHT;
    let available_body_height = viewport_height - y - PANEL_MARGIN - fixed_height;
    let (visible_entries, show_quest_overflow) = visible_entries(&entries, available_body_height);
    if visible_entries.is_empty() {
        return;
    }
    let body_height = visible_entries
        .iter()
        .map(|(_, detail_rows)| QUEST_TITLE_HEIGHT + *detail_rows as f64 * OBJECTIVE_HEIGHT)
        .sum::<f64>();
    let overflow_height = if show_quest_overflow {
        OBJECTIVE_HEIGHT
    } else {
        0.0
    };
    let height = fixed_height + body_height + overflow_height;
    let x = (viewport_width - width - PANEL_MARGIN).max(PANEL_MARGIN);

    game.surface
        .context
        .set_fill_style_str("rgba(24, 31, 34, 0.86)");
    game.surface.context.fill_rect(x, y, width, height);
    game.surface.context.set_fill_style_str("#83c9a7");
    game.surface.context.fill_rect(x, y, 3.0, height);
    game.surface.context.set_fill_style_str("#edf5ef");
    game.surface.context.set_font("bold 11px Arial");
    let _ = game.surface.context.fill_text(
        "QUEST TRACKER",
        x + PANEL_PADDING,
        y + PANEL_PADDING + 10.0,
    );

    let mut row_y = y + PANEL_PADDING + HEADER_HEIGHT;
    for (entry, detail_rows) in &visible_entries {
        game.surface
            .context
            .set_fill_style_str(if entry.ready { "#8ed6a8" } else { "#f4e8bd" });
        game.surface.context.set_font("bold 12px Arial");
        let _ = game.surface.context.fill_text_with_max_width(
            &entry.title,
            x + PANEL_PADDING,
            row_y + 12.0,
            width - PANEL_PADDING * 2.0,
        );
        row_y += QUEST_TITLE_HEIGHT;

        if entry.objectives.is_empty() {
            if *detail_rows > 0 && !entry.summary.is_empty() {
                draw_detail(game, &entry.summary, false, x, row_y, width);
                row_y += SUMMARY_HEIGHT;
            }
            continue;
        }
        let shown_objectives = if entry.objectives.len() > *detail_rows {
            detail_rows.saturating_sub(1)
        } else {
            *detail_rows
        };
        for objective in entry.objectives.iter().take(shown_objectives) {
            let text = if objective.show_counter {
                format!(
                    "{}  {}/{}",
                    objective.label, objective.current, objective.required
                )
            } else {
                objective.label.clone()
            };
            draw_detail(game, &text, objective.complete, x, row_y, width);
            row_y += OBJECTIVE_HEIGHT;
        }
        if shown_objectives < entry.objectives.len() && *detail_rows > shown_objectives {
            draw_detail(
                game,
                &format!(
                    "+{} more objectives",
                    entry.objectives.len() - shown_objectives
                ),
                false,
                x,
                row_y,
                width,
            );
            row_y += OBJECTIVE_HEIGHT;
        }
    }
    if show_quest_overflow {
        draw_detail(
            game,
            &format!("+{} more quests", entries.len() - visible_entries.len()),
            false,
            x,
            row_y,
            width,
        );
    }
}

fn visible_entries(
    entries: &[QuestTrackerEntry],
    available_body_height: f64,
) -> (Vec<(&QuestTrackerEntry, usize)>, bool) {
    let mut remaining = available_body_height.max(0.0);
    let mut visible = Vec::new();
    for entry in entries.iter().take(MAX_QUESTS) {
        if remaining < QUEST_TITLE_HEIGHT {
            break;
        }
        remaining -= QUEST_TITLE_HEIGHT;
        let desired_detail_rows = if entry.objectives.is_empty() {
            usize::from(!entry.summary.is_empty())
        } else {
            entry.objectives.len().min(MAX_OBJECTIVES)
                + usize::from(entry.objectives.len() > MAX_OBJECTIVES)
        };
        let detail_rows = desired_detail_rows.min((remaining / OBJECTIVE_HEIGHT).floor() as usize);
        remaining -= detail_rows as f64 * OBJECTIVE_HEIGHT;
        visible.push((entry, detail_rows));
        if detail_rows < desired_detail_rows {
            break;
        }
    }
    let show_quest_overflow = visible.len() < entries.len() && remaining >= OBJECTIVE_HEIGHT;
    (visible, show_quest_overflow)
}

fn draw_detail(
    game: &Game,
    text: &str,
    complete: bool,
    x: f64,
    y: f64,
    width: f64,
) {
    game.surface
        .context
        .set_fill_style_str(if complete { "#88ad99" } else { "#d9e1dc" });
    game.surface.context.set_font("11px Arial");
    let marker = if complete { "[x]" } else { "[ ]" };
    let _ = game
        .surface
        .context
        .fill_text(marker, x + PANEL_PADDING, y + 11.0);
    let _ = game.surface.context.fill_text_with_max_width(
        text,
        x + PANEL_PADDING + 25.0,
        y + 11.0,
        width - PANEL_PADDING * 2.0 - 25.0,
    );
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::QuestTrackerEntry;
    use oozems_proto::v1::QuestTrackerObjective;

    use super::HEADER_HEIGHT;
    use super::OBJECTIVE_HEIGHT;
    use super::PANEL_PADDING;
    use super::QUEST_TITLE_HEIGHT;
    use super::visible_entries;

    #[test]
    fn visible_rows_fit_the_available_panel_height() {
        let entries = (0..4)
            .map(|quest_id| QuestTrackerEntry {
                quest_id,
                objectives: vec![QuestTrackerObjective::default(); 6],
                ..QuestTrackerEntry::default()
            })
            .collect::<Vec<_>>();
        let available_body_height = QUEST_TITLE_HEIGHT + OBJECTIVE_HEIGHT * 2.0;

        let (visible, show_overflow) = visible_entries(&entries, available_body_height);
        let used_height = PANEL_PADDING * 2.0
            + HEADER_HEIGHT
            + visible
                .iter()
                .map(|(_, rows)| QUEST_TITLE_HEIGHT + *rows as f64 * OBJECTIVE_HEIGHT)
                .sum::<f64>()
            + if show_overflow { OBJECTIVE_HEIGHT } else { 0.0 };

        assert!(used_height <= PANEL_PADDING * 2.0 + HEADER_HEIGHT + available_body_height);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].1, 2);
    }
}
