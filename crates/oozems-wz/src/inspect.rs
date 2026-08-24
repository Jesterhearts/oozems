use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use wzlib_rs::WzFormat;
use wzlib_rs::wz::directory::WzDirectoryEntry;
use wzlib_rs::wz::directory::WzImageEntry;
use wzlib_rs::wz::properties::WzProperty;

use crate::archive::Archive;
use crate::archive::Location;
use crate::archive::find_property;
use crate::archive::join_path;
use crate::archive::parse_image;
use crate::archive::resolve_location;

#[derive(Debug, Serialize)]
pub struct ArchiveInfo {
    pub archive: PathBuf,
    pub file_type: &'static str,
    pub format: &'static str,
    pub region: crate::Region,
    pub version: i16,
    pub version_hash: u64,
    pub is_64_bit: bool,
    pub bytes: usize,
    pub directories: usize,
    pub images: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeSummary {
    pub path: String,
    pub name: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    pub path: String,
    pub total: usize,
    pub offset: usize,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub nodes: Vec<NodeSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeTree {
    pub name: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeTree>,
}

pub fn archive_info(archive: &Archive) -> ArchiveInfo {
    let (directories, images) = count_entries(&archive.file.directory);
    ArchiveInfo {
        archive: archive.source.clone(),
        file_type: match archive.file_type {
            wzlib_rs::WzFileType::Standard => "standard",
            wzlib_rs::WzFileType::Pkg2 => "pkg2",
            wzlib_rs::WzFileType::HotfixDataWz => "hotfix_data",
            wzlib_rs::WzFileType::ListFile => "list",
        },
        format: match archive.file.format {
            WzFormat::Pkg1 => "pkg1",
            WzFormat::Pkg2(_) => "pkg2",
        },
        region: archive.region,
        version: archive.file.version,
        version_hash: match archive.file.format {
            WzFormat::Pkg1 => u64::from(archive.file.version_hash),
            WzFormat::Pkg2(params) => params.hash_version,
        },
        is_64_bit: archive.file.is_64bit,
        bytes: archive.data.len(),
        directories,
        images,
    }
}

pub fn list(
    archive: &Archive,
    path: &str,
    offset: usize,
    limit: usize,
) -> Result<ListOutput> {
    if limit == 0 {
        bail!("list limit must be greater than zero");
    }
    let (resolved_path, total, nodes) = match resolve_location(&archive.file.directory, path)? {
        Location::Directory { entry, path } => {
            let (total, nodes) = directory_children(entry, &path, offset, limit);
            (path, total, nodes)
        }
        Location::Image {
            entry,
            path,
            property_segments,
        } => {
            let parsed = parse_image(archive, entry)?;
            if property_segments.is_empty() {
                let (total, children) = property_children(&parsed.properties, &path, offset, limit);
                (path, total, children)
            } else {
                let (_, property) = find_property(&parsed.properties, &property_segments)?;
                let property_path = append_segments(&path, &property_segments);
                let (total, children) = property
                    .children()
                    .map(|children| property_children(children, &property_path, offset, limit))
                    .unwrap_or_default();
                (property_path, total, children)
            }
        }
    };
    let count = nodes.len();
    let next_offset = (offset.saturating_add(count) < total).then_some(offset + count);

    Ok(ListOutput {
        path: resolved_path,
        total,
        offset,
        count,
        next_offset,
        nodes,
    })
}

pub fn get(
    archive: &Archive,
    path: &str,
) -> Result<NodeSummary> {
    match resolve_location(&archive.file.directory, path)? {
        Location::Directory { entry, path } => Ok(directory_summary(entry, &path)),
        Location::Image {
            entry,
            path,
            property_segments,
        } => {
            let parsed = parse_image(archive, entry)?;
            if property_segments.is_empty() {
                let mut summary = image_summary(entry, &path);
                summary.child_count = Some(parsed.properties.len());
                return Ok(summary);
            }

            let (name, property) = find_property(&parsed.properties, &property_segments)?;
            let property_path = append_segments(&path, &property_segments);
            Ok(property_summary(name, property, &property_path))
        }
    }
}

pub fn tree(
    archive: &Archive,
    path: &str,
    maximum_nodes: usize,
) -> Result<NodeTree> {
    if maximum_nodes == 0 {
        bail!("tree node limit must be greater than zero");
    }
    let mut remaining = maximum_nodes;
    match resolve_location(&archive.file.directory, path)? {
        Location::Directory { .. } => bail!("tree paths must select an image or property"),
        Location::Image {
            entry,
            property_segments,
            ..
        } => {
            let parsed = parse_image(archive, entry)?;
            if property_segments.is_empty() {
                consume_tree_node(&mut remaining, maximum_nodes)?;
                let mut properties = parsed.properties.iter().collect::<Vec<_>>();
                properties.sort_by(|left, right| left.0.cmp(&right.0));
                let children = properties
                    .into_iter()
                    .map(|(name, property)| {
                        property_tree(name, property, &mut remaining, maximum_nodes)
                    })
                    .collect::<Result<Vec<_>>>()?;
                return Ok(NodeTree {
                    name: entry.name.clone(),
                    kind: "image",
                    value: None,
                    details: Some(json!({
                        "bytes": entry.size,
                        "checksum": entry.checksum,
                        "offset": entry.offset,
                    })),
                    children,
                });
            }
            let (name, property) = find_property(&parsed.properties, &property_segments)?;
            property_tree(name, property, &mut remaining, maximum_nodes)
        }
    }
}

fn count_entries(directory: &WzDirectoryEntry) -> (usize, usize) {
    let mut directories = directory.subdirectories.len();
    let mut images = directory.images.len();
    for child in &directory.subdirectories {
        let (child_directories, child_images) = count_entries(child);
        directories += child_directories;
        images += child_images;
    }
    (directories, images)
}

fn directory_children(
    directory: &WzDirectoryEntry,
    path: &str,
    offset: usize,
    limit: usize,
) -> (usize, Vec<NodeSummary>) {
    let mut children: Vec<_> = directory
        .subdirectories
        .iter()
        .map(DirectoryChild::Directory)
        .chain(directory.images.iter().map(DirectoryChild::Image))
        .collect();
    children.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    let total = children.len();
    let nodes = children
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|child| match child {
            DirectoryChild::Directory(entry) => {
                directory_summary(entry, &join_path(path, &entry.name))
            }
            DirectoryChild::Image(entry) => image_summary(entry, &join_path(path, &entry.name)),
        })
        .collect();
    (total, nodes)
}

enum DirectoryChild<'a> {
    Directory(&'a WzDirectoryEntry),
    Image(&'a WzImageEntry),
}

impl DirectoryChild<'_> {
    fn sort_key(&self) -> (&str, u8) {
        match self {
            Self::Directory(entry) => (&entry.name, 0),
            Self::Image(entry) => (&entry.name, 1),
        }
    }
}

fn directory_summary(
    directory: &WzDirectoryEntry,
    path: &str,
) -> NodeSummary {
    NodeSummary {
        path: path.to_owned(),
        name: if path == "/" {
            String::from("/")
        } else {
            directory.name.clone()
        },
        kind: "directory",
        child_count: Some(directory.subdirectories.len() + directory.images.len()),
        value: None,
        details: None,
    }
}

fn image_summary(
    image: &WzImageEntry,
    path: &str,
) -> NodeSummary {
    NodeSummary {
        path: path.to_owned(),
        name: image.name.clone(),
        kind: "image",
        child_count: None,
        value: None,
        details: Some(json!({
            "bytes": image.size,
            "checksum": image.checksum,
            "offset": image.offset,
        })),
    }
}

fn property_children(
    properties: &[(String, WzProperty)],
    path: &str,
    offset: usize,
    limit: usize,
) -> (usize, Vec<NodeSummary>) {
    let mut properties: Vec<_> = properties.iter().collect();
    properties.sort_by(|left, right| left.0.cmp(&right.0));
    let total = properties.len();
    let nodes = properties
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(name, property)| property_summary(name, property, &join_path(path, name)))
        .collect();
    (total, nodes)
}

