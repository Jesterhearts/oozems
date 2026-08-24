use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Serialize;
use wzlib_rs::WzFile;
use wzlib_rs::WzFileType;
use wzlib_rs::WzHeader;
use wzlib_rs::WzMapleVersion;
use wzlib_rs::detect_file_type;
use wzlib_rs::parse_wz_image;
use wzlib_rs::wz::binary_reader::WzBinaryReader;
use wzlib_rs::wz::directory::WzDirectoryEntry;
use wzlib_rs::wz::directory::WzImageEntry;
use wzlib_rs::wz::properties::WzProperty;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    #[default]
    Gms,
    Ems,
    Bms,
}

impl Region {
    fn maple_version(self) -> WzMapleVersion {
        match self {
            Region::Gms => WzMapleVersion::Gms,
            Region::Ems => WzMapleVersion::Ems,
            Region::Bms => WzMapleVersion::Bms,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions {
    pub region: Region,
    pub version: Option<i16>,
}

pub struct Archive {
    pub(crate) source: PathBuf,
    pub(crate) data: Vec<u8>,
    pub(crate) file: WzFile,
    pub(crate) region: Region,
    pub(crate) file_type: WzFileType,
}

pub fn open_archive(
    path: &Path,
    options: OpenOptions,
) -> Result<Archive> {
    let data =
        fs::read(path).with_context(|| format!("failed to read WZ archive {}", path.display()))?;
    parse_archive(data, path.to_owned(), options)
}

pub(crate) fn parse_archive(
    data: Vec<u8>,
    source: PathBuf,
    options: OpenOptions,
) -> Result<Archive> {
    let file_type = detect_file_type(&data);
    if !matches!(file_type, WzFileType::Standard | WzFileType::Pkg2) {
        bail!(
            "{} is a {:?} file; only standard PKG1 and PKG2 archives are supported",
            source.display(),
            file_type
        );
    }

    let region = options.region;
    let file = WzFile::parse(&data, region.maple_version(), options.version)
        .with_context(|| format!("failed to parse {} as {region:?}", source.display()))?;
    validate_names(&file.directory, "/")?;

    Ok(Archive {
        source,
        data,
        file,
        region,
        file_type,
    })
}

fn validate_names(
    directory: &WzDirectoryEntry,
    path: &str,
) -> Result<()> {
    for name in directory
        .subdirectories
        .iter()
        .map(|entry| entry.name.as_str())
        .chain(directory.images.iter().map(|entry| entry.name.as_str()))
    {
        if name.is_empty() || name.contains('\u{fffd}') || name.chars().any(char::is_control) {
            bail!("WZ node at {path} has an invalid decrypted name; check --region");
        }
    }
    for child in &directory.subdirectories {
        validate_names(child, &join_path(path, &child.name))?;
    }
    Ok(())
}

pub(crate) enum Location<'a> {
    Directory {
        entry: &'a WzDirectoryEntry,
        path: String,
    },
    Image {
        entry: &'a WzImageEntry,
        path: String,
        property_segments: Vec<String>,
    },
}

pub(crate) fn resolve_location<'a>(
    root: &'a WzDirectoryEntry,
    path: &str,
) -> Result<Location<'a>> {
    let segments = parse_path(path)?;
    let mut directory = root;
    let mut current_path = String::from("/");

    for (index, segment) in segments.iter().enumerate() {
        let subdirectory = unique_named(&directory.subdirectories, segment, |entry| &entry.name)?;
        let image = unique_named(&directory.images, segment, |entry| &entry.name)?;

        match (subdirectory, image) {
            (Some(_), Some(_)) => {
                bail!(
                    "WZ path is ambiguous at {}",
                    join_path(&current_path, segment)
                );
            }
            (Some(next), None) => {
                directory = next;
                current_path = join_path(&current_path, segment);
            }
            (None, Some(entry)) => {
                let image_path = join_path(&current_path, segment);
                return Ok(Location::Image {
                    entry,
                    path: image_path,
                    property_segments: segments[index + 1..].to_vec(),
                });
            }
            (None, None) => {
                bail!(
                    "WZ path does not exist: {}",
                    join_path(&current_path, segment)
                );
            }
        }
    }

