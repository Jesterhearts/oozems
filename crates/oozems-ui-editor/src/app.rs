use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use eframe::egui::Align2;
use eframe::egui::Color32;
use eframe::egui::FontId;
use eframe::egui::Rect;
use eframe::egui::Sense;
use eframe::egui::Stroke;
use eframe::egui::StrokeKind;
use eframe::egui::TextureHandle;
use eframe::egui::TextureOptions;
use eframe::egui::Vec2;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiWindowDefinition;
use thiserror::Error;

use crate::wz::ArchiveError;
use crate::wz::PreviewImage;
use crate::wz::UiArchive;

mod preview;

const REGION_STROKE: Color32 = Color32::from_rgb(100, 214, 190);
const SELECTED_COLOR: Color32 = Color32::from_rgb(235, 190, 82);
const CANVAS_MARGIN: f32 = 32.0;
const RESIZE_HANDLE_SIZE: f32 = 7.0;

pub struct PreparedEditor {
    archive: UiArchive,
    documents: Vec<PreparedDocument>,
    layout_directory: PathBuf,
    wz_path: PathBuf,
}

struct PreparedDocument {
    path: PathBuf,
    definition: GuiWindowDefinition,
    images: HashMap<String, PreviewImage>,
    persisted: bool,
}

pub struct EditorApp {
    archive: UiArchive,
    documents: Vec<LayoutDocument>,
    active: usize,
    layout_directory: PathBuf,
    pixel_snap: bool,
    selection: Option<Selection>,
    show_grid: bool,
    show_regions: bool,
    status: String,
    wz_path: PathBuf,
    zoom: f32,
}

struct LayoutDocument {
    path: PathBuf,
    definition: GuiWindowDefinition,
    textures: HashMap<String, PreviewTexture>,
    dirty: bool,
    persisted: bool,
}

struct PreviewTexture {
    handle: TextureHandle,
    size: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selection {
    Background,
    Sprite(usize),
    Template(usize),
    Region(usize),
}

#[derive(Debug, Error)]
pub enum EditorError {
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Layout(#[from] oozems_ui_layout::LayoutError),
    #[error("no built-in source recipe exists for GUI window {name:?}")]
    MissingBuiltin { name: String },
}

impl PreparedEditor {
    pub fn load(
        wz_path: PathBuf,
        layout_directory: PathBuf,
    ) -> Result<Self, EditorError> {
        let archive = UiArchive::open(&wz_path)?;
        let mut files = oozems_ui_layout::load_files(&layout_directory)?
            .into_iter()
            .map(|file| (file.definition.name.clone(), file))
            .collect::<HashMap<_, _>>();
        let mut documents = Vec::with_capacity(oozems_ui_layout::SUPPORTED_WINDOWS.len());
        for name in oozems_ui_layout::SUPPORTED_WINDOWS {
            if let Some(file) = files.remove(name) {
                documents.push(prepare_document(
                    &archive,
                    file.path,
                    file.definition,
                    true,
                )?);
            } else {
                let definition = prepare_builtin_definition(&archive, name)?;
                documents.push(prepare_document(
                    &archive,
                    layout_directory.join(format!("{name}.textproto")),
                    definition,
                    false,
                )?);
            }
        }
        Ok(Self {
            archive,
            documents,
            layout_directory,
            wz_path,
        })
    }
}

impl EditorApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        prepared: PreparedEditor,
    ) -> Self {
        configure_style(&context.egui_ctx);
        let documents = prepared
            .documents
            .into_iter()
            .map(|document| finish_document(&context.egui_ctx, document))
            .collect();
        Self {
            archive: prepared.archive,
            documents,
            active: 0,
            layout_directory: prepared.layout_directory,
            pixel_snap: true,
            selection: None,
            show_grid: true,
            show_regions: true,
            status: "Ready".to_owned(),
            wz_path: prepared.wz_path,
            zoom: 1.0,
        }
    }
}

