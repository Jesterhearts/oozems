use eframe::egui;
use eframe::egui::Align2;
use eframe::egui::Color32;
use eframe::egui::FontId;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteTemplateSource;
use oozems_proto::v1::GuiWindowDefinition;

use super::LayoutDocument;
use super::PreviewTexture;
use super::paint_texture;

#[derive(Clone, Copy)]
pub(super) struct RepeatedTemplate {
    pub template_index: usize,
    pub first_baseline: egui::Vec2,
    pub step_y: f32,
    pub count: usize,
    pub size: egui::Vec2,
    pub offset: egui::Vec2,
}

impl RepeatedTemplate {
    pub fn position(
        self,
        index: usize,
    ) -> egui::Vec2 {
        self.first_baseline + egui::vec2(0.0, self.step_y * index as f32) + self.offset
    }
}

pub(super) fn paint(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    match document.definition.name.as_str() {
        "status-bar" => paint_status_bar(painter, origin, scale, document),
        "stats" => paint_stats(painter, origin, scale, document),
        "equipment" => paint_equipment(painter, origin, scale, document),
        "inventory" => paint_inventory(painter, origin, scale, document),
        "skills" => paint_skills(painter, origin, scale, document),
        "key-config" => paint_key_config(painter, origin, scale, document),
        "npc-dialog" => paint_npc_dialog(painter, origin, scale, document),
        "shop" => paint_shop(painter, origin, scale, document),
        "cash-shop" => paint_cash_shop(painter, origin, scale, document),
        "death-notice" => paint_death_notice(painter, origin, scale, document),
        _ => {}
    }
}

fn paint_skills(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for index in 0..5 {
        let Some(region) = named_region(&document.definition, &format!("skill-job-tab-{index}"))
        else {
            continue;
        };
        let state = if index == 0 { "enabled" } else { "disabled" };
        let name = format!("skill-job-tab-{index}-{state}");
        paint_template_centered(painter, origin, scale, document, &name, region);
    }
    if let (Some(list), Some((row_source, row))) = (
        named_region(&document.definition, "skill-list"),
        named_template(document, "skill-row"),
    ) {
        let rows = (list.height / row.size.y).floor() as usize;
        for index in 0..rows {
            let position = origin + egui::vec2(list.x, list.y + index as f32 * row.size.y) * scale;
            paint_texture(
                painter,
                row,
                position + egui::vec2(row_source.offset_x, row_source.offset_y) * scale,
                scale,
                Color32::WHITE,
            );
            paint_repeated_placeholder(
                painter,
                origin,
                scale,
                document,
                "skill-row-icon",
                index,
                row.size.y,
            );
            paint_repeated_text(
                painter,
                origin,
                scale,
                document,
                "skill-row-name",
                index,
                row.size.y,
                [
                    "Three Snails",
                    "Recovery",
                    "Nimble Feet",
                    "Legendary Spirit",
                ][index % 4],
                10.0,
            );
            paint_repeated_text(
                painter,
                origin,
                scale,
                document,
                "skill-row-level",
                index,
                row.size.y,
                &format!("Level {}/3", index.min(3)),
                9.0,
            );
        }
    }
    if let Some(instances) = skill_point_instances(document) {
        let template = &document.definition.sprite_templates[instances.template_index];
        let texture = &document.textures[&template.wz_path];
        for index in 0..instances.count {
            paint_texture(
                painter,
                texture,
                origin + instances.position(index) * scale,
                scale,
                Color32::WHITE,
            );
        }
    }
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "skill-title",
        "Beginner",
        Align2::LEFT_CENTER,
    );
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "skill-points",
        "3",
        Align2::CENTER_CENTER,
    );
    for (name, text) in [
        ("skill-page-previous", "<"),
        ("skill-page-label", "1/2"),
        ("skill-page-next", ">"),
    ] {
        paint_region_text(
            painter,
            origin,
            scale,
            document,
            name,
            text,
            Align2::CENTER_CENTER,
        );
    }
}

