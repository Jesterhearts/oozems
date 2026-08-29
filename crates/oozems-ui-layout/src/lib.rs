#![forbid(unsafe_code)]

mod builtin;
mod runtime_regions;

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub use builtin::SpriteDimensions;
pub use builtin::builtin_definition;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteSource;
use oozems_proto::v1::GuiSpriteTemplateSource;
use oozems_proto::v1::GuiWindowDefinition;
use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::text_format::FormatOptions;
use thiserror::Error;

const DEFINITION_MESSAGE: &str = "oozems.v1.GuiWindowDefinition";
pub const SUPPORTED_WINDOWS: [&str; 10] = [
    "status-bar",
    "stats",
    "equipment",
    "inventory",
    "skills",
    "key-config",
    "npc-dialog",
    "shop",
    "cash-shop",
    "death-notice",
];

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutFile {
    pub path: PathBuf,
    pub definition: GuiWindowDefinition,
}

#[derive(Debug, Error)]
pub enum LayoutError {
    #[error("failed to inspect GUI layout path {path}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read GUI layout directory {path}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read GUI layout file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse GUI layout file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: prost_reflect::text_format::ParseError,
    },
    #[error("failed to decode parsed GUI layout file {path}")]
    Decode {
        path: PathBuf,
        #[source]
        source: prost::DecodeError,
    },
    #[error("protobuf descriptor {DEFINITION_MESSAGE} is unavailable")]
    MissingDescriptor,
    #[error("invalid GUI layout {name:?}: {message}")]
    Invalid { name: String, message: String },
    #[error("GUI window {name:?} is defined more than once in {directory}")]
    DuplicateWindow { name: String, directory: PathBuf },
    #[error("failed to encode GUI layout {name:?}")]
    Encode {
        name: String,
        #[source]
        source: prost::DecodeError,
    },
    #[error("failed to write GUI layout file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn load_directory(directory: &Path) -> Result<Vec<GuiWindowDefinition>, LayoutError> {
    Ok(load_files(directory)?
        .into_iter()
        .map(|file| file.definition)
        .collect())
}

pub fn load_files(directory: &Path) -> Result<Vec<LayoutFile>, LayoutError> {
    if !directory
        .try_exists()
        .map_err(|source| LayoutError::Inspect {
            path: directory.to_owned(),
            source,
        })?
    {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(directory).map_err(|source| LayoutError::ReadDirectory {
        path: directory.to_owned(),
        source,
    })?;
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| LayoutError::ReadDirectory {
                    path: directory.to_owned(),
                    source,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "textproto")
    });
    paths.sort();

    let mut names = HashSet::new();
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let definition = load_file(&path)?;
        if !names.insert(definition.name.clone()) {
            return Err(LayoutError::DuplicateWindow {
                name: definition.name,
                directory: directory.to_owned(),
            });
        }
        files.push(LayoutFile { path, definition });
    }
    Ok(files)
}

pub fn load_file(path: &Path) -> Result<GuiWindowDefinition, LayoutError> {
    let input = fs::read_to_string(path).map_err(|source| LayoutError::Read {
        path: path.to_owned(),
        source,
    })?;
    let descriptor = definition_descriptor()?;
    let dynamic = DynamicMessage::parse_text_format(descriptor, &input).map_err(|source| {
        LayoutError::Parse {
            path: path.to_owned(),
            source,
        }
    })?;
    let mut definition: GuiWindowDefinition =
        dynamic
            .transcode_to()
            .map_err(|source| LayoutError::Decode {
                path: path.to_owned(),
                source,
            })?;
    let name = definition.name.clone();
    add_missing_runtime_regions(
        &name,
        definition.width,
        definition.height,
        &mut definition.regions,
    );
    validate(&definition)?;
    Ok(definition)
}

pub fn add_missing_runtime_regions(
    name: &str,
    width: f32,
    height: f32,
    regions: &mut Vec<GuiRegion>,
) {
    runtime_regions::add_missing(name, width, height, regions);
}