pub(crate) fn property_summary(
    name: &str,
    property: &WzProperty,
    path: &str,
) -> NodeSummary {
    let child_count = property.children().map_or(0, <[_]>::len);
    let (kind, value, details) = property_parts(property);

    NodeSummary {
        path: path.to_owned(),
        name: name.to_owned(),
        kind,
        child_count: Some(child_count),
        value,
        details,
    }
}

fn property_parts(property: &WzProperty) -> (&'static str, Option<Value>, Option<Value>) {
    match property {
        WzProperty::Null => ("null", Some(Value::Null), None),
        WzProperty::Short(value) => ("short", Some(json!(value)), None),
        WzProperty::Int(value) => ("int", Some(json!(value)), None),
        WzProperty::Long(value) => ("long", Some(json!(value)), None),
        WzProperty::Float(value) => ("float", Some(json!(value)), None),
        WzProperty::Double(value) => ("double", Some(json!(value)), None),
        WzProperty::String(value) => ("string", Some(json!(value)), None),
        WzProperty::SubProperty { .. } => ("property", None, None),
        WzProperty::Canvas {
            width,
            height,
            format,
            png_data,
            ..
        } => (
            "canvas",
            None,
            Some(json!({
                "width": width,
                "height": height,
                "format": format.format_id(),
                "compressed_bytes": png_data.len(),
            })),
        ),
        WzProperty::Vector { x, y } => ("vector", Some(json!({ "x": x, "y": y })), None),
        WzProperty::Convex { .. } => ("convex", None, None),
        WzProperty::Sound {
            duration_ms,
            data,
            header,
        } => (
            "sound",
            None,
            Some(json!({
                "duration_ms": duration_ms,
                "header_bytes": header.len(),
                "data_bytes": data.len(),
            })),
        ),
        WzProperty::Uol(value) => ("uol", Some(json!(value)), None),
        WzProperty::Lua(data) => ("lua", None, Some(json!({ "bytes": data.len() }))),
        WzProperty::RawData { raw_type, data, .. } => (
            "raw_data",
            None,
            Some(json!({ "raw_type": raw_type, "bytes": data.len() })),
        ),
        WzProperty::Video {
            video_type,
            data_length,
            video_data,
            ..
        } => (
            "video",
            None,
            Some(json!({
                "video_type": video_type,
                "bytes": video_data.as_ref().map_or(*data_length as usize, Vec::len),
            })),
        ),
    }
}

