use std::cell::Cell;
use std::collections::HashMap;
use std::hash::Hash;

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

pub fn merge_assets(
    prepared: &mut HashMap<String, BrowserAsset>,
    additions: HashMap<String, BrowserAsset>,
) {
    merge_missing(prepared, additions);
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

pub fn images_ready<'a>(
    assets: &HashMap<String, BrowserAsset>,
    asset_ids: impl IntoIterator<Item = &'a str>,
) -> bool {
    let mut all_ready = true;
    for asset_id in asset_ids {
        all_ready &= ready_image(assets, asset_id).is_some();
    }
    all_ready
}

pub fn ready_or_fallback_index<'a>(
    assets: &HashMap<String, BrowserAsset>,
    asset_ids: impl IntoIterator<Item = &'a str>,
    preferred_index: usize,
) -> Option<usize> {
    preferred_or_first_ready(
        asset_ids
            .into_iter()
            .map(|asset_id| ready_image(assets, asset_id).is_some()),
        preferred_index,
    )
}

fn merge_missing<K, V>(
    target: &mut HashMap<K, V>,
    additions: HashMap<K, V>,
) where
    K: Eq + Hash,
{
    for (key, value) in additions {
        target.entry(key).or_insert(value);
    }
}

fn preferred_or_first_ready(
    readiness: impl IntoIterator<Item = bool>,
    preferred_index: usize,
) -> Option<usize> {
    let mut preferred_ready = false;
    let mut first_ready = None;
    for (index, ready) in readiness.into_iter().enumerate() {
        if !ready {
            continue;
        }
        first_ready.get_or_insert(index);
        preferred_ready |= index == preferred_index;
    }
    preferred_ready.then_some(preferred_index).or(first_ready)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use super::merge_missing;
    use super::preferred_or_first_ready;

    #[test]
    fn merging_assets_preserves_existing_entries() {
        let mut target = HashMap::from([("shared", 1), ("old", 2)]);
        let additions = HashMap::from([("shared", 3), ("new", 4)]);

        merge_missing(&mut target, additions);

        assert_eq!(target["shared"], 1);
        assert_eq!(target["old"], 2);
        assert_eq!(target["new"], 4);
    }

    #[test]
    fn preferred_frame_is_used_when_ready() {
        assert_eq!(preferred_or_first_ready([true, false, true], 2), Some(2));
    }

    #[test]
    fn first_ready_frame_is_a_stable_fallback() {
        assert_eq!(preferred_or_first_ready([false, true, false], 2), Some(1));
        assert_eq!(preferred_or_first_ready([false, false], 1), None);
    }

    #[test]
    fn readiness_checks_do_not_stop_at_the_first_missing_frame() {
        let checked = Cell::new(0);
        let readiness = [false, true, false].into_iter().map(|ready| {
            checked.set(checked.get() + 1);
            ready
        });

        assert_eq!(preferred_or_first_ready(readiness, 0), Some(1));
        assert_eq!(checked.get(), 3);
    }
}