    Ok(Location::Directory {
        entry: directory,
        path: current_path,
    })
}

fn unique_named<'a, T, F>(
    entries: &'a [T],
    name: &str,
    get_name: F,
) -> Result<Option<&'a T>>
where
    F: Fn(&T) -> &str,
{
    let mut matching = entries.iter().filter(|entry| get_name(entry) == name);
    let result = matching.next();
    if matching.next().is_some() {
        bail!("WZ node name appears more than once: {name}");
    }
    Ok(result)
}

pub(crate) fn parse_path(path: &str) -> Result<Vec<String>> {
    if !path.starts_with('/') {
        bail!("WZ paths must start with '/': {path}");
    }
    if path == "/" {
        return Ok(Vec::new());
    }
    if path.ends_with('/') {
        bail!("WZ paths must not end with '/': {path}");
    }

    path[1..]
        .split('/')
        .map(|segment| {
            if segment.is_empty() || matches!(segment, "." | "..") {
                bail!("WZ path contains an invalid segment: {path}");
            }
            Ok(segment.to_owned())
        })
        .collect()
}

pub(crate) fn join_path(
    parent: &str,
    name: &str,
) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub(crate) struct ParsedImage {
    pub properties: Vec<(String, WzProperty)>,
    pub iv: [u8; 4],
}

pub(crate) fn parse_image(
    archive: &Archive,
    image: &WzImageEntry,
) -> Result<ParsedImage> {
    let bytes = image_bytes(&archive.data, image)?;

    let mut reader = WzBinaryReader::new(
        Cursor::new(bytes),
        archive.file.iv,
        WzHeader::dummy(bytes.len() as u64),
        0,
    );
    let properties = parse_wz_image(&mut reader)
        .with_context(|| format!("failed to parse image {}", image.name))?;

    Ok(ParsedImage {
        properties,
        iv: reader.wz_key.iv(),
    })
}

pub(crate) fn image_bytes<'a>(
    data: &'a [u8],
    image: &WzImageEntry,
) -> Result<&'a [u8]> {
    let size = usize::try_from(image.size)
        .with_context(|| format!("image {} has a negative size", image.name))?;
    let start = usize::try_from(image.offset)
        .with_context(|| format!("image {} offset is too large", image.name))?;
    let end = start
        .checked_add(size)
        .with_context(|| format!("image {} byte range overflows", image.name))?;
    data.get(start..end).with_context(|| {
        format!(
            "image {} byte range {start}..{end} exceeds archive length {}",
            image.name,
            data.len()
        )
    })
}

pub(crate) struct ImageDescriptor {
    pub path: String,
    pub entry: WzImageEntry,
}

pub(crate) fn image_descriptors(root: &WzDirectoryEntry) -> Vec<ImageDescriptor> {
    let mut images = Vec::new();
    collect_images(root, "/", &mut images);
    images
}

pub(crate) fn entry_paths(root: &WzDirectoryEntry) -> Vec<String> {
    let mut entries = Vec::new();
    collect_entry_paths(root, "/", &mut entries);
    entries
}

fn collect_entry_paths(
    directory: &WzDirectoryEntry,
    path: &str,
    entries: &mut Vec<String>,
) {
    for child in &directory.subdirectories {
        let child_path = join_path(path, &child.name);
        entries.push(format!("directory:{child_path}"));
        collect_entry_paths(child, &child_path, entries);
    }
    for image in &directory.images {
        entries.push(format!("image:{}", join_path(path, &image.name)));
    }
}

fn collect_images(
    directory: &WzDirectoryEntry,
    directory_path: &str,
    images: &mut Vec<ImageDescriptor>,
) {
    for image in &directory.images {
        images.push(ImageDescriptor {
            path: join_path(directory_path, &image.name),
            entry: image.clone(),
        });
    }
    for subdirectory in &directory.subdirectories {
        collect_images(
            subdirectory,
            &join_path(directory_path, &subdirectory.name),
            images,
        );
    }
}