fn paint_status_bar(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for (name, text) in [
        ("status-level", "200"),
        ("status-job", "Beginner"),
        ("status-name", "Oozems"),
        ("status-map-name", "Henesys"),
    ] {
        paint_region_text(
            painter,
            origin,
            scale,
            document,
            name,
            text,
            Align2::LEFT_CENTER,
        );
    }
    for (name, fraction, color) in [
        ("status-hp-gauge", 0.72, Color32::from_rgb(207, 63, 74)),
        ("status-mp-gauge", 0.54, Color32::from_rgb(58, 120, 211)),
        ("status-exp-gauge", 0.38, Color32::from_rgb(245, 201, 67)),
    ] {
        paint_region_fill(painter, origin, scale, document, name, fraction, color);
    }
}

fn paint_stats(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for (name, text) in [
        ("stat-character-name", "Oozems"),
        ("stat-level", "200"),
        ("stat-guild", "-"),
        ("stat-hp", "12,345 / 12,345"),
        ("stat-mp", "6,789 / 6,789"),
        ("stat-experience", "1,234,567 (54.32%)"),
        ("stat-fame", "123"),
        ("stat-ability-points", "5"),
        ("stat-strength", "999"),
        ("stat-dexterity", "999"),
        ("stat-intelligence", "999"),
        ("stat-luck", "999"),
    ] {
        paint_region_text(
            painter,
            origin,
            scale,
            document,
            name,
            text,
            Align2::LEFT_CENTER,
        );
    }
}

fn paint_equipment(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for name in [
        "equipment-slot-top",
        "equipment-slot-bottom",
        "equipment-slot-shoes",
        "equipment-slot-weapon",
    ] {
        paint_placeholder(painter, origin, scale, document, name);
    }
}

fn paint_inventory(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "inventory-mesos",
        "1,234,567",
        Align2::RIGHT_CENTER,
    );
    let Some(slots) = named_region(&document.definition, "inventory-slots") else {
        return;
    };
    let step_x = (slots.width - 32.0) / 3.0;
    let step_y = (slots.height - 32.0) / 5.0;
    for index in 0..8 {
        let x = slots.x + (index % 4) as f32 * step_x;
        let y = slots.y + (index / 4) as f32 * step_y;
        paint_placeholder_rect(painter, origin, scale, x, y, 32.0, 32.0);
    }
}

fn paint_key_config(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for code in [
        "AltLeft",
        "KeyZ",
        "KeyC",
        "KeyE",
        "KeyI",
        "KeyK",
        "KeyS",
        "ControlLeft",
    ] {
        paint_placeholder(
            painter,
            origin,
            scale,
            document,
            &format!("key-config-slot-{code}"),
        );
    }
}

fn paint_npc_dialog(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    paint_placeholder(painter, origin, scale, document, "npc-portrait");
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "npc-title",
        "Maple Administrator",
        Align2::LEFT_CENTER,
    );
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "npc-text-with-choices",
        "Choose the destination you would like to visit.",
        Align2::LEFT_CENTER,
    );
    for index in 0..4 {
        if let Some(marker) =
            named_region(&document.definition, &format!("npc-choice-marker-{index}"))
        {
            paint_template_at_region(
                painter,
                origin,
                scale,
                document,
                "npc-dialog-choice-selected",
                marker,
            );
        }
        paint_region_text(
            painter,
            origin,
            scale,
            document,
            &format!("npc-choice-label-{index}"),
            ["Henesys", "Ellinia", "Perion", "Kerning City"][index],
            Align2::LEFT_CENTER,
        );
    }
}