impl eframe::App for EditorApp {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _frame: &mut eframe::Frame,
    ) {
        handle_shortcuts(ui.ctx(), self);
        egui::Panel::top("editor-toolbar").show(ui, |ui| {
            draw_toolbar(ui, self);
        });
        egui::Panel::bottom("editor-status").show(ui, |ui| {
            draw_status(ui, self);
        });
        egui::Panel::left("editor-elements")
            .resizable(true)
            .default_size(220.0)
            .size_range(170.0..=340.0)
            .show(ui, |ui| draw_elements(ui, self));
        egui::Panel::right("editor-inspector")
            .resizable(true)
            .default_size(250.0)
            .size_range(210.0..=360.0)
            .show(ui, |ui| draw_inspector(ui, self));
        egui::CentralPanel::default().show(ui, |ui| draw_canvas(ui, self));
    }
}

fn prepare_document(
    archive: &UiArchive,
    path: PathBuf,
    definition: GuiWindowDefinition,
    persisted: bool,
) -> Result<PreparedDocument, EditorError> {
    let mut images = HashMap::new();
    for source_path in source_paths(&definition) {
        if images.contains_key(source_path) {
            continue;
        }
        images.insert(source_path.to_owned(), archive.load_image(source_path)?);
    }
    Ok(PreparedDocument {
        path,
        definition,
        images,
        persisted,
    })
}

fn prepare_builtin_definition(
    archive: &UiArchive,
    name: &str,
) -> Result<GuiWindowDefinition, EditorError> {
    let definition = oozems_ui_layout::builtin_definition(name, |path| {
        archive
            .image_dimensions(path)
            .map(|(width, height)| oozems_ui_layout::SpriteDimensions {
                width: width as f32,
                height: height as f32,
            })
    })?
    .ok_or_else(|| EditorError::MissingBuiltin {
        name: name.to_owned(),
    })?;
    oozems_ui_layout::validate(&definition)?;
    Ok(definition)
}

fn finish_document(
    context: &egui::Context,
    document: PreparedDocument,
) -> LayoutDocument {
    let textures = document
        .images
        .into_iter()
        .map(|(path, image)| {
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            );
            let texture = context.load_texture(path.clone(), color, TextureOptions::NEAREST);
            (
                path,
                PreviewTexture {
                    handle: texture,
                    size: egui::vec2(image.width as f32, image.height as f32),
                },
            )
        })
        .collect();
    LayoutDocument {
        path: document.path,
        definition: document.definition,
        textures,
        dirty: false,
        persisted: document.persisted,
    }
}

fn source_paths(definition: &GuiWindowDefinition) -> impl Iterator<Item = &str> {
    definition
        .background
        .iter()
        .map(|source| source.wz_path.as_str())
        .chain(
            definition
                .sprites
                .iter()
                .map(|source| source.wz_path.as_str()),
        )
        .chain(
            definition
                .sprite_templates
                .iter()
                .map(|source| source.wz_path.as_str()),
        )
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(25, 37, 35);
    visuals.window_fill = Color32::from_rgb(30, 45, 42);
    visuals.extreme_bg_color = Color32::from_rgb(15, 24, 23);
    visuals.selection.bg_fill = Color32::from_rgb(111, 132, 54);
    visuals.selection.stroke.color = Color32::from_rgb(228, 240, 166);
    context.set_visuals(visuals);
}

fn draw_toolbar(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    ui.horizontal(|ui| {
        ui.strong("OOZEMS UI EDITOR");
        ui.separator();
        egui::ComboBox::from_id_salt("layout-selector")
            .selected_text(active_document(app).definition.name.as_str())
            .show_ui(ui, |ui| {
                for (index, document) in app.documents.iter().enumerate() {
                    let label = if document.dirty {
                        format!("{} *", document.definition.name)
                    } else if !document.persisted {
                        format!("{} (new)", document.definition.name)
                    } else {
                        document.definition.name.clone()
                    };
                    if ui.selectable_value(&mut app.active, index, label).clicked() {
                        app.selection = None;
                    }
                }
            });
        if ui.button("Save").clicked() {
            save_active(app);
        }
        if ui.button("Reload").clicked() {
            reload_active(ui.ctx(), app);
        }
        ui.separator();
        ui.checkbox(&mut app.show_regions, "Regions");
        ui.checkbox(&mut app.show_grid, "Grid");
        ui.checkbox(&mut app.pixel_snap, "Pixel snap");
        ui.add(
            egui::Slider::new(&mut app.zoom, 0.5..=3.0)
                .logarithmic(true)
                .text("Zoom"),
        );
    });
}