pub fn save_file(
    path: &Path,
    definition: &GuiWindowDefinition,
) -> Result<(), LayoutError> {
    validate(definition)?;
    let text = format_textproto(definition)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| LayoutError::Write {
        path: parent.to_owned(),
        source,
    })?;
    let temporary = path.with_extension("textproto.tmp");
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        sync_directory(parent)
    })();
    if let Err(source) = result {
        let _ = fs::remove_file(&temporary);
        return Err(LayoutError::Write {
            path: path.to_owned(),
            source,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn format_textproto(definition: &GuiWindowDefinition) -> Result<String, LayoutError> {
    validate(definition)?;
    let descriptor = definition_descriptor()?;
    let mut dynamic = DynamicMessage::new(descriptor);
    dynamic
        .transcode_from(definition)
        .map_err(|source| LayoutError::Encode {
            name: definition.name.clone(),
            source,
        })?;
    let options = FormatOptions::new().pretty(true);
    let mut output = dynamic.to_text_format_with_options(&options);
    output.push('\n');
    Ok(output)
}

pub fn validate(definition: &GuiWindowDefinition) -> Result<(), LayoutError> {
    if !SUPPORTED_WINDOWS.contains(&definition.name.as_str()) {
        return invalid(
            definition,
            format!(
                "name must be one of {}",
                SUPPORTED_WINDOWS
                    .iter()
                    .map(|name| format!("{name:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    if !finite_nonnegative([definition.x, definition.y]) {
        return invalid(
            definition,
            "window coordinates must be finite and nonnegative",
        );
    }
    if definition.name == "status-bar" && (definition.x != 0.0 || definition.y != 0.0) {
        return invalid(definition, "the status-bar window position must be (0, 0)");
    }
    let uses_background_dimensions = definition.width == 0.0 && definition.height == 0.0;
    let uses_explicit_dimensions = [definition.width, definition.height]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0);
    if !uses_background_dimensions && !uses_explicit_dimensions {
        return invalid(
            definition,
            "layout width and height must both be zero or finite and positive",
        );
    }
    let Some(background) = definition.background.as_ref() else {
        return invalid(definition, "a background source is required");
    };
    if background.anchor_right || background.pin_right || background.pin_bottom {
        return invalid(definition, "the background cannot be anchored or pinned");
    }
    if definition.name == "status-bar" && background.x != 0.0 {
        return invalid(
            definition,
            "the status-bar background x coordinate must be zero",
        );
    }
    if definition.name != "status-bar" && (background.x != 0.0 || background.y != 0.0) {
        return invalid(
            definition,
            "the background must stay at the window origin (0, 0)",
        );
    }

    let mut all_sprite_names = HashSet::new();
    let mut sprite_names = HashSet::new();
    validate_sprite(definition, background, "background")?;
    all_sprite_names.insert(background.name.as_str());
    for sprite in &definition.sprites {
        validate_sprite(definition, sprite, "sprite")?;
        if !all_sprite_names.insert(sprite.name.as_str()) {
            return invalid(
                definition,
                format!("sprite name {:?} appears more than once", sprite.name),
            );
        }
        sprite_names.insert(sprite.name.as_str());
    }

    let mut template_names = HashSet::new();
    for template in &definition.sprite_templates {
        validate_template(definition, template)?;
        if !template_names.insert(template.name.as_str()) {
            return invalid(
                definition,
                format!(
                    "sprite template name {:?} appears more than once",
                    template.name
                ),
            );
        }
    }

    let mut region_names = HashSet::new();
    for region in &definition.regions {
        validate_region(definition, region)?;
        if !region_names.insert(region.name.as_str()) {
            return invalid(
                definition,
                format!("region name {:?} appears more than once", region.name),
            );
        }
    }
    validate_contract(definition, &sprite_names, &template_names, &region_names)?;
    validate_template_families(definition)
}

fn definition_descriptor() -> Result<prost_reflect::MessageDescriptor, LayoutError> {
    DescriptorPool::decode(oozems_proto::FILE_DESCRIPTOR_SET)
        .ok()
        .and_then(|pool| pool.get_message_by_name(DEFINITION_MESSAGE))
        .ok_or(LayoutError::MissingDescriptor)
}

fn validate_sprite(
    definition: &GuiWindowDefinition,
    sprite: &GuiSpriteSource,
    kind: &str,
) -> Result<(), LayoutError> {
    if sprite.name.is_empty() {
        return invalid(definition, format!("{kind} name cannot be empty"));
    }
    validate_wz_path(definition, &sprite.wz_path, &sprite.name)?;
    if !finite_nonnegative([sprite.x, sprite.y, sprite.right, sprite.bottom]) {
        return invalid(
            definition,
            format!(
                "{kind} {:?} coordinates must be finite and nonnegative",
                sprite.name
            ),
        );
    }
    if sprite.anchor_right && definition.name != "status-bar" {
        return invalid(
            definition,
            format!(
                "{kind} {:?} can anchor to the viewport right edge only in status-bar",
                sprite.name
            ),
        );
    }
    Ok(())
}

fn validate_template(
    definition: &GuiWindowDefinition,
    template: &GuiSpriteTemplateSource,
) -> Result<(), LayoutError> {
    if template.name.is_empty() {
        return invalid(definition, "sprite template name cannot be empty");
    }
    validate_wz_path(definition, &template.wz_path, &template.name)?;
    if ![template.offset_x, template.offset_y]
        .into_iter()
        .all(f32::is_finite)
    {
        return invalid(
            definition,
            format!("sprite template {:?} offsets must be finite", template.name),
        );
    }
    let has_offset = template.offset_x != 0.0 || template.offset_y != 0.0;
    let supports_offset = definition.name == "skills" && is_skill_point_template(&template.name);
    if has_offset && !supports_offset {
        return invalid(
            definition,
            format!(
                "sprite template {:?} is positioned by runtime geometry and cannot use offsets",
                template.name
            ),
        );
    }
    Ok(())
}

fn validate_region(
    definition: &GuiWindowDefinition,
    region: &GuiRegion,
) -> Result<(), LayoutError> {
    if region.name.is_empty() {
        return invalid(definition, "region name cannot be empty");
    }
    if !finite_nonnegative([region.x, region.y])
        || ![region.width, region.height]
            .into_iter()
            .all(|value| value.is_finite() && value > 0.0)
    {
        return invalid(
            definition,
            format!(
                "region {:?} must have finite nonnegative coordinates and positive dimensions",
                region.name
            ),
        );
    }
    if definition.width > 0.0
        && (region.x + region.width > definition.width
            || region.y + region.height > definition.height)
    {
        return invalid(
            definition,
            format!(
                "region {:?} extends outside the explicit layout",
                region.name
            ),
        );
    }
    Ok(())
}

fn validate_wz_path(
    definition: &GuiWindowDefinition,
    path: &str,
    element_name: &str,
) -> Result<(), LayoutError> {
    let mut components = path.split('/');
    let valid_image = components
        .next()
        .is_some_and(|image| image.ends_with(".img"));
    let remainder = components.collect::<Vec<_>>();
    if !valid_image
        || remainder.is_empty()
        || remainder
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        return invalid(
            definition,
            format!(
                "element {element_name:?} has invalid UI.wz path {path:?}; expected \
                 Image.img/path/to/png"
            ),
        );
    }
    Ok(())
}

fn validate_contract(
    definition: &GuiWindowDefinition,
    sprite_names: &HashSet<&str>,
    template_names: &HashSet<&str>,
    region_names: &HashSet<&str>,
) -> Result<(), LayoutError> {
    let contract = contract_for(&definition.name);
    require_names(definition, "sprite", sprite_names, contract.sprites)?;
    require_names(
        definition,
        "sprite template",
        template_names,
        contract.templates,
    )?;
    require_names(definition, "region", region_names, contract.regions)?;
    let runtime_regions = runtime_regions::defaults(
        &definition.name,
        definition.width,
        definition.height,
        &definition.regions,
    );
    let runtime_region_names = runtime_regions
        .iter()
        .map(|region| region.name.as_str())
        .collect::<Vec<_>>();
    require_names(
        definition,
        "runtime region",
        region_names,
        &runtime_region_names,
    )
}

struct LayoutContract {
    name: &'static str,
    sprites: &'static [&'static str],
    templates: &'static [&'static str],
    regions: &'static [&'static str],
}

const EMPTY_NAMES: &[&str] = &[];

const LAYOUT_CONTRACTS: &[LayoutContract] = &[
    LayoutContract {
        name: "status-bar",
        sprites: &[
            "gauge",
            "cash-shop",
            "equip",
            "inventory",
            "stats",
            "skills",
            "key-settings",
        ],
        templates: EMPTY_NAMES,
        regions: EMPTY_NAMES,
    },
    LayoutContract {
        name: "stats",
        sprites: &["stat-close"],
        templates: &["stat-ability-up", "stat-ability-up-disabled"],
        regions: &[
            "stat-strength-up",
            "stat-dexterity-up",
            "stat-intelligence-up",
            "stat-luck-up",
        ],
    },
    LayoutContract {
        name: "equipment",
        sprites: &["equipment-close"],
        templates: EMPTY_NAMES,
        regions: EMPTY_NAMES,
    },
    LayoutContract {
        name: "inventory",
        sprites: &["inventory-close"],
        templates: &[
            "inventory-locked-slot",
            "inventory-tab-equipment-active-background",
            "inventory-tab-equipment-inactive-background",
            "inventory-tab-equipment-active-label",
            "inventory-tab-equipment-inactive-label",
            "inventory-tab-consume-active-background",
            "inventory-tab-consume-inactive-background",
            "inventory-tab-consume-active-label",
            "inventory-tab-consume-inactive-label",
            "inventory-tab-install-active-background",
            "inventory-tab-install-inactive-background",
            "inventory-tab-install-active-label",
            "inventory-tab-install-inactive-label",
            "inventory-tab-etc-active-background",
            "inventory-tab-etc-inactive-background",
            "inventory-tab-etc-active-label",
            "inventory-tab-etc-inactive-label",
            "inventory-tab-cash-active-background",
            "inventory-tab-cash-inactive-background",
            "inventory-tab-cash-active-label",
            "inventory-tab-cash-inactive-label",
        ],
        regions: &[
            "inventory-tab-equipment",
            "inventory-tab-consume",
            "inventory-tab-install",
            "inventory-tab-etc",
            "inventory-tab-cash",
            "inventory-mesos",
        ],
    },
    LayoutContract {
        name: "skills",
        sprites: &["skill-close"],
        templates: &[
            "skill-row",
            "skill-row-selected",
            "skill-point-up",
            "skill-point-up-hover",
            "skill-point-up-pressed",
            "skill-point-up-disabled",
            "skill-job-tab-0-enabled",
            "skill-job-tab-0-disabled",
            "skill-job-tab-1-enabled",
            "skill-job-tab-1-disabled",
            "skill-job-tab-2-enabled",
            "skill-job-tab-2-disabled",
            "skill-job-tab-3-enabled",
            "skill-job-tab-3-disabled",
            "skill-job-tab-4-enabled",
            "skill-job-tab-4-disabled",
        ],
        regions: &[
            "skill-title",
            "skill-list",
            "skill-points",
            "skill-page-previous",
            "skill-page-label",
            "skill-page-next",
            "skill-job-tab-0",
            "skill-job-tab-1",
            "skill-job-tab-2",
            "skill-job-tab-3",
            "skill-job-tab-4",
        ],
    },
    LayoutContract {
        name: "key-config",
        sprites: &[
            "key-config-close",
            "key-action-53",
            "key-action-50",
            "key-action-2",
            "key-action-0",
            "key-action-1",
            "key-action-9",
            "key-action-3",
            "key-action-52",
        ],
        templates: EMPTY_NAMES,
        regions: EMPTY_NAMES,
    },
    LayoutContract {
        name: "npc-dialog",
        sprites: EMPTY_NAMES,
        templates: &[
            "npc-dialog-close",
            "npc-dialog-ok",
            "npc-dialog-next",
            "npc-dialog-previous",
            "npc-dialog-accept",
            "npc-dialog-decline",
            "npc-dialog-choice-selected",
        ],
        regions: &[
            "npc-portrait",
            "npc-title",
            "npc-text",
            "npc-choices",
            "npc-previous",
            "npc-decision-previous",
            "npc-next",
            "npc-ok",
            "npc-close",
            "npc-accept",
            "npc-decline",
        ],
    },
    LayoutContract {
        name: "shop",
        sprites: EMPTY_NAMES,
        templates: &[
            "shop-selection",
            "shop-meso",
            "shop-buy",
            "shop-sell",
            "shop-exit",
        ],
        regions: &[
            "shop-stock",
            "shop-inventory",
            "shop-inventory-previous",
            "shop-inventory-next",
            "shop-buy",
            "shop-sell",
            "shop-close",
            "shop-mesos",
        ],
    },
    LayoutContract {
        name: "cash-shop",
        sprites: &[
            "cash-shop-item-card-0",
            "cash-shop-item-card-1",
            "cash-shop-item-card-2",
            "cash-shop-item-card-3",
            "cash-shop-item-card-4",
            "cash-shop-item-card-5",
            "cash-shop-item-card-6",
            "cash-shop-item-card-7",
            "cash-shop-item-card-8",
            "cash-shop-item-card-9",
        ],
        templates: &["cash-shop-buy"],
        regions: &[
            "cash-shop-buy-0",
            "cash-shop-buy-1",
            "cash-shop-buy-2",
            "cash-shop-buy-3",
            "cash-shop-buy-4",
            "cash-shop-buy-5",
            "cash-shop-buy-6",
            "cash-shop-buy-7",
            "cash-shop-buy-8",
            "cash-shop-buy-9",
            "cash-shop-exit",
        ],
    },
    LayoutContract {
        name: "death-notice",
        sprites: &["death-notice-ok"],
        templates: EMPTY_NAMES,
        regions: &["death-notice-ok"],
    },
];

fn contract_for(name: &str) -> &'static LayoutContract {
    LAYOUT_CONTRACTS
        .iter()
        .find(|contract| contract.name == name)
        .expect("supported window must have a layout contract")
}

fn require_names(
    definition: &GuiWindowDefinition,
    kind: &str,
    actual: &HashSet<&str>,
    required: &[&str],
) -> Result<(), LayoutError> {
    let missing = required
        .iter()
        .copied()
        .filter(|name| !actual.contains(name))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    invalid(
        definition,
        format!("missing required {kind} names: {}", missing.join(", ")),
    )
}

fn validate_template_families(definition: &GuiWindowDefinition) -> Result<(), LayoutError> {
    if definition.name != "skills" {
        return Ok(());
    }
    let Some(base) = definition
        .sprite_templates
        .iter()
        .find(|template| template.name == "skill-point-up")
    else {
        return Ok(());
    };
    let mismatched = definition.sprite_templates.iter().any(|template| {
        matches!(
            template.name.as_str(),
            "skill-point-up-hover" | "skill-point-up-pressed" | "skill-point-up-disabled"
        ) && (template.offset_x != base.offset_x || template.offset_y != base.offset_y)
    });
    if mismatched {
        return invalid(
            definition,
            "skill point button states must use the same offsets",
        );
    }
    Ok(())
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

fn finite_nonnegative<const N: usize>(values: [f32; N]) -> bool {
    values
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
}

fn invalid<T>(
    definition: &GuiWindowDefinition,
    message: impl Into<String>,
) -> Result<T, LayoutError> {
    Err(LayoutError::Invalid {
        name: definition.name.clone(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSpriteSource;
    use oozems_proto::v1::GuiSpriteTemplateSource;
    use oozems_proto::v1::GuiWindowDefinition;

    use super::LAYOUT_CONTRACTS;
    use super::SUPPORTED_WINDOWS;
    use super::add_missing_runtime_regions;
    use super::contract_for;
    use super::format_textproto;
    use super::load_directory;
    use super::load_file;
    use super::save_file;
    use super::validate;

    #[test]
    fn textproto_round_trip_preserves_layout_values() {
        let definition = test_definition();
        let text = format_textproto(&definition).expect("format definition");
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("skills.textproto");
        fs::write(&path, text).expect("write definition");

        let loaded = load_file(&path).expect("load definition");

        assert_eq!(loaded, definition);
    }

    #[test]
    fn duplicate_region_names_are_rejected() {
        let mut definition = test_definition();
        definition.regions.push(definition.regions[0].clone());

        let error = validate(&definition).expect_err("duplicate region");

        assert!(error.to_string().contains("appears more than once"));
    }

    #[test]
    fn legacy_inventory_layouts_gain_the_mesos_region() {
        let mut definition = contract_definition("inventory");
        definition
            .regions
            .retain(|region| region.name != "inventory-mesos");

        add_missing_runtime_regions(
            "inventory",
            definition.width,
            definition.height,
            &mut definition.regions,
        );

        let mesos = definition
            .regions
            .iter()
            .find(|region| region.name == "inventory-mesos")
            .expect("migrated mesos region");
        assert_eq!(
            (mesos.x, mesos.y, mesos.width, mesos.height),
            (26.0, 274.0, 111.0, 14.0)
        );
        validate(&definition).expect("migrated inventory definition");
    }

    #[test]
    fn every_supported_window_has_a_satisfied_contract() {
        assert_eq!(LAYOUT_CONTRACTS.len(), SUPPORTED_WINDOWS.len());
        for name in SUPPORTED_WINDOWS {
            let definition = contract_definition(name);
            validate(&definition)
                .unwrap_or_else(|error| panic!("invalid {name} contract: {error}"));
        }
    }

    #[test]
    fn every_contract_rejects_a_missing_runtime_name() {
        for contract in LAYOUT_CONTRACTS {
            let mut definition = contract_definition(contract.name);
            let missing = if let Some(name) = contract.sprites.first() {
                definition.sprites.remove(0);
                *name
            } else if let Some(name) = contract.templates.first() {
                definition.sprite_templates.remove(0);
                *name
            } else {
                let name = contract
                    .regions
                    .first()
                    .expect("contract must require a runtime name");
                definition.regions.remove(0);
                *name
            };

            let error = validate(&definition).expect_err("missing runtime name");

            assert!(
                error.to_string().contains(missing),
                "{missing:?} was not reported for {}: {error}",
                contract.name
            );
        }
    }

    #[test]
    fn background_name_does_not_satisfy_a_sprite_contract() {
        let mut definition = contract_definition("equipment");
        definition.sprites.clear();
        definition.background.as_mut().expect("background").name = "equipment-close".to_owned();

        let error = validate(&definition).expect_err("missing close sprite");

        assert!(error.to_string().contains("equipment-close"));
    }

    #[test]
    fn viewport_right_anchoring_is_restricted_to_the_status_bar() {
        let mut equipment = contract_definition("equipment");
        equipment.sprites[0].anchor_right = true;
        let mut status_bar = contract_definition("status-bar");
        status_bar.sprites[0].anchor_right = true;

        let error = validate(&equipment).expect_err("unsupported viewport anchor");

        assert!(error.to_string().contains("only in status-bar"));
        validate(&status_bar).expect("status bar viewport anchor");
    }

    #[test]
    fn skill_point_button_states_require_one_shared_offset() {
        let mut definition = test_definition();
        definition
            .sprite_templates
            .iter_mut()
            .find(|template| template.name == "skill-point-up-disabled")
            .expect("disabled skill point template")
            .offset_x += 1.0;

        let error = validate(&definition).expect_err("mismatched skill point offsets");

        assert!(error.to_string().contains("must use the same offsets"));
    }

    #[test]
    fn only_skill_point_templates_accept_offsets() {
        let mut definition = contract_definition("shop");
        definition.sprite_templates[0].offset_x = 1.0;

        let error = validate(&definition).expect_err("unsupported template offset");

        assert!(error.to_string().contains("cannot use offsets"));
    }

    #[test]
    fn status_bar_keeps_its_viewport_anchored_coordinates() {
        let mut definition = contract_definition("status-bar");
        definition.x = 1.0;
        let error = validate(&definition).expect_err("status window position");
        assert!(error.to_string().contains("position must be (0, 0)"));

        definition.x = 0.0;
        definition.background.as_mut().expect("background").x = 1.0;
        let error = validate(&definition).expect_err("status background position");
        assert!(error.to_string().contains("x coordinate must be zero"));
    }

    #[test]
    fn ordinary_window_backgrounds_stay_at_the_window_origin() {
        let mut definition = contract_definition("equipment");
        definition.width = 100.0;
        definition.height = 100.0;
        let background = definition.background.as_mut().expect("background");
        background.x = 5.0;
        background.y = 7.0;

        let error = validate(&definition).expect_err("detached window background");

        assert!(error.to_string().contains("window origin"));
    }

    #[test]
    fn save_file_replaces_the_definition_without_leaving_a_temporary_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gui/skills.textproto");
        let mut definition = test_definition();

        save_file(&path, &definition).expect("initial save");
        definition.x = 42.0;
        save_file(&path, &definition).expect("replacement save");

        assert_eq!(load_file(&path).expect("saved definition").x, 42.0);
        assert!(!path.with_extension("textproto.tmp").exists());
    }

    #[test]
    fn bundled_layout_definitions_are_valid() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/gui");

        let definitions = load_directory(&directory).expect("bundled GUI layouts");

        assert!(
            definitions
                .iter()
                .any(|definition| definition.name == "skills")
        );
    }

    fn test_definition() -> GuiWindowDefinition {
        let mut definition = GuiWindowDefinition {
            name: "skills".to_owned(),
            x: 20.0,
            y: 80.0,
            background: Some(GuiSpriteSource {
                name: "skill-background".to_owned(),
                wz_path: "UIWindow.img/Skill/backgrnd".to_owned(),
                x: 0.0,
                y: 0.0,
                anchor_right: false,
                pin_right: false,
                right: 0.0,
                pin_bottom: false,
                bottom: 0.0,
            }),
            sprites: vec![GuiSpriteSource {
                name: "skill-close".to_owned(),
                wz_path: "UIWindow.img/BtUIClose/normal/0".to_owned(),
                x: 157.0,
                y: 5.0,
                anchor_right: false,
                pin_right: true,
                right: 5.0,
                pin_bottom: false,
                bottom: 0.0,
            }],
            sprite_templates: Vec::new(),
            regions: Vec::new(),
            width: 0.0,
            height: 0.0,
        };
        definition.sprite_templates.extend(
            [
                "skill-row",
                "skill-row-selected",
                "skill-point-up",
                "skill-point-up-hover",
                "skill-point-up-pressed",
                "skill-point-up-disabled",
            ]
            .into_iter()
            .map(template),
        );
        for index in 0..5 {
            definition
                .sprite_templates
                .push(template(&format!("skill-job-tab-{index}-enabled")));
            definition
                .sprite_templates
                .push(template(&format!("skill-job-tab-{index}-disabled")));
        }
        for template in definition
            .sprite_templates
            .iter_mut()
            .filter(|template| template.name.starts_with("skill-point-up"))
        {
            template.offset_x = -3.0;
            template.offset_y = 4.0;
        }
        definition.regions.extend(
            [
                "skill-title",
                "skill-list",
                "skill-points",
                "skill-page-previous",
                "skill-page-label",
                "skill-page-next",
                "skill-job-tab-0",
                "skill-job-tab-1",
                "skill-job-tab-2",
                "skill-job-tab-3",
                "skill-job-tab-4",
            ]
            .into_iter()
            .map(|name| GuiRegion {
                name: name.to_owned(),
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }),
        );
        add_missing_runtime_regions("skills", 0.0, 0.0, &mut definition.regions);
        definition
    }

    fn contract_definition(name: &str) -> GuiWindowDefinition {
        let contract = contract_for(name);
        let mut definition = GuiWindowDefinition {
            name: name.to_owned(),
            x: if name == "status-bar" { 0.0 } else { 20.0 },
            y: if name == "status-bar" { 0.0 } else { 80.0 },
            background: Some(source(&format!("{name}-background"))),
            sprites: contract.sprites.iter().map(|name| source(name)).collect(),
            sprite_templates: contract
                .templates
                .iter()
                .map(|name| template(name))
                .collect(),
            regions: contract
                .regions
                .iter()
                .map(|name| GuiRegion {
                    name: (*name).to_owned(),
                    x: 1.0,
                    y: 2.0,
                    width: 3.0,
                    height: 4.0,
                })
                .collect(),
            width: 0.0,
            height: 0.0,
        };
        add_missing_runtime_regions(name, 0.0, 0.0, &mut definition.regions);
        definition
    }

    fn source(name: &str) -> GuiSpriteSource {
        GuiSpriteSource {
            name: name.to_owned(),
            wz_path: format!("UIWindow.img/Layout/{name}"),
            ..GuiSpriteSource::default()
        }
    }

    fn template(name: &str) -> GuiSpriteTemplateSource {
        GuiSpriteTemplateSource {
            name: name.to_owned(),
            wz_path: format!("UIWindow.img/Skill/{name}"),
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}