fn paint_shop(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    paint_placeholder(painter, origin, scale, document, "shop-portrait");
    for prefix in ["stock", "inventory"] {
        for index in 0..5 {
            paint_placeholder(
                painter,
                origin,
                scale,
                document,
                &format!("shop-{prefix}-item-icon-{index}"),
            );
            paint_region_text(
                painter,
                origin,
                scale,
                document,
                &format!("shop-{prefix}-item-name-{index}"),
                "Sample Item",
                Align2::LEFT_CENTER,
            );
            paint_region_text(
                painter,
                origin,
                scale,
                document,
                &format!("shop-{prefix}-item-detail-{index}"),
                "1,000 mesos",
                Align2::LEFT_CENTER,
            );
        }
    }
    for (template, region) in [
        ("shop-buy", "shop-buy"),
        ("shop-sell", "shop-sell"),
        ("shop-exit", "shop-close"),
        ("shop-meso", "shop-mesos-icon"),
    ] {
        if let Some(region) = named_region(&document.definition, region) {
            paint_template_at_region(painter, origin, scale, document, template, region);
        }
    }
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "shop-mesos-text",
        "1,234,567",
        Align2::LEFT_CENTER,
    );
}

fn paint_cash_shop(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    paint_placeholder(painter, origin, scale, document, "cash-shop-character");
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "cash-shop-player-name",
        "Oozems",
        Align2::LEFT_CENTER,
    );
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "cash-shop-balance",
        "12,345 Ooze",
        Align2::LEFT_CENTER,
    );
    for index in 0..10 {
        paint_placeholder(
            painter,
            origin,
            scale,
            document,
            &format!("cash-shop-item-icon-{index}"),
        );
        for (suffix, text) in [
            ("name", "Sample Item"),
            ("duration", "30 days"),
            ("price", "1,200 Ooze"),
        ] {
            paint_region_text(
                painter,
                origin,
                scale,
                document,
                &format!("cash-shop-item-{suffix}-{index}"),
                text,
                Align2::LEFT_CENTER,
            );
        }
        for (template, prefix) in [
            ("cash-shop-buy", "cash-shop-buy"),
            ("cash-shop-gift-disabled", "cash-shop-gift"),
        ] {
            if let Some(region) = named_region(&document.definition, &format!("{prefix}-{index}")) {
                paint_template_at_region(painter, origin, scale, document, template, region);
            }
        }
    }
    paint_region_fill(
        painter,
        origin,
        scale,
        document,
        "cash-shop-request-panel",
        1.0,
        Color32::from_rgba_unmultiplied(245, 248, 250, 220),
    );
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "cash-shop-request-text",
        "Processing...",
        Align2::LEFT_CENTER,
    );
}

fn paint_death_notice(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "death-notice-title",
        "You have died.",
        Align2::CENTER_CENTER,
    );
    paint_region_text(
        painter,
        origin,
        scale,
        document,
        "death-notice-detail",
        "You will be revived in the nearest town.",
        Align2::CENTER_CENTER,
    );
}

fn paint_repeated_placeholder(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    index: usize,
    step_y: f32,
) {
    let Some(region) = named_region(&document.definition, name) else {
        return;
    };
    paint_placeholder_rect(
        painter,
        origin,
        scale,
        region.x,
        region.y + index as f32 * step_y,
        region.width,
        region.height,
    );
}

fn paint_repeated_text(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    index: usize,
    step_y: f32,
    text: &str,
    font_size: f32,
) {
    let Some(region) = named_region(&document.definition, name) else {
        return;
    };
    painter.text(
        origin
            + egui::vec2(
                region.x,
                region.y + index as f32 * step_y + region.height / 2.0,
            ) * scale,
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(font_size * scale),
        Color32::from_rgb(48, 56, 59),
    );
}

fn paint_placeholder(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
) {
    let Some(region) = named_region(&document.definition, name) else {
        return;
    };
    paint_placeholder_rect(
        painter,
        origin,
        scale,
        region.x,
        region.y,
        region.width,
        region.height,
    );
}