fn draw_status(
    ui: &mut egui::Ui,
    app: &EditorApp,
) {
    ui.horizontal(|ui| {
        let color = if app.status.starts_with("Error:") {
            Color32::from_rgb(255, 145, 125)
        } else {
            Color32::from_rgb(164, 188, 177)
        };
        ui.colored_label(color, &app.status);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(format!(
                "{} | {}",
                app.layout_directory.display(),
                app.wz_path.display()
            ));
        });
    });
}

fn draw_elements(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    ui.heading("Elements");
    ui.label("Select an element, then drag it on the canvas.");
    ui.separator();
    let document = active_document(app);
    let current = app.selection;
    let mut selected = current;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong("Artwork");
        if ui
            .selectable_label(current == Some(Selection::Background), "Background")
            .clicked()
        {
            selected = Some(Selection::Background);
        }
        for (index, sprite) in document.definition.sprites.iter().enumerate() {
            if ui
                .selectable_label(
                    current == Some(Selection::Sprite(index)),
                    format!("Sprite  {}", sprite.name),
                )
                .clicked()
            {
                selected = Some(Selection::Sprite(index));
            }
        }
        ui.add_space(8.0);
        ui.strong("Dynamic artwork");
        for (index, template) in document.definition.sprite_templates.iter().enumerate() {
            if ui
                .selectable_label(
                    current == Some(Selection::Template(index)),
                    format!("Template  {}", template.name),
                )
                .clicked()
            {
                selected = Some(Selection::Template(index));
            }
        }
        ui.add_space(8.0);
        ui.strong("Regions");
        for (index, region) in document.definition.regions.iter().enumerate() {
            if ui
                .selectable_label(
                    current == Some(Selection::Region(index)),
                    format!("Region  {}", region.name),
                )
                .clicked()
            {
                selected = Some(Selection::Region(index));
            }
        }
    });
    app.selection = selected;
}

fn draw_inspector(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    ui.heading("Inspector");
    let selection = app.selection;
    let document = active_document_mut(app);
    let mut changed = false;
    let mut template_offset_change = None;
    egui::Grid::new("window-inspector")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Window");
            ui.strong(&document.definition.name);
            ui.end_row();
            if document.definition.name == "status-bar" {
                ui.label("Position");
                ui.label("Viewport anchored");
                ui.end_row();
            } else {
                changed |= coordinate_control(ui, "X", &mut document.definition.x);
                changed |= coordinate_control(ui, "Y", &mut document.definition.y);
            }
            changed |= coordinate_control(ui, "Canvas width", &mut document.definition.width);
            changed |= coordinate_control(ui, "Canvas height", &mut document.definition.height);
        });
    ui.separator();
    match selection {
        Some(Selection::Background) => {
            if let Some(background) = document.definition.background.as_mut() {
                ui.strong(&background.name);
                ui.monospace(&background.wz_path);
                if let Some(texture) = document.textures.get(&background.wz_path) {
                    ui.label(format!("{} x {} px", texture.size.x, texture.size.y));
                }
                egui::Grid::new("background-inspector")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        if document.definition.name == "status-bar" {
                            changed |= coordinate_control(ui, "Y", &mut background.y);
                        } else {
                            ui.label("Position");
                            ui.label("Window origin");
                            ui.end_row();
                        }
                    });
            }
        }
        Some(Selection::Sprite(index)) => {
            let supports_anchor_right = document.definition.name == "status-bar";
            if let Some(sprite) = document.definition.sprites.get_mut(index) {
                ui.strong(&sprite.name);
                ui.monospace(&sprite.wz_path);
                egui::Grid::new("sprite-inspector")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        changed |= coordinate_control(ui, "X", &mut sprite.x);
                        changed |= coordinate_control(ui, "Y", &mut sprite.y);
                        ui.label("Pin right");
                        changed |= ui.checkbox(&mut sprite.pin_right, "").changed();
                        ui.end_row();
                        if sprite.pin_right {
                            changed |= coordinate_control(ui, "Right", &mut sprite.right);
                        }
                        ui.label("Pin bottom");
                        changed |= ui.checkbox(&mut sprite.pin_bottom, "").changed();
                        ui.end_row();
                        if sprite.pin_bottom {
                            changed |= coordinate_control(ui, "Bottom", &mut sprite.bottom);
                        }
                        if supports_anchor_right {
                            ui.label("Anchor right");
                            changed |= ui.checkbox(&mut sprite.anchor_right, "").changed();
                            ui.end_row();
                        }
                    });
            }
        }
        Some(Selection::Template(index)) => {
            if let Some(template) = document.definition.sprite_templates.get_mut(index) {
                ui.strong(&template.name);
                ui.monospace(&template.wz_path);
                if is_skill_point_template(&template.name) {
                    ui.label("Offsets adjust the arrow's row-relative placement.");
                    let offset_changed = egui::Grid::new("template-inspector")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            let x_changed =
                                signed_coordinate_control(ui, "Offset X", &mut template.offset_x);
                            let y_changed =
                                signed_coordinate_control(ui, "Offset Y", &mut template.offset_y);
                            x_changed || y_changed
                        })
                        .inner;
                    changed |= offset_changed;
                    if offset_changed {
                        template_offset_change =
                            Some((index, template.offset_x, template.offset_y));
                    }
                } else {
                    ui.label("Move the named region that positions this template.");
                }
            }
        }
        Some(Selection::Region(index)) => {
            if let Some(region) = document.definition.regions.get_mut(index) {
                ui.strong(&region.name);
                egui::Grid::new("region-inspector")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        changed |= coordinate_control(ui, "X", &mut region.x);
                        changed |= coordinate_control(ui, "Y", &mut region.y);
                        changed |= size_control(ui, "Width", &mut region.width);
                        changed |= size_control(ui, "Height", &mut region.height);
                    });
            }
        }
        None => {
            ui.label("Select artwork or a region to inspect it.");
        }
    }
    if let Some((index, offset_x, offset_y)) = template_offset_change {
        set_template_offset(document, index, offset_x, offset_y);
    }
    document.dirty |= changed;
}

