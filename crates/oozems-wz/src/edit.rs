use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Serialize;
use serde_json::Value;
use tempfile::NamedTempFile;
use wzlib_rs::WzFormat;
use wzlib_rs::wz::binary_writer::WzBinaryWriter;
use wzlib_rs::wz::directory::compute_image_checksum;
use wzlib_rs::wz::header::WzHeader;
use wzlib_rs::wz::image_writer::write_image;
use wzlib_rs::wz::mcv::McvHeader;
use wzlib_rs::wz::properties::WzProperty;
use wzlib_rs::wz::types::WzPngFormat;

use crate::archive::Archive;
use crate::archive::ImageDescriptor;
use crate::archive::Location;
use crate::archive::OpenOptions;
use crate::archive::entry_paths;
use crate::archive::find_property_mut;
use crate::archive::image_bytes;
use crate::archive::image_descriptors;
use crate::archive::open_archive;
use crate::archive::parse_archive;
use crate::archive::parse_image;
use crate::archive::resolve_location;
use crate::inspect::get;
use crate::inspect::property_summary;
use crate::verify::validate_reference_archive;

#[derive(Debug, Serialize)]
pub struct EditReport {
    pub archive: PathBuf,
    pub output: PathBuf,
    pub path: String,
    pub kind: &'static str,
    pub old_value: Value,
    pub new_value: Value,
    pub bytes_written: usize,
    pub unchanged_images: usize,
}

struct EditedArchive {
    bytes: Vec<u8>,
    path: String,
    kind: &'static str,
    old_value: Value,
    new_value: Value,
    unchanged_images: usize,
}

struct EditedImage {
    image_path: String,
    property_path: String,
    properties: Vec<(String, WzProperty)>,
    bytes: Vec<u8>,
    kind: &'static str,
    old_value: Value,
    new_value: Value,
}

pub fn set_value(
    archive_path: &Path,
    output_path: &Path,
    path: &str,
    value: Value,
    options: OpenOptions,
    force: bool,
) -> Result<EditReport> {
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
    let target_image_path = match resolve_location(&archive.file.directory, path)? {
        Location::Image { path, .. } => path,
        Location::Directory { .. } => bail!("cannot set a directory value: {path}"),
    };
    let source_image_shape = validate_reference_archive(
        archive_path,
        &expected_entries,
        Some(&target_image_path),
        validation_options,
    )
    .context("input archive failed independent validation")?
    .context("independent validation did not inspect the target image")?;
    let edited = edit_archive(archive, path, value)?;
    write_atomic(
        &resolved_output,
        &edited.bytes,
        archive_path,
        force,
        &expected_entries,
        &target_image_path,
        &source_image_shape,
        validation_options,
    )?;

    Ok(EditReport {
        archive: archive_path.to_owned(),
        output: output_path.to_owned(),
        path: edited.path,
        kind: edited.kind,
        old_value: edited.old_value,
        new_value: edited.new_value,
        bytes_written: edited.bytes.len(),
        unchanged_images: edited.unchanged_images,
    })
}

fn edit_archive(
    mut archive: Archive,
    path: &str,
    value: Value,
) -> Result<EditedArchive> {
    let edit = edit_image(&archive, path, &value)?;
    let original_images = image_descriptors(&archive.file.directory);
    let output = rebuild_archive(&mut archive, &original_images, &edit)?;
    let bytes = validate_rebuilt_archive(&archive, &original_images, &edit, output)?;

    Ok(EditedArchive {
        bytes,
        path: edit.property_path,
        kind: edit.kind,
        old_value: edit.old_value,
        new_value: edit.new_value,
        unchanged_images: original_images.len().saturating_sub(1),
    })
}

