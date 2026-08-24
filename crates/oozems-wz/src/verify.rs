use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use wz_reader::WzNode;
use wz_reader::WzNodeArc;
use wz_reader::WzObjectType;
use wz_reader::property::WzSubProperty;
use wz_reader::property::WzValue;
use wz_reader::util::node_util;

use crate::OpenOptions;
use crate::Region;
use crate::archive::join_path;

pub(crate) fn validate_reference_archive(
    path: &Path,
    expected_entries: &[String],
    image_path: Option<&str>,
    options: OpenOptions,
) -> Result<Option<Vec<String>>> {
    path.to_str()
        .context("wz_reader requires UTF-8 archive paths")?;
    let root: WzNodeArc = WzNode::from_wz_file_full(
        path,
        Some(reader_region(options.region)),
        options.version.map(i32::from),
        None,
        None,
    )
    .with_context(|| format!("wz_reader could not open {}", path.display()))?
    .into();
    node_util::parse_node(&root)
        .with_context(|| format!("wz_reader could not parse {}", path.display()))?;

    let mut actual_entries = Vec::new();
    collect_entries(&root, "/", &mut actual_entries)?;
    actual_entries.sort();
    let mut expected_entries = expected_entries.to_vec();
    expected_entries.sort();
    if actual_entries != expected_entries {
        bail!(
            "independent archive validation found a different entry tree: wzlib-rs={}, \
             wz_reader={}",
            expected_entries.len(),
            actual_entries.len()
        );
    }

    let image_shape = if let Some(image_path) = image_path {
        let image = root
            .read()
            .map_err(|_| anyhow::anyhow!("wz_reader root lock was poisoned"))?
            .at_path_parsed(image_path.trim_start_matches('/'))
            .with_context(|| format!("wz_reader could not resolve image {image_path}"))?;
        node_util::parse_node(&image)
            .with_context(|| format!("wz_reader could not parse image {image_path}"))?;
        let mut shape = Vec::new();
        collect_property_shape(&image, image_path, &mut shape)?;
        shape.sort();
        Some(shape)
    } else {
        None
    };
    Ok(image_shape)
}

fn reader_region(region: Region) -> wz_reader::util::version::WzMapleVersion {
    match region {
        Region::Gms => wz_reader::util::version::WzMapleVersion::GMS,
        Region::Ems => wz_reader::util::version::WzMapleVersion::EMS,
        Region::Bms => wz_reader::util::version::WzMapleVersion::BMS,
    }
}

fn collect_entries(
    node: &WzNodeArc,
    path: &str,
    entries: &mut Vec<String>,
) -> Result<()> {
    let children: Vec<WzNodeArc> = node
        .read()
        .map_err(|_| anyhow::anyhow!("wz_reader node lock was poisoned at {path}"))?
        .children
        .values()
        .cloned()
        .collect();

    for child in children {
        let (name, kind) = {
            let child = child
                .read()
                .map_err(|_| anyhow::anyhow!("wz_reader child lock was poisoned at {path}"))?;
            let kind = match child.object_type {
                WzObjectType::Directory(_) => EntryKind::Directory,
                WzObjectType::Image(_) => EntryKind::Image,
                _ => bail!("wz_reader found a non-directory entry below {path}"),
            };
            (child.name.to_string(), kind)
        };
        let child_path = join_path(path, &name);
        match kind {
            EntryKind::Directory => {
                entries.push(format!("directory:{child_path}"));
                node_util::parse_node(&child)
                    .with_context(|| format!("wz_reader could not parse directory {child_path}"))?;
                collect_entries(&child, &child_path, entries)?;
            }
            EntryKind::Image => entries.push(format!("image:{child_path}")),
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    Image,
}

fn collect_property_shape(
    node: &WzNodeArc,
    path: &str,
    shape: &mut Vec<String>,
) -> Result<()> {
    let children: Vec<WzNodeArc> = node
        .read()
        .map_err(|_| anyhow::anyhow!("wz_reader node lock was poisoned at {path}"))?
        .children
        .values()
        .cloned()
        .collect();
    for child in children {
        let (name, kind) = {
            let child = child
                .read()
                .map_err(|_| anyhow::anyhow!("wz_reader property lock was poisoned at {path}"))?;
            (child.name.to_string(), property_kind(&child.object_type)?)
        };
        let child_path = join_path(path, &name);
        shape.push(format!("{kind}:{child_path}"));
        collect_property_shape(&child, &child_path, shape)?;
    }
    Ok(())
}

fn property_kind(property: &WzObjectType) -> Result<&'static str> {
    let kind = match property {
        WzObjectType::Property(WzSubProperty::Convex) => "convex",
        WzObjectType::Property(WzSubProperty::Sound(_)) => "sound",
        WzObjectType::Property(WzSubProperty::PNG(_)) => "canvas",
        WzObjectType::Property(WzSubProperty::Property) => "property",
        WzObjectType::Value(WzValue::RawData(_)) => "raw_data",
        WzObjectType::Value(WzValue::Video(_)) => "video",
        WzObjectType::Value(WzValue::Lua(_)) => "lua",
        WzObjectType::Value(WzValue::Short(_)) => "short",
        WzObjectType::Value(WzValue::Int(_)) => "int",
        WzObjectType::Value(WzValue::Long(_)) => "long",
        WzObjectType::Value(WzValue::Float(_)) => "float",
        WzObjectType::Value(WzValue::Double(_)) => "double",
        WzObjectType::Value(WzValue::Vector(_)) => "vector",
        WzObjectType::Value(WzValue::UOL(_)) => "uol",
        WzObjectType::Value(WzValue::String(_) | WzValue::ParsedString(_)) => "string",
        WzObjectType::Value(WzValue::Null) => "null",
        _ => bail!("wz_reader found a non-property node inside an image"),
    };
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use wzlib_rs::WzFile;
    use wzlib_rs::WzFormat;
    use wzlib_rs::WzHeader;
    use wzlib_rs::WzMapleVersion;
    use wzlib_rs::wz::directory::WzDirectoryEntry;
    use wzlib_rs::wz::directory::WzImageEntry;
    use wzlib_rs::wz::file::compute_version_hash;
    use wzlib_rs::wz::properties::WzProperty;
    use wzlib_rs::wz::types::WzDirectoryType;

    use super::*;

    #[test]
    fn independent_validation_rejects_a_truncated_nested_tree() {
        let version = 83;
        let mut root = WzDirectoryEntry::root();
        let mut child =
            WzDirectoryEntry::new(String::from("Nested"), WzDirectoryType::Directory as u8);
        child.images.push(WzImageEntry {
            name: String::from("Test.img"),
            size: 0,
            checksum: 0,
            offset: 0,
            properties: Some(vec![(String::from("value"), WzProperty::Int(1))]),
            iv: None,
        });
        root.subdirectories.push(child);
        let mut archive = WzFile {
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
            directory: root,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.wz");
        fs::write(&path, archive.save().unwrap()).unwrap();

        let error = validate_reference_archive(
            &path,
            &[
                String::from("directory:/Nested"),
                String::from("image:/Nested/Test.img"),
            ],
            None,
            OpenOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("different entry tree"));
    }
}