fn coordinate_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
) -> bool {
    ui.label(label);
    let changed = ui
        .add(
            egui::DragValue::new(value)
                .range(0.0..=10_000.0)
                .speed(1.0)
                .max_decimals(1),
        )
        .changed();
    ui.end_row();
    changed
}

fn size_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
) -> bool {
    ui.label(label);
    let changed = ui
        .add(
            egui::DragValue::new(value)
                .range(1.0..=10_000.0)
                .speed(1.0)
                .max_decimals(1),
        )
        .changed();
    ui.end_row();
    changed
}

fn signed_coordinate_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
) -> bool {
    ui.label(label);
    let changed = ui
        .add(
            egui::DragValue::new(value)
                .range(-10_000.0..=10_000.0)
                .speed(1.0)
                .max_decimals(1),
        )
        .changed();
    ui.end_row();
    changed
}

fn draw_canvas(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    let mut selection = app.selection;
    let zoom = app.zoom;
    let show_grid = app.show_grid;
    let show_regions = app.show_regions;
    let pixel_snap = app.pixel_snap;
    let document = active_document_mut(app);
    let Some(background) = document.definition.background.as_ref() else {
        ui.colored_label(Color32::LIGHT_RED, "This layout has no background.");
        return;
    };
    let Some(background_texture) = document.textures.get(&background.wz_path) else {
        ui.colored_label(Color32::LIGHT_RED, "The background texture is unavailable.");
        return;
    };
    let layout_size = layout_size(document);
    let available = ui.available_size() - Vec2::splat(CANVAS_MARGIN * 2.0);
    let fit = (available.x / layout_size.x)
        .min(available.y / layout_size.y)
        .max(0.1);
    let scale = fit * zoom;
    let canvas_size = layout_size * scale;
    let (canvas_response, painter) = ui.allocate_painter(canvas_size, Sense::hover());
    let canvas_rect = canvas_response.rect;
    painter.rect_filled(canvas_rect, 2.0, Color32::from_rgb(9, 15, 14));
    paint_texture(
        &painter,
        background_texture,
        canvas_rect.min + egui::vec2(background.x, background.y) * scale,
        scale,
        Color32::WHITE,
    );
    if show_grid && scale >= 1.5 {
        paint_grid(&painter, canvas_rect, scale);
    }
    paint_static_sprites(&painter, canvas_rect.min, scale, document);
    preview::paint(&painter, canvas_rect.min, scale, document);
    if show_regions {
        interact_regions(
            ui,
            &painter,
            canvas_rect.min,
            layout_size,
            scale,
            &mut selection,
            pixel_snap,
            document,
        );
    }
    interact_sprites(
        ui,
        canvas_rect.min,
        layout_size,
        scale,
        &mut selection,
        pixel_snap,
        document,
    );
    interact_templates(
        ui,
        canvas_rect.min,
        layout_size,
        scale,
        &mut selection,
        pixel_snap,
        document,
    );
    app.selection = selection;
}