fn edit_image(
    archive: &Archive,
    path: &str,
    value: &Value,
) -> Result<EditedImage> {
    let (image_path, mut parsed_image, property_segments) =
        match resolve_location(&archive.file.directory, path)? {
            Location::Directory { .. } => bail!("cannot set a directory value: {path}"),
            Location::Image {
                entry,
                path,
                property_segments,
            } => (path, parse_image(archive, entry)?, property_segments),
        };
    if property_segments.is_empty() {
        bail!("cannot set an image value; append a property path after {image_path}");
    }

    let property_path = property_segments
        .iter()
        .fold(image_path.clone(), |path, segment| {
            crate::archive::join_path(&path, segment)
        });
    let (name, property) = find_property_mut(&mut parsed_image.properties, &property_segments)?;
    let old_summary = property_summary(name, property, &property_path);
    let old_value = old_summary
        .value
        .context("only scalar and vector properties can be set")?;
    replace_value(property, value)?;
    let new_summary = property_summary(name, property, &property_path);
    let new_value = new_summary
        .value
        .context("edited property did not produce a value")?;
    let bytes = serialize_image(&parsed_image.properties, parsed_image.iv)?;

    Ok(EditedImage {
        image_path,
        property_path,
        properties: parsed_image.properties,
        bytes,
        kind: new_summary.kind,
        old_value,
        new_value,
    })
}

fn rebuild_archive(
    archive: &mut Archive,
    images: &[ImageDescriptor],
    edit: &EditedImage,
) -> Result<Vec<u8>> {
    let target_count = images
        .iter()
        .filter(|image| image.path == edit.image_path)
        .count();
    if target_count != 1 {
        bail!(
            "expected one image at {}, found {target_count}",
            edit.image_path
        );
    }

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
        bail!(
            "archive directory consumed {consumed} of {} image blobs",
            blobs.len()
        );
    }
    archive
        .file
        .save_with_image_data(&blobs)
        .map_err(Into::into)
}

fn validate_rebuilt_archive(
    original: &Archive,
    original_images: &[ImageDescriptor],
    edit: &EditedImage,
    output: Vec<u8>,
) -> Result<Vec<u8>> {
    let validation_options = OpenOptions {
        region: original.region,
        version: Some(original.file.version),
    };
    let validated = parse_archive(output, original.source.clone(), validation_options)
        .context("rebuilt archive failed validation")?;

    validate_edited_property_tree(&validated, edit)?;
    validate_rebuilt_images(original, &validated, original_images, &edit.image_path)?;
    validate_requested_value(&validated, edit)?;

    Ok(validated.data)
}

fn validate_edited_property_tree(
    rebuilt: &Archive,
    edit: &EditedImage,
) -> Result<()> {
    let rebuilt_image = match resolve_location(&rebuilt.file.directory, &edit.image_path)? {
        Location::Image { entry, .. } => parse_image(rebuilt, entry)?,
        Location::Directory { .. } => {
            bail!(
                "rebuilt archive replaced image {} with a directory",
                edit.image_path
            )
        }
    };
    if !property_lists_equal(&rebuilt_image.properties, &edit.properties) {
        bail!(
            "rebuilt archive changed properties outside {}",
            edit.property_path
        );
    }
    Ok(())
}

fn validate_rebuilt_images(
    original: &Archive,
    rebuilt: &Archive,
    original_images: &[ImageDescriptor],
    edited_image_path: &str,
) -> Result<()> {
    let rebuilt_images = image_descriptors(&rebuilt.file.directory);
    if rebuilt_images.len() != original_images.len() {
        bail!("rebuilt archive changed the number of images");
    }
    for (original_image, rebuilt_image) in original_images.iter().zip(&rebuilt_images) {
        if original_image.path != rebuilt_image.path {
            bail!("rebuilt archive changed image path {}", original_image.path);
        }
        let rebuilt_bytes = image_bytes(&rebuilt.data, &rebuilt_image.entry)?;
        if compute_image_checksum(rebuilt_bytes) != rebuilt_image.entry.checksum {
            bail!(
                "rebuilt image {} has an invalid checksum",
                rebuilt_image.path
            );
        }
        if original_image.path != edited_image_path
            && image_bytes(&original.data, &original_image.entry)? != rebuilt_bytes
        {
            bail!(
                "rebuilt archive changed unedited image {}",
                original_image.path
            );
        }
    }
    Ok(())
}

