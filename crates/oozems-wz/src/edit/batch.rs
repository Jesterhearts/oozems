use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde_json::Value;
use wzlib_rs::WzFormat;
use wzlib_rs::wz::properties::WzProperty;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    Short,
    Int,
    Long,
    Float,
    Double,
    String,
    Vector,
    Null,
    Property,
}

impl PropertyKind {
    pub const ALL: [Self; 9] = [
        Self::Int,
        Self::String,
        Self::Float,
        Self::Double,
        Self::Short,
        Self::Long,
        Self::Vector,
        Self::Null,
        Self::Property,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "Short integer",
            Self::Int => "Integer",
            Self::Long => "Long integer",
            Self::Float => "Float",
            Self::Double => "Double",
            Self::String => "String",
            Self::Vector => "Vector",
            Self::Null => "Null",
            Self::Property => "Container",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropertyEdit {
    Set {
        path: String,
        value: Value,
    },
    Add {
        path: String,
        kind: PropertyKind,
        value: Value,
    },
    Remove {
        path: String,
    },
}

impl PropertyEdit {
    pub fn path(&self) -> &str {
        match self {
            Self::Set { path, .. } | Self::Add { path, .. } | Self::Remove { path } => path,
        }
    }
}

#[derive(Debug)]
pub struct PropertyEditReport {
    pub archive: PathBuf,
    pub output: PathBuf,
    pub path: String,
    pub kind: Option<&'static str>,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub bytes_written: usize,
    pub unchanged_images: usize,
}

struct EditedBatchArchive {
    bytes: Vec<u8>,
    edits: Vec<EditedValue>,
    unchanged_images: usize,
}

struct EditedBatchImage {
    image_path: String,
    properties: Vec<(String, WzProperty)>,
    bytes: Vec<u8>,
    edits: Vec<EditedValue>,
}

struct EditedValue {
    path: String,
    kind: Option<&'static str>,
    old_value: Option<Value>,
    new_value: Option<Value>,
    removed: bool,
}

pub fn set_values(
    archive_path: &Path,
    output_path: &Path,
    edits: &[(String, Value)],
    options: OpenOptions,
    force: bool,
) -> Result<Vec<EditReport>> {
    let operations = edits
        .iter()
        .map(|(path, value)| PropertyEdit::Set {
            path: path.clone(),
            value: value.clone(),
        })
        .collect::<Vec<_>>();
    edit_properties(archive_path, output_path, &operations, options, force)?
        .into_iter()
        .map(|report| {
            Ok(EditReport {
                archive: report.archive,
                output: report.output,
                path: report.path,
                kind: report.kind.context("set operation has no property kind")?,
                old_value: report.old_value.context("set operation has no old value")?,
                new_value: report.new_value.context("set operation has no new value")?,
                bytes_written: report.bytes_written,
                unchanged_images: report.unchanged_images,
            })
        })
        .collect()
}

pub fn edit_properties(
    archive_path: &Path,
    output_path: &Path,
    edits: &[PropertyEdit],
    options: OpenOptions,
    force: bool,
) -> Result<Vec<PropertyEditReport>> {
    if edits.is_empty() {
        bail!("at least one WZ property edit is required");
    }
    let resolved_output = resolve_output_path(archive_path, output_path)?;
    let archive = open_archive(archive_path, options)?;
    if !matches!(archive.file.format, WzFormat::Pkg1) {
        bail!("editing PKG2 archives is not supported");
    }
    let validation_options = OpenOptions {
        region: archive.region,
        version: Some(archive.file.version),
    };
    let expected_entries = entry_paths(&archive.file.directory);
    let image_path = batch_target_image(&archive, edits)?;
    let source_shape = validate_reference_archive(
        archive_path,
        &expected_entries,
        Some(&image_path),
        validation_options,
    )
    .context("input archive failed independent validation")?
    .context("independent validation did not inspect the target image")?;
    let expected_shape = edited_property_shape(source_shape, edits)?;
    let edited = edit_batch_archive(archive, edits)?;
    write_atomic(
        &resolved_output,
        &edited.bytes,
        archive_path,
        force,
        &expected_entries,
        &image_path,
        &expected_shape,
        validation_options,
    )?;
    let bytes_written = edited.bytes.len();
    Ok(edited
        .edits
        .into_iter()
        .map(|edit| PropertyEditReport {
            archive: archive_path.to_owned(),
            output: output_path.to_owned(),
            path: edit.path,
            kind: edit.kind,
            old_value: edit.old_value,
            new_value: edit.new_value,
            bytes_written,
            unchanged_images: edited.unchanged_images,
        })
        .collect())
}

fn batch_target_image(
    archive: &Archive,
    edits: &[PropertyEdit],
) -> Result<String> {
    let mut target = None;
    let mut paths = BTreeSet::new();
    for edit in edits {
        if !paths.insert(edit.path()) {
            bail!("WZ property is edited more than once: {}", edit.path());
        }
        let image_path = operation_image_path(archive, edit)?;
        if target
            .as_ref()
            .is_some_and(|target: &String| target != &image_path)
        {
            bail!("batched WZ edits must target one image");
        }
        target = Some(image_path);
    }
    target.context("at least one WZ property edit is required")
}

fn operation_image_path(
    archive: &Archive,
    edit: &PropertyEdit,
) -> Result<String> {
    if let PropertyEdit::Add { path, .. } = edit {
        return image_path_for_property_path(archive, path);
    }
    match resolve_location(&archive.file.directory, edit.path())? {
        Location::Image { path, .. } => Ok(path),
        Location::Directory { .. } => bail!("property edit path does not select an image"),
    }
}

fn image_path_for_property_path(
    archive: &Archive,
    path: &str,
) -> Result<String> {
    let segments = crate::archive::parse_path(path)?;
    let mut candidate = String::new();
    for segment in segments {
        candidate = crate::archive::join_path(&candidate, &segment);
        match resolve_location(&archive.file.directory, &candidate) {
            Ok(Location::Image { path, .. }) => return Ok(path),
            Ok(Location::Directory { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    bail!("property edit path does not select an image: {path}")
}

fn edit_batch_archive(
    mut archive: Archive,
    edits: &[PropertyEdit],
) -> Result<EditedBatchArchive> {
    let edit = edit_batch_image(&archive, edits)?;
    let original_images = image_descriptors(&archive.file.directory);
    let output = rebuild_batch_archive(&mut archive, &original_images, &edit)?;
    let bytes = validate_rebuilt_batch_archive(&archive, &original_images, &edit, output)?;
    Ok(EditedBatchArchive {
        bytes,
        edits: edit.edits,
        unchanged_images: original_images.len().saturating_sub(1),
    })
}

fn edit_batch_image(
    archive: &Archive,
    edits: &[PropertyEdit],
) -> Result<EditedBatchImage> {
    let image_path = batch_target_image(archive, edits)?;
    let entry = match resolve_location(&archive.file.directory, &image_path)? {
        Location::Image { entry, .. } => entry,
        Location::Directory { .. } => unreachable!("target was resolved as an image"),
    };
    let mut parsed = parse_image(archive, entry)?;
    let mut applied = Vec::with_capacity(edits.len());
    for edit in edits {
        applied.push(apply_edit(&mut parsed.properties, &image_path, edit)?);
    }
    let bytes = serialize_image(&parsed.properties, parsed.iv)?;
    Ok(EditedBatchImage {
        image_path,
        properties: parsed.properties,
        bytes,
        edits: applied,
    })
}

fn apply_edit(
    properties: &mut Vec<(String, WzProperty)>,
    image_path: &str,
    edit: &PropertyEdit,
) -> Result<EditedValue> {
    match edit {
        PropertyEdit::Set { path, value } => {
            let segments = property_segments(image_path, path)?;
            let (name, property) = find_property_mut(properties, &segments)?;
            let old_value = property_summary(name, property, path)
                .value
                .context("only scalar and vector properties can be set")?;
            replace_value(property, value)?;
            let new = property_summary(name, property, path);
            Ok(EditedValue {
                path: path.clone(),
                kind: Some(new.kind),
                old_value: Some(old_value),
                new_value: new.value,
                removed: false,
            })
        }
        PropertyEdit::Add { path, kind, value } => {
            let (parent, name) = parent_path(path)?;
            let segments = property_segments(image_path, parent)?;
            let children = property_children_at_mut(properties, &segments)?;
            if children.iter().any(|(existing, _)| existing == name) {
                bail!("property already exists: {path}");
            }
            let property = new_property(*kind, value)?;
            let summary = property_summary(name, &property, path);
            let report = EditedValue {
                path: path.clone(),
                kind: Some(summary.kind),
                old_value: None,
                new_value: summary.value,
                removed: false,
            };
            children.push((name.to_owned(), property));
            Ok(report)
        }
        PropertyEdit::Remove { path } => {
            let segments = property_segments(image_path, path)?;
            let (name, parent) = segments
                .split_last()
                .context("cannot remove an image root")?;
            let children = property_children_at_mut(properties, parent)?;
            let matching = children
                .iter()
                .enumerate()
                .filter_map(|(index, (candidate, _))| (candidate == name).then_some(index))
                .collect::<Vec<_>>();
            let [index] = matching.as_slice() else {
                if matching.is_empty() {
                    bail!("property does not exist: {path}");
                }
                bail!("property name appears more than once: {name}");
            };
            let (removed_name, removed) = children.remove(*index);
            let summary = property_summary(&removed_name, &removed, path);
            Ok(EditedValue {
                path: path.clone(),
                kind: Some(summary.kind),
                old_value: summary.value,
                new_value: None,
                removed: true,
            })
        }
    }
}

fn property_segments(
    image_path: &str,
    path: &str,
) -> Result<Vec<String>> {
    let suffix = path
        .strip_prefix(image_path)
        .with_context(|| format!("property path {path} is outside image {image_path}"))?;
    if suffix.is_empty() {
        return Ok(Vec::new());
    }
    crate::archive::parse_path(suffix)
}

fn parent_path(path: &str) -> Result<(&str, &str)> {
    let (parent, name) = path
        .rsplit_once('/')
        .context("property path must contain a parent and name")?;
    if parent.is_empty() || name.is_empty() || matches!(name, "." | "..") {
        bail!("property path has an invalid final segment: {path}");
    }
    Ok((parent, name))
}

fn property_children_at_mut<'a>(
    properties: &'a mut Vec<(String, WzProperty)>,
    segments: &[String],
) -> Result<&'a mut Vec<(String, WzProperty)>> {
    if segments.is_empty() {
        return Ok(properties);
    }
    let (name, property) = find_property_mut(properties, segments)?;
    crate::archive::property_children_mut(property)
        .with_context(|| format!("property {name} has no children"))
}

fn new_property(
    kind: PropertyKind,
    value: &Value,
) -> Result<WzProperty> {
    let mut property = match kind {
        PropertyKind::Short => WzProperty::Short(0),
        PropertyKind::Int => WzProperty::Int(0),
        PropertyKind::Long => WzProperty::Long(0),
        PropertyKind::Float => WzProperty::Float(0.0),
        PropertyKind::Double => WzProperty::Double(0.0),
        PropertyKind::String => WzProperty::String(String::new()),
        PropertyKind::Vector => WzProperty::Vector { x: 0, y: 0 },
        PropertyKind::Null => WzProperty::Null,
        PropertyKind::Property => {
            return Ok(WzProperty::SubProperty {
                properties: Vec::new(),
            });
        }
    };
    replace_value(&mut property, value)?;
    Ok(property)
}

fn rebuild_batch_archive(
    archive: &mut Archive,
    images: &[ImageDescriptor],
    edit: &EditedBatchImage,
) -> Result<Vec<u8>> {
    let blobs = images
        .iter()
        .map(|image| {
            if image.path == edit.image_path {
                Ok(edit.bytes.as_slice())
            } else {
                image_bytes(&archive.data, &image.entry)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let consumed = archive.file.directory.attach_image_data(&blobs)?;
    if consumed != blobs.len() {
        bail!("archive directory did not consume every image blob");
    }
    archive
        .file
        .save_with_image_data(&blobs)
        .map_err(Into::into)
}

fn validate_rebuilt_batch_archive(
    original: &Archive,
    original_images: &[ImageDescriptor],
    edit: &EditedBatchImage,
    output: Vec<u8>,
) -> Result<Vec<u8>> {
    let options = OpenOptions {
        region: original.region,
        version: Some(original.file.version),
    };
    let validated = parse_archive(output, original.source.clone(), options)
        .context("rebuilt archive failed validation")?;
    let rebuilt = match resolve_location(&validated.file.directory, &edit.image_path)? {
        Location::Image { entry, .. } => parse_image(&validated, entry)?,
        Location::Directory { .. } => bail!("rebuilt archive replaced the edited image"),
    };
    if !property_lists_equal(&rebuilt.properties, &edit.properties) {
        bail!("rebuilt archive changed properties outside the requested edits");
    }
    validate_rebuilt_images(original, &validated, original_images, &edit.image_path)?;
    for requested in &edit.edits {
        if requested.removed {
            if get(&validated, &requested.path).is_ok() {
                bail!("rebuilt archive did not remove {}", requested.path);
            }
            continue;
        }
        let node = get(&validated, &requested.path)
            .context("rebuilt archive does not contain an edited property")?;
        if node.kind != requested.kind.context("edited property has no kind")?
            || node.value != requested.new_value
        {
            bail!("rebuilt archive has the wrong value at {}", requested.path);
        }
    }
    Ok(validated.data)
}

fn edited_property_shape(
    source_shape: Vec<String>,
    edits: &[PropertyEdit],
) -> Result<Vec<String>> {
    let mut shape = source_shape.into_iter().collect::<BTreeSet<_>>();
    for edit in edits {
        match edit {
            PropertyEdit::Set { .. } => {}
            PropertyEdit::Add { path, kind, .. } => {
                if shape.iter().any(|entry| shape_path(entry) == path) {
                    bail!("property already exists: {path}");
                }
                shape.insert(format!("{}:{path}", property_kind_name(*kind)));
            }
            PropertyEdit::Remove { path } => {
                let child_prefix = format!("{path}/");
                let old_len = shape.len();
                shape.retain(|entry| {
                    let candidate = shape_path(entry);
                    candidate != path && !candidate.starts_with(&child_prefix)
                });
                if shape.len() == old_len {
                    bail!("property does not exist: {path}");
                }
            }
        }
    }
    for edit in edits {
        let PropertyEdit::Set { path, .. } = edit else {
            continue;
        };
        if !shape.iter().any(|entry| shape_path(entry) == path) {
            bail!("cannot set {path} because another edit removes it");
        }
    }
    Ok(shape.into_iter().collect())
}

fn shape_path(shape_entry: &str) -> &str {
    shape_entry
        .split_once(':')
        .map_or(shape_entry, |(_, path)| path)
}

fn property_kind_name(kind: PropertyKind) -> &'static str {
    match kind {
        PropertyKind::Short => "short",
        PropertyKind::Int => "int",
        PropertyKind::Long => "long",
        PropertyKind::Float => "float",
        PropertyKind::Double => "double",
        PropertyKind::String => "string",
        PropertyKind::Vector => "vector",
        PropertyKind::Null => "null",
        PropertyKind::Property => "property",
    }
}