fn paint_texture(
    painter: &egui::Painter,
    texture: &PreviewTexture,
    position: egui::Pos2,
    scale: f32,
    tint: Color32,
) {
    let rect = Rect::from_min_size(position, texture.size * scale);
    painter.image(
        texture.handle.id(),
        rect,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        tint,
    );
}

fn paint_grid(
    painter: &egui::Painter,
    rect: Rect,
    scale: f32,
) {
    let step = 10.0 * scale;
    let mut x = rect.left();
    while x <= rect.right() {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
        );
        x += step;
    }
    let mut y = rect.top();
    while y <= rect.bottom() {
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
        );
        y += step;
    }
}

fn paint_static_sprites(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    document: &LayoutDocument,
) {
    for sprite in &document.definition.sprites {
        let Some(texture) = document.textures.get(&sprite.wz_path) else {
            continue;
        };
        let position = sprite_position(sprite, texture.size, layout_size(document));
        paint_texture(
            painter,
            texture,
            origin + position * scale,
            scale,
            Color32::WHITE,
        );
    }
}

fn interact_regions(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    origin: egui::Pos2,
    layout_size: Vec2,
    scale: f32,
    selection: &mut Option<Selection>,
    pixel_snap: bool,
    document: &mut LayoutDocument,
) {
    for index in 0..document.definition.regions.len() {
        let selected = *selection == Some(Selection::Region(index));
        let region = &document.definition.regions[index];
        let rect = scaled_rect(
            origin,
            scale,
            region.x,
            region.y,
            region.width,
            region.height,
        );
        let response = ui.interact(
            rect,
            ui.id().with(("region", index)),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            *selection = Some(Selection::Region(index));
        }
        let stroke_color = if selected {
            SELECTED_COLOR
        } else {
            REGION_STROKE
        };
        painter.rect_filled(
            rect,
            1.0,
            Color32::from_rgba_unmultiplied(100, 214, 190, 42),
        );
        painter.rect_stroke(
            rect,
            1.0,
            Stroke::new(if selected { 2.0 } else { 1.0 }, stroke_color),
            StrokeKind::Inside,
        );
        if scale >= 1.0 {
            painter.text(
                rect.left_top() + egui::vec2(3.0, 2.0),
                Align2::LEFT_TOP,
                &region.name,
                FontId::monospace(8.0_f32.min(7.0 * scale)),
                stroke_color,
            );
        }
        let handle = Rect::from_center_size(
            rect.right_bottom(),
            Vec2::splat(RESIZE_HANDLE_SIZE.max(4.0 * scale)),
        );
        let resize = ui.interact(
            handle,
            ui.id().with(("region-resize", index)),
            Sense::drag(),
        );
        if resize.dragged() {
            let delta = resize.drag_delta() / scale;
            let region = &mut document.definition.regions[index];
            region.width = constrained_extent(region.width + delta.x, region.x, layout_size.x);
            region.height = constrained_extent(region.height + delta.y, region.y, layout_size.y);
            document.dirty = true;
        }
        if response.dragged() && !resize.dragged() {
            let delta = response.drag_delta() / scale;
            let region = &mut document.definition.regions[index];
            region.x = constrained_offset(region.x + delta.x, region.width, layout_size.x);
            region.y = constrained_offset(region.y + delta.y, region.height, layout_size.y);
            document.dirty = true;
        }
        if pixel_snap && (response.drag_stopped() || resize.drag_stopped()) {
            let region = &mut document.definition.regions[index];
            snap_region(region);
        }
        if selected {
            painter.rect_filled(handle, 1.0, SELECTED_COLOR);
        }
    }
}