fn validate_requested_value(
    rebuilt: &Archive,
    edit: &EditedImage,
) -> Result<()> {
    let validated_node = get(rebuilt, &edit.property_path)
        .context("rebuilt archive does not contain the edited property")?;
    if validated_node.kind != edit.kind || validated_node.value.as_ref() != Some(&edit.new_value) {
        bail!(
            "rebuilt archive did not preserve the requested value at {}",
            edit.property_path
        );
    }
    Ok(())
}

fn replace_value(
    property: &mut WzProperty,
    value: &Value,
) -> Result<()> {
    match property {
        WzProperty::Null => {
            if !value.is_null() {
                bail!("null properties only accept JSON null");
            }
        }
        WzProperty::Short(current) => {
            *current = integer(value, "short")?
                .try_into()
                .context("value is outside the i16 range for a short property")?;
        }
        WzProperty::Int(current) => {
            *current = integer(value, "int")?
                .try_into()
                .context("value is outside the i32 range for an int property")?;
        }
        WzProperty::Long(current) => *current = integer(value, "long")?,
        WzProperty::Float(current) => {
            let number = finite_number(value, "float")?;
            let converted = number as f32;
            if !converted.is_finite() {
                bail!("value is outside the finite f32 range for a float property");
            }
            *current = converted;
        }
        WzProperty::Double(current) => *current = finite_number(value, "double")?,
        WzProperty::String(current) | WzProperty::Uol(current) => {
            *current = value
                .as_str()
                .context("string and UOL properties require a JSON string")?
                .to_owned();
        }
        WzProperty::Vector { x, y } => {
            let object = value
                .as_object()
                .context("vector properties require a JSON object with x and y integers")?;
            *x = integer(
                object.get("x").context("vector value is missing x")?,
                "vector x",
            )?
            .try_into()
            .context("vector x is outside the i32 range")?;
            *y = integer(
                object.get("y").context("vector value is missing y")?,
                "vector y",
            )?
            .try_into()
            .context("vector y is outside the i32 range")?;
            if object.len() != 2 {
                bail!("vector values accept only x and y fields");
            }
        }
        _ => bail!("only scalar and vector properties can be set"),
    }
    Ok(())
}

fn integer(
    value: &Value,
    kind: &str,
) -> Result<i64> {
    value
        .as_i64()
        .with_context(|| format!("{kind} properties require a signed JSON integer"))
}

fn finite_number(
    value: &Value,
    kind: &str,
) -> Result<f64> {
    let number = value
        .as_f64()
        .with_context(|| format!("{kind} properties require a JSON number"))?;
    if !number.is_finite() {
        bail!("{kind} properties require a finite number");
    }
    Ok(number)
}

fn serialize_image(
    properties: &[(String, WzProperty)],
    iv: [u8; 4],
) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = WzBinaryWriter::new(&mut cursor, iv, WzHeader::dummy(0));
        write_image(&mut writer, properties)?;
    }
    Ok(cursor.into_inner())
}

fn property_lists_equal(
    left: &[(String, WzProperty)],
    right: &[(String, WzProperty)],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((left_name, left), (right_name, right))| {
                left_name == right_name && properties_equal(left, right)
            })
}

fn properties_equal(
    left: &WzProperty,
    right: &WzProperty,
) -> bool {
    // Archive version is validated separately, and video offsets may change during
    // rebuild. Neither form of physical metadata belongs in the semantic view.
    semantic_property(left) == semantic_property(right)
}

