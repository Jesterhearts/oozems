use std::io::Cursor;
use std::sync::Arc;
use std::sync::OnceLock;

use image::ImageFormat;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::WzSoundType;
use wz_reader::property::png::get_image;

use super::WzContentError;
use super::lock_error;

pub(crate) struct WzAsset {
    id: String,
    node: WzNodeArc,
    kind: WzAssetKind,
    bytes: OnceLock<Arc<[u8]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WzAssetKind {
    Png,
    Mp3,
    Wav,
}

impl WzAsset {
    pub(in crate::content) fn new(
        id: String,
        node: WzNodeArc,
    ) -> Self {
        Self {
            id,
            node,
            kind: WzAssetKind::Png,
            bytes: OnceLock::new(),
        }
    }

    pub(in crate::content) fn new_sound(
        id: String,
        node: WzNodeArc,
    ) -> Result<Self, WzContentError> {
        let kind = {
            let read = node.read().map_err(|_| lock_error("WZ sound asset"))?;
            let sound = read
                .try_as_sound()
                .ok_or_else(|| WzContentError::InvalidAsset {
                    asset_id: id.clone(),
                    message: "source is not a sound property".to_owned(),
                })?;
            match sound.sound_type {
                WzSoundType::Mp3 => WzAssetKind::Mp3,
                WzSoundType::Wav => WzAssetKind::Wav,
                WzSoundType::Binary => {
                    return Err(WzContentError::InvalidAsset {
                        asset_id: id,
                        message: "binary sound data has no browser media type".to_owned(),
                    });
                }
            }
        };
        Ok(Self {
            id,
            node,
            kind,
            bytes: OnceLock::new(),
        })
    }

    pub fn asset_bytes(&self) -> Result<Arc<[u8]>, WzContentError> {
        if let Some(bytes) = self.bytes.get() {
            return Ok(Arc::clone(bytes));
        }

        let bytes: Arc<[u8]> = match self.kind {
            WzAssetKind::Png => {
                let image =
                    get_image(&self.node).map_err(|source| WzContentError::DecodeAsset {
                        asset_id: self.id.clone(),
                        source,
                    })?;
                let mut output = Cursor::new(Vec::new());
                image
                    .write_to(&mut output, ImageFormat::Png)
                    .map_err(|source| WzContentError::EncodeAsset {
                        asset_id: self.id.clone(),
                        source,
                    })?;
                output.into_inner().into()
            }
            WzAssetKind::Mp3 | WzAssetKind::Wav => {
                let read = self.node.read().map_err(|_| lock_error("WZ sound bytes"))?;
                read.try_as_sound()
                    .ok_or_else(|| WzContentError::InvalidAsset {
                        asset_id: self.id.clone(),
                        message: "source is no longer a sound property".to_owned(),
                    })?
                    .get_buffer()
                    .into()
            }
        };
        let _ = self.bytes.set(Arc::clone(&bytes));
        Ok(self.bytes.get().cloned().unwrap_or(bytes))
    }

    pub fn extension(&self) -> &'static str {
        match self.kind {
            WzAssetKind::Png => "png",
            WzAssetKind::Mp3 => "mp3",
            WzAssetKind::Wav => "wav",
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self.kind {
            WzAssetKind::Png => "image/png",
            WzAssetKind::Mp3 => "audio/mpeg",
            WzAssetKind::Wav => "audio/wav",
        }
    }
}