fn interact_sprites(
    ui: &mut egui::Ui,
    origin: egui::Pos2,
    layout_size: Vec2,
    scale: f32,
    selection: &mut Option<Selection>,
    pixel_snap: bool,
    document: &mut LayoutDocument,
) {
    for index in 0..document.definition.sprites.len() {
        let sprite = &document.definition.sprites[index];
        let Some(texture) = document.textures.get(&sprite.wz_path) else {
            continue;
        };
        let size = texture.size;
        let position = sprite_position(sprite, size, layout_size);
        let rect = scaled_rect(origin, scale, position.x, position.y, size.x, size.y);
        let response = ui.interact(
            rect,
            ui.id().with(("sprite", index)),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            *selection = Some(Selection::Sprite(index));
        }
        if response.dragged() {
            let delta = response.drag_delta() / scale;
            let sprite = &mut document.definition.sprites[index];
            if sprite.pin_right {
                sprite.right = constrained_offset(sprite.right - delta.x, size.x, layout_size.x);
            } else {
                sprite.x = constrained_offset(sprite.x + delta.x, size.x, layout_size.x);
            }
            if sprite.pin_bottom {
                sprite.bottom = constrained_offset(sprite.bottom - delta.y, size.y, layout_size.y);
            } else {
                sprite.y = constrained_offset(sprite.y + delta.y, size.y, layout_size.y);
            }
            document.dirty = true;
        }
        if pixel_snap && response.drag_stopped() {
            let sprite = &mut document.definition.sprites[index];
            sprite.x = sprite.x.round();
            sprite.y = sprite.y.round();
            sprite.right = sprite.right.round();
            sprite.bottom = sprite.bottom.round();
        }
        if *selection == Some(Selection::Sprite(index)) {
            ui.painter().rect_stroke(
                rect,
                1.0,
                Stroke::new(2.0, SELECTED_COLOR),
                StrokeKind::Outside,
            );
        }
    }
}

fn interact_templates(
    ui: &mut egui::Ui,
    origin: egui::Pos2,
    layout_size: Vec2,
    scale: f32,
    selection: &mut Option<Selection>,
    pixel_snap: bool,
    document: &mut LayoutDocument,
) {
    let Some(instances) = preview::skill_point_instances(document) else {
        return;
    };
    for instance_index in 0..instances.count {
        let position = instances.position(instance_index);
        let rect = scaled_rect(
            origin,
            scale,
            position.x,
            position.y,
            instances.size.x,
            instances.size.y,
        );
        let response = ui.interact(
            rect,
            ui.id()
                .with(("template", instances.template_index, instance_index)),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            *selection = Some(Selection::Template(instances.template_index));
        }
        if response.dragged() {
            let delta = response.drag_delta() / scale;
            let offset = constrained_repeated_template_offset(
                instances,
                instances.offset + delta,
                layout_size,
            );
            set_template_offset(document, instances.template_index, offset.x, offset.y);
            document.dirty = true;
        }
        if pixel_snap && response.drag_stopped() {
            let template = &document.definition.sprite_templates[instances.template_index];
            let offset = constrained_repeated_template_offset(
                instances,
                egui::vec2(template.offset_x.round(), template.offset_y.round()),
                layout_size,
            );
            set_template_offset(document, instances.template_index, offset.x, offset.y);
        }
        if *selection == Some(Selection::Template(instances.template_index)) {
            ui.painter().rect_stroke(
                rect,
                1.0,
                Stroke::new(2.0, SELECTED_COLOR),
                StrokeKind::Outside,
            );
        }
    }
}

fn set_template_offset(
    document: &mut LayoutDocument,
    index: usize,
    offset_x: f32,
    offset_y: f32,
) {
    let synchronize_skill_points = document.definition.name == "skills"
        && document
            .definition
            .sprite_templates
            .get(index)
            .is_some_and(|template| is_skill_point_template(&template.name));
    for (template_index, template) in document.definition.sprite_templates.iter_mut().enumerate() {
        if template_index == index
            || (synchronize_skill_points && is_skill_point_template(&template.name))
        {
            template.offset_x = offset_x;
            template.offset_y = offset_y;
        }
    }
}

fn is_skill_point_template(name: &str) -> bool {
    matches!(
        name,
        "skill-point-up"
            | "skill-point-up-hover"
            | "skill-point-up-pressed"
            | "skill-point-up-disabled"
    )
}

