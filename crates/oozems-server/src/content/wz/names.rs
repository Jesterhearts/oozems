use std::collections::HashMap;
use std::path::Path;

use wz_reader::WzNodeArc;

use super::WzContentError;
use super::child;
use super::children;
use super::node_name;
use super::open_archive;
use super::parse;
use super::string_value;

pub(super) fn load_map_names(path: &Path) -> Result<HashMap<u32, String>, WzContentError> {
    if !path
        .try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })?
    {
        tracing::warn!(path = %path.display(), "String.wz is absent; using numeric map names");
        return Ok(HashMap::new());
    }

    let root = open_archive(path)?;
    parse(&root, format!("{} root", path.display()))?;
    let map_image = child(&root, "Map.img")?.ok_or_else(|| WzContentError::InvalidMap {
        map_id: 0,
        message: format!("{} does not contain Map.img", path.display()),
    })?;
    parse(&map_image, format!("{} Map.img", path.display()))?;
    let mut names = HashMap::new();
    collect_map_names(&map_image, &mut names)?;
    Ok(names)
}

fn collect_map_names(
    node: &WzNodeArc,
    names: &mut HashMap<u32, String>,
) -> Result<(), WzContentError> {
    let name = node_name(node)?;
    if let Ok(map_id) = name.parse::<u32>()
        && let Some(map_name) = string_value(node, "mapName")?.filter(|value| !value.is_empty())
    {
        names.insert(map_id, map_name);
    }
    for child in children(node)? {
        collect_map_names(&child, names)?;
    }
    Ok(())
}