fn paint_placeholder_rect(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    let rect = egui::Rect::from_min_size(
        origin + egui::vec2(x, y) * scale,
        egui::vec2(width, height) * scale,
    );
    painter.rect_filled(
        rect,
        2.0,
        Color32::from_rgba_unmultiplied(64, 101, 113, 150),
    );
    painter.line_segment(
        [rect.left_top(), rect.right_bottom()],
        egui::Stroke::new(1.0, Color32::from_rgb(205, 230, 235)),
    );
    painter.line_segment(
        [rect.right_top(), rect.left_bottom()],
        egui::Stroke::new(1.0, Color32::from_rgb(205, 230, 235)),
    );
}

fn paint_region_fill(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    fraction: f32,
    color: Color32,
) {
    let Some(region) = named_region(&document.definition, name) else {
        return;
    };
    painter.rect_filled(
        egui::Rect::from_min_size(
            origin + egui::vec2(region.x, region.y) * scale,
            egui::vec2(region.width * fraction.clamp(0.0, 1.0), region.height) * scale,
        ),
        1.0,
        color,
    );
}

fn paint_template_at_region(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    region: &GuiRegion,
) {
    let Some((source, texture)) = named_template(document, name) else {
        return;
    };
    paint_texture(
        painter,
        texture,
        origin + egui::vec2(region.x + source.offset_x, region.y + source.offset_y) * scale,
        scale,
        Color32::WHITE,
    );
}

pub(super) fn skill_point_instances(document: &LayoutDocument) -> Option<RepeatedTemplate> {
    if document.definition.name != "skills" {
        return None;
    }
    let list = named_region(&document.definition, "skill-list")?;
    let region = named_region(&document.definition, "skill-row-point-button")?;
    let (_, row) = named_template(document, "skill-row")?;
    let template_index = document
        .definition
        .sprite_templates
        .iter()
        .position(|template| template.name == "skill-point-up")?;
    let source = &document.definition.sprite_templates[template_index];
    let button = document.textures.get(&source.wz_path)?;
    let count = (list.height / row.size.y).floor() as usize;
    Some(RepeatedTemplate {
        template_index,
        first_baseline: egui::vec2(
            region.x + (region.width - button.size.x) / 2.0,
            region.y + (region.height - button.size.y) / 2.0,
        ),
        step_y: row.size.y,
        count,
        size: button.size,
        offset: egui::vec2(source.offset_x, source.offset_y),
    })
}

fn paint_template_centered(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    region: &GuiRegion,
) {
    let Some((source, texture)) = named_template(document, name) else {
        return;
    };
    let x = region.x + (region.width - texture.size.x) / 2.0 + source.offset_x;
    let y = region.y + (region.height - texture.size.y) / 2.0 + source.offset_y;
    paint_texture(
        painter,
        texture,
        origin + egui::vec2(x, y) * scale,
        scale,
        Color32::WHITE,
    );
}

fn paint_region_text(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
    name: &str,
    text: &str,
    align: Align2,
) {
    let Some(region) = named_region(&document.definition, name) else {
        return;
    };
    let x = if align == Align2::LEFT_CENTER {
        region.x
    } else if align == Align2::RIGHT_CENTER {
        region.x + region.width
    } else {
        region.x + region.width / 2.0
    };
    painter.text(
        origin + egui::vec2(x, region.y + region.height / 2.0) * scale,
        align,
        text,
        FontId::monospace(9.0 * scale),
        Color32::from_rgb(45, 55, 58),
    );
}

fn named_region<'a>(
    definition: &'a GuiWindowDefinition,
    name: &str,
) -> Option<&'a GuiRegion> {
    definition.regions.iter().find(|region| region.name == name)
}

fn named_template<'a>(
    document: &'a LayoutDocument,
    name: &str,
) -> Option<(&'a GuiSpriteTemplateSource, &'a PreviewTexture)> {
    let source = document
        .definition
        .sprite_templates
        .iter()
        .find(|template| template.name == name)?;
    let texture = document.textures.get(&source.wz_path)?;
    Some((source, texture))
}