fn constrained_repeated_template_offset(
    instances: preview::RepeatedTemplate,
    offset: Vec2,
    layout_size: Vec2,
) -> Vec2 {
    let last_baseline_y =
        instances.first_baseline.y + instances.step_y * instances.count.saturating_sub(1) as f32;
    let min_x = -instances.first_baseline.x;
    let min_y = -instances.first_baseline.y;
    let max_x = (layout_size.x - instances.size.x - instances.first_baseline.x).max(min_x);
    let max_y = (layout_size.y - instances.size.y - last_baseline_y).max(min_y);
    egui::vec2(offset.x.clamp(min_x, max_x), offset.y.clamp(min_y, max_y))
}

fn scaled_rect(
    origin: egui::Pos2,
    scale: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Rect {
    Rect::from_min_size(
        origin + egui::vec2(x, y) * scale,
        egui::vec2(width, height) * scale,
    )
}

fn snap_region(region: &mut GuiRegion) {
    region.x = region.x.round();
    region.y = region.y.round();
    region.width = region.width.round().max(1.0);
    region.height = region.height.round().max(1.0);
}

fn constrained_offset(
    value: f32,
    element_extent: f32,
    container_extent: f32,
) -> f32 {
    value.clamp(0.0, (container_extent - element_extent).max(0.0))
}

fn constrained_extent(
    value: f32,
    offset: f32,
    container_extent: f32,
) -> f32 {
    value.clamp(1.0, (container_extent - offset).max(1.0))
}

fn layout_size(document: &LayoutDocument) -> Vec2 {
    if document.definition.width > 0.0 && document.definition.height > 0.0 {
        return egui::vec2(document.definition.width, document.definition.height);
    }
    document
        .definition
        .background
        .as_ref()
        .and_then(|background| document.textures.get(&background.wz_path))
        .map_or(Vec2::ZERO, |texture| texture.size)
}

fn sprite_position(
    sprite: &oozems_proto::v1::GuiSpriteSource,
    sprite_size: Vec2,
    layout_size: Vec2,
) -> Vec2 {
    egui::vec2(
        if sprite.pin_right {
            layout_size.x - sprite_size.x - sprite.right
        } else {
            sprite.x
        },
        if sprite.pin_bottom {
            layout_size.y - sprite_size.y - sprite.bottom
        } else {
            sprite.y
        },
    )
}

fn save_active(app: &mut EditorApp) {
    let result = {
        let document = active_document(app);
        validate_document_geometry(document).and_then(|()| {
            oozems_ui_layout::save_file(&document.path, &document.definition)
                .map_err(|error| error.to_string())
        })
    };
    match result {
        Ok(()) => {
            let path = active_document(app).path.clone();
            let document = active_document_mut(app);
            document.dirty = false;
            document.persisted = true;
            app.status = format!("Saved {}", path.display());
        }
        Err(error) => app.status = format!("Error: {error}"),
    }
}

fn validate_document_geometry(document: &LayoutDocument) -> Result<(), String> {
    let background = document
        .definition
        .background
        .as_ref()
        .ok_or("the layout has no background")?;
    let layout_size = document
        .textures
        .get(&background.wz_path)
        .ok_or("the background texture is unavailable")
        .map(|_| layout_size(document))?;
    let background_size = document
        .textures
        .get(&background.wz_path)
        .expect("background texture checked above")
        .size;
    if background.x + background_size.x > layout_size.x
        || background.y + background_size.y > layout_size.y
    {
        return Err(format!(
            "background extends outside the {} x {} layout",
            layout_size.x, layout_size.y
        ));
    }
    for sprite in &document.definition.sprites {
        let size = document
            .textures
            .get(&sprite.wz_path)
            .ok_or_else(|| format!("sprite {:?} texture is unavailable", sprite.name))?
            .size;
        let position = sprite_position(sprite, size, layout_size);
        if position.x < 0.0
            || position.y < 0.0
            || position.x + size.x > layout_size.x
            || position.y + size.y > layout_size.y
        {
            return Err(format!(
                "sprite {:?} extends outside the {} x {} layout",
                sprite.name, layout_size.x, layout_size.y
            ));
        }
    }
    for region in &document.definition.regions {
        if region.x + region.width > layout_size.x || region.y + region.height > layout_size.y {
            return Err(format!(
                "region {:?} extends outside the {} x {} layout",
                region.name, layout_size.x, layout_size.y
            ));
        }
    }
    if let Some(instances) = preview::skill_point_instances(document)
        && instances.count > 0
    {
        let first = instances.position(0);
        let last = instances.position(instances.count - 1);
        if first.x < 0.0
            || first.y < 0.0
            || first.x + instances.size.x > layout_size.x
            || last.y + instances.size.y > layout_size.y
        {
            return Err("skill point arrows extend outside the layout".to_owned());
        }
    }
    Ok(())
}

fn reload_active(
    context: &egui::Context,
    app: &mut EditorApp,
) {
    let path = active_document(app).path.clone();
    let name = active_document(app).definition.name.clone();
    let persisted = path.exists();
    let result = if persisted {
        oozems_ui_layout::load_file(&path).map_err(EditorError::from)
    } else {
        prepare_builtin_definition(&app.archive, &name)
    }
    .and_then(|definition| prepare_document(&app.archive, path.clone(), definition, persisted));
    match result {
        Ok(document) => {
            app.documents[app.active] = finish_document(context, document);
            app.selection = None;
            app.status = format!("Reloaded {}", path.display());
        }
        Err(error) => app.status = format!("Error: {error}"),
    }
}

fn handle_shortcuts(
    context: &egui::Context,
    app: &mut EditorApp,
) {
    if context.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
        save_active(app);
    }
}