#[derive(PartialEq, Eq)]
enum SemanticProperty<'a> {
    Null,
    Short(i16),
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    String(&'a str),
    SubProperty(Vec<(&'a str, SemanticProperty<'a>)>),
    Canvas {
        width: i32,
        height: i32,
        format: WzPngFormat,
        properties: Vec<(&'a str, SemanticProperty<'a>)>,
        png_data: &'a [u8],
    },
    Vector {
        x: i32,
        y: i32,
    },
    Convex(Vec<(&'a str, SemanticProperty<'a>)>),
    Sound {
        duration_ms: i32,
        data: &'a [u8],
        header: &'a [u8],
    },
    Uol(&'a str),
    Lua(&'a [u8]),
    RawData {
        raw_type: u8,
        properties: Vec<(&'a str, SemanticProperty<'a>)>,
        data: &'a [u8],
    },
    Video {
        video_type: u8,
        properties: Vec<(&'a str, SemanticProperty<'a>)>,
        data_length: u32,
        mcv_header: Option<SemanticMcvHeader>,
        video_data: Option<&'a [u8]>,
    },
}

#[derive(PartialEq, Eq)]
struct SemanticMcvHeader {
    header_length: u16,
    fourcc: u32,
    width: u16,
    height: u16,
    frame_count: i32,
    data_flags: u8,
    frame_delay_unit_ns: i64,
    default_delay: i32,
}

fn semantic_property(property: &WzProperty) -> SemanticProperty<'_> {
    match property {
        WzProperty::Null => SemanticProperty::Null,
        WzProperty::Short(value) => SemanticProperty::Short(*value),
        WzProperty::Int(value) => SemanticProperty::Int(*value),
        WzProperty::Long(value) => SemanticProperty::Long(*value),
        WzProperty::Float(value) => SemanticProperty::Float(value.to_bits()),
        WzProperty::Double(value) => SemanticProperty::Double(value.to_bits()),
        WzProperty::String(value) => SemanticProperty::String(value),
        WzProperty::SubProperty { properties } => {
            SemanticProperty::SubProperty(semantic_property_list(properties))
        }
        WzProperty::Canvas {
            width,
            height,
            format,
            properties,
            png_data,
        } => SemanticProperty::Canvas {
            width: *width,
            height: *height,
            format: *format,
            properties: semantic_property_list(properties),
            png_data,
        },
        WzProperty::Vector { x, y } => SemanticProperty::Vector { x: *x, y: *y },
        WzProperty::Convex { points } => SemanticProperty::Convex(semantic_property_list(points)),
        WzProperty::Sound {
            duration_ms,
            data,
            header,
        } => SemanticProperty::Sound {
            duration_ms: *duration_ms,
            data,
            header,
        },
        WzProperty::Uol(value) => SemanticProperty::Uol(value),
        WzProperty::Lua(value) => SemanticProperty::Lua(value),
        WzProperty::RawData {
            raw_type,
            properties,
            data,
        } => SemanticProperty::RawData {
            raw_type: *raw_type,
            properties: semantic_property_list(properties),
            data,
        },
        WzProperty::Video {
            video_type,
            properties,
            data_offset: _,
            data_length,
            mcv_header,
            video_data,
        } => SemanticProperty::Video {
            video_type: *video_type,
            properties: semantic_property_list(properties),
            data_length: *data_length,
            mcv_header: mcv_header.as_ref().map(semantic_mcv_header),
            video_data: video_data.as_deref(),
        },
    }
}

fn semantic_property_list(
    properties: &[(String, WzProperty)]
) -> Vec<(&str, SemanticProperty<'_>)> {
    properties
        .iter()
        .map(|(name, property)| (name.as_str(), semantic_property(property)))
        .collect()
}

fn semantic_mcv_header(header: &McvHeader) -> SemanticMcvHeader {
    SemanticMcvHeader {
        header_length: header.header_length,
        fourcc: header.fourcc,
        width: header.width,
        height: header.height,
        frame_count: header.frame_count,
        data_flags: header.data_flags,
        frame_delay_unit_ns: header.frame_delay_unit_ns,
        default_delay: header.default_delay,
    }
}

fn resolve_output_path(
    input: &Path,
    output: &Path,
) -> Result<PathBuf> {
    let input = input
        .canonicalize()
        .with_context(|| format!("failed to resolve input archive {}", input.display()))?;
    let output_parent = parent_or_current(output);
    let output_name = output.file_name().context("output path must name a file")?;
    let output = output_parent
        .canonicalize()
        .with_context(|| {
            format!(
                "failed to resolve output directory {}",
                output_parent.display()
            )
        })?
        .join(output_name);
    if input == output {
        bail!("output must differ from the input archive; direct in-place edits are not allowed");
    }
    if output.exists() && input == output.canonicalize()? {
        bail!("output refers to the input archive; direct in-place edits are not allowed");
    }
    Ok(output)
}

fn write_atomic(
    output: &Path,
    bytes: &[u8],
    input: &Path,
    force: bool,
    expected_entries: &[String],
    image_path: &str,
    source_image_shape: &[String],
    options: OpenOptions,
) -> Result<()> {
    if output.exists() && !force {
        bail!(
            "output already exists: {} (use --force to replace it)",
            output.display()
        );
    }
    let parent = parent_or_current(output);
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create a temporary file in {}", parent.display()))?;
    temporary
        .write_all(bytes)
        .with_context(|| format!("failed to write output archive {}", output.display()))?;
    temporary.as_file().sync_all()?;
    fs::set_permissions(temporary.path(), fs::metadata(input)?.permissions())?;
    temporary.as_file().sync_all()?;
    let output_image_shape = validate_reference_archive(
        temporary.path(),
        expected_entries,
        Some(image_path),
        options,
    )
    .context("rebuilt archive failed independent validation")?
    .context("independent validation did not inspect the rebuilt image")?;
    if output_image_shape != source_image_shape {
        bail!("rebuilt image {image_path} changed its property structure or types");
    }

    if force {
        temporary.persist(output).map_err(|error| error.error)?;
    } else {
        temporary
            .persist_noclobber(output)
            .map_err(|error| error.error)?;
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::util::node_util;
    use wzlib_rs::WzFile;
    use wzlib_rs::WzFormat;
    use wzlib_rs::wz::directory::WzDirectoryEntry;
    use wzlib_rs::wz::directory::WzImageEntry;
    use wzlib_rs::wz::file::compute_version_hash;
    use wzlib_rs::wz::types::WzDirectoryType;
    use wzlib_rs::wz::types::WzMapleVersion;

    use super::*;

    fn test_archive() -> Archive {
        let mut directory = WzDirectoryEntry::root();
        let mut mobs = WzDirectoryEntry::new(String::from("Mob"), WzDirectoryType::Directory as u8);
        mobs.images.push(test_image(
            "100.img",
            vec![
                (String::from("hp"), WzProperty::Int(100)),
                (String::from("origin"), WzProperty::Vector { x: 1, y: 2 }),
            ],
        ));
        mobs.images.push(test_image(
            "200.img",
            vec![(
                String::from("name"),
                WzProperty::String(String::from("slime")),
            )],
        ));
        directory.subdirectories.push(mobs);

        archive_from_directory(directory)
    }

    fn test_root_archive() -> Archive {
        test_root_archive_with(vec![(String::from("hp"), WzProperty::Int(100))])
    }

    fn test_root_archive_with(properties: Vec<(String, WzProperty)>) -> Archive {
        let mut directory = WzDirectoryEntry::root();
        directory.images.push(test_image("100.img", properties));
        archive_from_directory(directory)
    }

    fn archive_from_directory(directory: WzDirectoryEntry) -> Archive {
        let version = 83;

        let mut file = WzFile {
            header: WzHeader {
                ident: String::from("PKG1"),
                file_size: 0,
                data_start: 60,
                copyright: String::from("Oozems test archive"),
            },
            version,
            version_hash: compute_version_hash(version),
            maple_version: WzMapleVersion::Gms,
            iv: WzMapleVersion::Gms.iv(),
            user_key: None,
            format: WzFormat::Pkg1,
            is_64bit: false,
            directory,
        };
        let bytes = file.save().unwrap();
        parse_archive(
            bytes,
            PathBuf::from("test.wz"),
            OpenOptions {
                region: crate::Region::Gms,
                version: Some(version),
            },
        )
        .unwrap()
    }

    fn test_image(
        name: &str,
        properties: Vec<(String, WzProperty)>,
    ) -> WzImageEntry {
        WzImageEntry {
            name: name.to_owned(),
            size: 0,
            checksum: 0,
            offset: 0,
            properties: Some(properties),
            iv: None,
        }
    }

    #[test]
    fn edits_one_property_and_preserves_other_image_blob() {
        let archive = test_archive();
        let original_images = image_descriptors(&archive.file.directory);
        let untouched = image_bytes(&archive.data, &original_images[1].entry)
            .unwrap()
            .to_vec();

        let edited = edit_archive(archive, "/Mob/100.img/hp", json!(250)).unwrap();
        let output = parse_archive(
            edited.bytes,
            PathBuf::from("edited.wz"),
            OpenOptions {
                region: crate::Region::Gms,
                version: Some(83),
            },
        )
        .unwrap();
        let hp = get(&output, "/Mob/100.img/hp").unwrap();
        assert_eq!(hp.value, Some(json!(250)));
        let output_images = image_descriptors(&output.file.directory);
        assert_eq!(
            image_bytes(&output.data, &output_images[1].entry).unwrap(),
            untouched
        );
        assert_eq!(edited.unchanged_images, 1);
    }

    #[test]
    fn edits_vector_as_one_typed_value() {
        let edited = edit_archive(
            test_archive(),
            "/Mob/100.img/origin",
            json!({ "x": -5, "y": 12 }),
        )
        .unwrap();
        assert_eq!(edited.new_value, json!({ "x": -5, "y": 12 }));
    }

    #[test]
    fn rejects_value_with_the_wrong_type() {
        let error = edit_archive(test_archive(), "/Mob/100.img/hp", json!("250"))
            .err()
            .unwrap();
        assert!(error.to_string().contains("signed JSON integer"));
    }

    #[test]
    fn bare_output_name_uses_current_directory() {
        assert_eq!(parent_or_current(Path::new("edited.wz")), Path::new("."));
    }

    #[test]
    fn set_value_validates_and_installs_new_archive() {
        let source_archive = test_root_archive();
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.wz");
        let output = directory.path().join("output.wz");
        fs::write(&input, source_archive.data).unwrap();
        let options = OpenOptions {
            region: crate::Region::Gms,
            version: Some(83),
        };

        let report = set_value(&input, &output, "/100.img/hp", json!(250), options, false).unwrap();

        assert_eq!(report.old_value, json!(100));
        assert_eq!(report.new_value, json!(250));
        let output_archive = open_archive(&output, options).unwrap();
        assert_eq!(
            get(&output_archive, "/100.img/hp").unwrap().value,
            Some(json!(250))
        );
        assert!(set_value(&input, &output, "/100.img/hp", json!(300), options, false).is_err());
    }

    #[test]
    fn semantic_video_comparison_ignores_physical_offset() {
        let video = |data_offset| WzProperty::Video {
            video_type: 1,
            properties: vec![(String::from("fps"), WzProperty::Int(30))],
            data_offset,
            data_length: 3,
            mcv_header: None,
            video_data: Some(vec![1, 2, 3]),
        };

        assert!(properties_equal(&video(10), &video(20)));
    }

    #[test]
    fn independent_shape_check_rejects_a_dropped_unknown_float() {
        let mut source_archive = test_root_archive_with(vec![
            (String::from("hp"), WzProperty::Int(100)),
            (String::from("weird"), WzProperty::Float(1.5)),
        ]);
        let image = &source_archive.file.directory.images[0];
        let image_start = image.offset as usize;
        let image_end = image_start + image.size as usize;
        let marker = [0x04, 0x80, 0x00, 0x00, 0xc0, 0x3f];
        let marker_offset = source_archive.data[image_start..image_end]
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        source_archive.data[image_start + marker_offset + 1] = 0x01;

        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.wz");
        let output = directory.path().join("output.wz");
        fs::write(&input, source_archive.data).unwrap();
        let error = set_value(
            &input,
            &output,
            "/100.img/hp",
            json!(250),
            OpenOptions {
                region: crate::Region::Gms,
                version: Some(83),
            },
            false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("property structure or types"));
        assert!(!output.exists());
    }

    #[test]
    fn rebuilt_archive_is_compatible_with_server_reader() {
        let edited = edit_archive(test_root_archive(), "/100.img/hp", json!(250)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.wz");
        fs::write(&path, edited.bytes).unwrap();

        let root: WzNodeArc = WzNode::from_wz_file(&path, None).unwrap().into();
        node_util::parse_node(&root).unwrap();
        root.read().unwrap().at_path_parsed("100.img/hp").unwrap();
    }
}
