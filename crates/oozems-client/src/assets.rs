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
    let mut prepared = HashMap::new();
    insert_assets(&mut prepared, assets)?;
    Ok(prepared)
}

pub fn insert_assets<'a>(
    prepared: &mut HashMap<String, BrowserAsset>,
    assets: impl IntoIterator<Item = &'a AssetDescriptor>,
) -> Result<(), String> {
    for asset in assets {
        if prepared.contains_key(&asset.id) {
            continue;
        }
        let image = HtmlImageElement::new().map_err(js_error)?;
        image.set_decoding("async");
        prepared.insert(
            asset.id.clone(),
            BrowserAsset {
                element: image,
                requested: Cell::new(false),
                url: asset.url.clone(),
            },
        );
    }
    Ok(())
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