fn active_document(app: &EditorApp) -> &LayoutDocument {
    &app.documents[app.active]
}

fn active_document_mut(app: &mut EditorApp) -> &mut LayoutDocument {
    &mut app.documents[app.active]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use eframe::egui;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSpriteTemplateSource;
    use oozems_proto::v1::GuiWindowDefinition;

    use super::LayoutDocument;
    use super::constrained_extent;
    use super::constrained_offset;
    use super::constrained_repeated_template_offset;
    use super::set_template_offset;
    use super::snap_region;

    #[test]
    fn pixel_snap_rounds_region_geometry() {
        let mut region = GuiRegion {
            x: 1.4,
            y: 2.6,
            width: 3.2,
            height: 4.8,
            ..GuiRegion::default()
        };

        snap_region(&mut region);

        assert_eq!((region.x, region.y), (1.0, 3.0));
        assert_eq!((region.width, region.height), (3.0, 5.0));
    }

    #[test]
    fn oversized_element_is_constrained_to_the_container_origin() {
        assert_eq!(constrained_offset(12.0, 120.0, 100.0), 0.0);
    }

    #[test]
    fn region_outside_container_resizes_to_minimum_extent() {
        assert_eq!(constrained_extent(20.0, 120.0, 100.0), 1.0);
    }

    #[test]
    fn moving_a_skill_point_template_moves_every_visual_state() {
        let mut document = LayoutDocument {
            path: PathBuf::new(),
            definition: GuiWindowDefinition {
                name: "skills".to_owned(),
                sprite_templates: [
                    "skill-point-up",
                    "skill-point-up-hover",
                    "skill-point-up-pressed",
                    "skill-point-up-disabled",
                ]
                .map(|name| GuiSpriteTemplateSource {
                    name: name.to_owned(),
                    ..GuiSpriteTemplateSource::default()
                })
                .into(),
                ..GuiWindowDefinition::default()
            },
            textures: HashMap::new(),
            dirty: false,
            persisted: false,
        };

        set_template_offset(&mut document, 0, -6.0, 3.0);

        assert!(
            document
                .definition
                .sprite_templates
                .iter()
                .all(|template| (template.offset_x, template.offset_y) == (-6.0, 3.0))
        );
    }

    #[test]
    fn repeated_template_offset_keeps_every_instance_inside_the_layout() {
        let instances = super::preview::RepeatedTemplate {
            template_index: 0,
            first_baseline: egui::vec2(144.0, 105.0),
            step_y: 35.0,
            count: 4,
            size: egui::vec2(12.0, 12.0),
            offset: egui::Vec2::ZERO,
        };

        let offset = constrained_repeated_template_offset(
            instances,
            egui::vec2(100.0, 200.0),
            egui::vec2(175.0, 289.0),
        );

        assert_eq!(offset, egui::vec2(19.0, 67.0));
    }
}
