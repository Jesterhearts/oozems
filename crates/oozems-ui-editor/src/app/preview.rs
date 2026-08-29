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
    if document.definition.name == "inventory" {
        paint_inventory_mesos(painter, origin, scale, document);
        return;
    }
    if document.definition.name != "skills" {
        return;
    }
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
            painter.text(
                position + egui::vec2(38.0, 7.0) * scale,
                Align2::LEFT_TOP,
                [
                    "Three Snails",
                    "Recovery",
                    "Nimble Feet",
                    "Legendary Spirit",
                ][index % 4],
                FontId::monospace(9.0 * scale),
                Color32::from_rgb(45, 55, 58),
            );
            painter.text(
                position + egui::vec2(38.0, 21.0) * scale,
                Align2::LEFT_TOP,
                format!("Level {}/3", index.min(3)),
                FontId::monospace(8.0 * scale),
                Color32::from_rgb(80, 92, 94),
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

fn paint_inventory_mesos(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    let Some(region) = named_region(&document.definition, "inventory-mesos") else {
        return;
    };
    painter.text(
        origin + egui::vec2(region.x + region.width, region.y + region.height / 2.0) * scale,
        Align2::RIGHT_CENTER,
        "1,234,567",
        FontId::proportional(10.0 * scale),
        Color32::from_rgb(48, 56, 59),
    );
}

pub(super) fn skill_point_instances(document: &LayoutDocument) -> Option<RepeatedTemplate> {
    if document.definition.name != "skills" {
        return None;
    }
    let list = named_region(&document.definition, "skill-list")?;
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
            list.x + row.size.x - button.size.x - 2.0,
            list.y + (row.size.y - button.size.y) / 2.0,
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