fn property_tree(
    name: &str,
    property: &WzProperty,
    remaining: &mut usize,
    maximum_nodes: usize,
) -> Result<NodeTree> {
    consume_tree_node(remaining, maximum_nodes)?;
    let (kind, value, details) = property_parts(property);
    let mut properties = property
        .children()
        .unwrap_or_default()
        .iter()
        .collect::<Vec<_>>();
    properties.sort_by(|left, right| left.0.cmp(&right.0));
    let children = properties
        .into_iter()
        .map(|(name, property)| property_tree(name, property, remaining, maximum_nodes))
        .collect::<Result<Vec<_>>>()?;
    Ok(NodeTree {
        name: name.to_owned(),
        kind,
        value,
        details,
        children,
    })
}

fn consume_tree_node(
    remaining: &mut usize,
    maximum_nodes: usize,
) -> Result<()> {
    if *remaining == 0 {
        bail!("tree exceeds the {maximum_nodes} node limit");
    }
    *remaining -= 1;
    Ok(())
}

fn append_segments(
    path: &str,
    segments: &[String],
) -> String {
    segments
        .iter()
        .fold(path.to_owned(), |path, segment| join_path(&path, segment))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_pagination_sorts_before_selecting_page() {
        let properties = vec![
            (String::from("c"), WzProperty::Int(3)),
            (String::from("a"), WzProperty::Int(1)),
            (String::from("b"), WzProperty::Int(2)),
        ];

        let (total, page) = property_children(&properties, "/Test.img", 1, 1);

        assert_eq!(total, 3);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].path, "/Test.img/b");
        assert_eq!(page[0].value, Some(json!(2)));
    }

    #[test]
    fn property_tree_is_sorted_and_bounded() {
        let property = WzProperty::SubProperty {
            properties: vec![
                (String::from("z"), WzProperty::Int(2)),
                (String::from("a"), WzProperty::Int(1)),
            ],
        };

        let tree = property_tree("root", &property, &mut 3, 3).expect("bounded tree");

        assert_eq!(tree.name, "root");
        assert_eq!(tree.kind, "property");
        assert_eq!(tree.children[0].name, "a");
        assert_eq!(tree.children[1].name, "z");
        assert!(property_tree("root", &property, &mut 2, 2).is_err());
    }
}
