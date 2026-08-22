use std::fs;
use std::path::Path;
use std::sync::Arc;

use sha2::Digest;
use sha2::Sha256;
use wz_reader::WzFile;
use wz_reader::WzNode;
use wz_reader::WzNodeArc;

use super::WzContentError;
use super::lock_error;

pub(in crate::content) fn open_archive(path: &Path) -> Result<WzNodeArc, WzContentError> {
    WzNode::from_wz_file(path, None)
        .map(Into::into)
        .map_err(|source| WzContentError::Open {
            path: path.to_owned(),
            source,
        })
}

pub(in crate::content) fn wrap_archive_root(root: &WzNodeArc) -> Result<WzNodeArc, WzContentError> {
    let base = WzNode::from_str("Base", WzFile::default(), None).into_lock();
    root.write()
        .map_err(|_| lock_error("WZ archive parent"))?
        .parent = Arc::downgrade(&base);
    base.write()
        .map_err(|_| lock_error("WZ synthetic Base root"))?
        .add(root);
    Ok(base)
}

pub(in crate::content) fn archive_fingerprint(path: &Path) -> Result<String, WzContentError> {
    let metadata = fs::metadata(path).map_err(|source| WzContentError::Metadata {
        path: path.to_owned(),
        source,
    })?;
    let modified = metadata
        .modified()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(hex::encode(Sha256::digest(
        format!("{}:{modified}", metadata.len()).as_bytes(),
    )))
}