pub(crate) fn find_property<'a>(
    properties: &'a [(String, WzProperty)],
    segments: &[String],
) -> Result<(&'a str, &'a WzProperty)> {
    let (first, remaining) = segments
        .split_first()
        .context("a property path is required after the image name")?;
    let (name, property) = unique_property(properties, first)?
        .with_context(|| format!("property does not exist: {first}"))?;

    if remaining.is_empty() {
        return Ok((name, property));
    }
    let children = property
        .children()
        .with_context(|| format!("property {name} has no children"))?;
    find_property(children, remaining)
}

pub(crate) fn find_property_mut<'a>(
    properties: &'a mut [(String, WzProperty)],
    segments: &[String],
) -> Result<(&'a str, &'a mut WzProperty)> {
    let (first, remaining) = segments
        .split_first()
        .context("a property path is required after the image name")?;
    let matching: Vec<usize> = properties
        .iter()
        .enumerate()
        .filter_map(|(index, (name, _))| (name == first).then_some(index))
        .collect();
    let [index] = matching.as_slice() else {
        if matching.is_empty() {
            bail!("property does not exist: {first}");
        }
        bail!("property name appears more than once: {first}");
    };
    let (name, property) = &mut properties[*index];

    if remaining.is_empty() {
        return Ok((name.as_str(), property));
    }
    let children = property_children_mut(property)
        .with_context(|| format!("property {name} has no children"))?;
    find_property_mut(children, remaining)
}

fn unique_property<'a>(
    properties: &'a [(String, WzProperty)],
    name: &str,
) -> Result<Option<(&'a str, &'a WzProperty)>> {
    let mut matching = properties
        .iter()
        .filter(|(property_name, _)| property_name == name);
    let result = matching.next();
    if matching.next().is_some() {
        bail!("property name appears more than once: {name}");
    }
    Ok(result.map(|(property_name, property)| (property_name.as_str(), property)))
}

fn property_children_mut(property: &mut WzProperty) -> Option<&mut Vec<(String, WzProperty)>> {
    match property {
        WzProperty::SubProperty { properties }
        | WzProperty::Canvas { properties, .. }
        | WzProperty::Video { properties, .. } => Some(properties),
        WzProperty::Convex { points } => Some(points),
        WzProperty::RawData { properties, .. } if !properties.is_empty() => Some(properties),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use wzlib_rs::WzFormat;
    use wzlib_rs::wz::directory::WzImageEntry;
    use wzlib_rs::wz::file::compute_version_hash;

    use super::*;

    fn archive_bytes(region: WzMapleVersion) -> Vec<u8> {
        let version = 83;
        let mut directory = WzDirectoryEntry::root();
        directory.images.push(WzImageEntry {
            name: String::from("Real.img"),
            size: 0,
            checksum: 0,
            offset: 0,
            properties: Some(vec![(String::from("value"), WzProperty::Int(1))]),
            iv: None,
        });
        let mut file = WzFile {
            header: WzHeader {
                ident: String::from("PKG1"),
                file_size: 0,
                data_start: 60,
                copyright: String::from("Oozems test archive"),
            },
            version,
            version_hash: compute_version_hash(version),
            maple_version: region,
            iv: region.iv(),
            user_key: None,
            format: WzFormat::Pkg1,
            is_64bit: false,
            directory,
        };
        file.save().unwrap()
    }

    #[test]
    fn explicit_bms_region_preserves_names() {
        let archive = parse_archive(
            archive_bytes(WzMapleVersion::Bms),
            PathBuf::from("test.wz"),
            OpenOptions {
                region: Region::Bms,
                version: None,
            },
        )
        .unwrap();

        assert_eq!(archive.region, Region::Bms);
        assert_eq!(archive.file.directory.images[0].name, "Real.img");
    }

    #[test]
    fn image_parser_cannot_read_past_declared_size() {
        let mut archive = parse_archive(
            archive_bytes(WzMapleVersion::Gms),
            PathBuf::from("test.wz"),
            OpenOptions::default(),
        )
        .unwrap();
        archive.file.directory.images[0].size = 1;

        assert!(parse_image(&archive, &archive.file.directory.images[0]).is_err());
    }
}
