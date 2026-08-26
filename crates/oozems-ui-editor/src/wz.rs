use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use wz_reader::WzFile;
use wz_reader::WzNode;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::WzPngParseError;
use wz_reader::property::png::get_image;
use wz_reader::util::node_util::parse_node;

pub struct UiArchive {
    _base: WzNodeArc,
    root: WzNodeArc,
}

pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("failed to open UI archive {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: wz_reader::node::Error,
    },
    #[error("failed to parse UI archive node {path:?}")]
    Parse {
        path: String,
        #[source]
        source: wz_reader::node::Error,
    },
    #[error("UI archive node {path:?} does not exist")]
    Missing { path: String },
    #[error("UI archive node {path:?} is not a PNG image")]
    NotPng { path: String },
    #[error("failed to decode UI archive image {path:?}")]
    Decode {
        path: String,
        #[source]
        source: WzPngParseError,
    },
    #[error("UI archive lock was poisoned while reading {context}")]
    Lock { context: &'static str },
}

impl UiArchive {
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let root: WzNodeArc =
            WzNode::from_wz_file(path, None)
                .map(Into::into)
                .map_err(|source| ArchiveError::Open {
                    path: path.to_owned(),
                    source,
                })?;
        let base = WzNode::from_str("Base", WzFile::default(), None).into_lock();
        root.write()
            .map_err(|_| ArchiveError::Lock {
                context: "archive parent",
            })?
            .parent = Arc::downgrade(&base);
        base.write()
            .map_err(|_| ArchiveError::Lock {
                context: "synthetic archive root",
            })?
            .add(&root);
        parse_node(&root).map_err(|source| ArchiveError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self { _base: base, root })
    }

    pub fn load_image(
        &self,
        path: &str,
    ) -> Result<PreviewImage, ArchiveError> {
        let node = self.image_node(path)?;
        let decoded = get_image(&node).map_err(|source| ArchiveError::Decode {
            path: path.to_owned(),
            source,
        })?;
        let rgba = decoded.into_rgba8();
        Ok(PreviewImage {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        })
    }

    pub fn image_dimensions(
        &self,
        path: &str,
    ) -> Result<(u32, u32), ArchiveError> {
        let node = self.image_node(path)?;
        let read = node.read().map_err(|_| ArchiveError::Lock {
            context: "archive PNG",
        })?;
        let png = read.try_as_png().ok_or_else(|| ArchiveError::NotPng {
            path: path.to_owned(),
        })?;
        Ok((png.width, png.height))
    }

    fn image_node(
        &self,
        path: &str,
    ) -> Result<WzNodeArc, ArchiveError> {
        let (image_name, property_path) =
            path.split_once('/').ok_or_else(|| ArchiveError::Missing {
                path: path.to_owned(),
            })?;
        let image = self
            .root
            .read()
            .map_err(|_| ArchiveError::Lock {
                context: "archive root",
            })?
            .at(image_name)
            .ok_or_else(|| ArchiveError::Missing {
                path: image_name.to_owned(),
            })?;
        parse_node(&image).map_err(|source| ArchiveError::Parse {
            path: image_name.to_owned(),
            source,
        })?;
        let node = image
            .read()
            .map_err(|_| ArchiveError::Lock {
                context: "archive image",
            })?
            .at_path(property_path)
            .ok_or_else(|| ArchiveError::Missing {
                path: path.to_owned(),
            })?;
        Ok(node)
    }
}
