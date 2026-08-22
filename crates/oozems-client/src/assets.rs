use std::cell::Cell;
use std::collections::HashMap;

use oozems_proto::v1::AssetDescriptor;
use web_sys::HtmlImageElement;

use crate::js_error;

pub struct BrowserAsset {
    pub element: HtmlImageElement,
    requested: Cell<bool>,
    url: String,
}

pub fn prepare_assets<'a>(
    assets: impl IntoIterator<Item = &'a AssetDescriptor>
) -> Result<HashMap<String, BrowserAsset>, String> {
    assets
        .into_iter()
        .map(|asset| {
            let image = HtmlImageElement::new().map_err(js_error)?;
            image.set_decoding("async");
            Ok((
                asset.id.clone(),
                BrowserAsset {
                    element: image,
                    requested: Cell::new(false),
                    url: asset.url.clone(),
                },
            ))
        })
        .collect()
}

pub fn ready_image<'a>(
    assets: &'a HashMap<String, BrowserAsset>,
    asset_id: &str,
) -> Option<&'a HtmlImageElement> {
    let asset = assets.get(asset_id)?;
    if !asset.requested.replace(true) {
        asset.element.set_src(&asset.url);
    }
    (asset.element.complete() && asset.element.natural_width() > 0).then_some(&asset.element)
}
