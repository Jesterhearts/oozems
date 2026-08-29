use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use oozems_wz::OpenOptions;
use oozems_wz::PropertyEdit;
use serde_json::Value;

pub fn write_archive_edits(
    source: &Path,
    output: &Path,
    edits: &BTreeMap<String, Value>,
    options: OpenOptions,
) -> Result<usize> {
    if edits.is_empty() {
        bail!("there are no staged changes to save");
    }
    if paths_match(source, output)? {
        bail!("edited WZ output must differ from its source archive");
    }
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::tempdir_in(parent).with_context(|| {
        format!(
            "failed to create temporary archive directory in {}",
            parent.display()
        )
    })?;
    let mut image_edits = BTreeMap::<String, Vec<(String, Value)>>::new();
    for (path, value) in edits {
        let image = path
            .strip_prefix('/')
            .and_then(|path| path.split('/').next())
            .filter(|image| !image.is_empty())
            .with_context(|| format!("invalid staged WZ path {path:?}"))?;
        image_edits
            .entry(image.to_owned())
            .or_default()
            .push((path.clone(), value.clone()));
    }
    let image_count = image_edits.len();
    let mut input = source.to_owned();
    for (index, edits) in image_edits.into_values().enumerate() {
        let last = index + 1 == image_count;
        let next = if last {
            output.to_owned()
        } else {
            temporary.path().join(format!("edit-{index}.wz"))
        };
        oozems_wz::set_values(&input, &next, &edits, options, true)
            .with_context(|| format!("failed to apply edits in /{}", image_key(&edits)))?;
        if input.starts_with(temporary.path()) {
            fs::remove_file(&input).with_context(|| {
                format!("failed to remove intermediate archive {}", input.display())
            })?;
        }
        input = next;
    }
    Ok(edits.len())
}

pub fn write_archive_property_edits(
    source: &Path,
    output: &Path,
    edits: &[PropertyEdit],
    options: OpenOptions,
) -> Result<usize> {
    if edits.is_empty() {
        bail!("there are no staged WZ changes to save");
    }
    if paths_match(source, output)? {
        bail!("edited WZ output must differ from its source archive");
    }
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::tempdir_in(parent).with_context(|| {
        format!(
            "failed to create temporary archive directory in {}",
            parent.display()
        )
    })?;
    let mut image_edits = BTreeMap::<String, Vec<PropertyEdit>>::new();
    for edit in edits {
        let image = image_key_from_path(edit.path())?;
        image_edits
            .entry(image.to_owned())
            .or_default()
            .push(edit.clone());
    }
    let image_count = image_edits.len();
    let mut input = source.to_owned();
    for (index, edits) in image_edits.into_values().enumerate() {
        let last = index + 1 == image_count;
        let next = if last {
            output.to_owned()
        } else {
            temporary.path().join(format!("structural-edit-{index}.wz"))
        };
        oozems_wz::edit_properties(&input, &next, &edits, options, true).with_context(|| {
            format!(
                "failed to edit /{}",
                image_key_from_path(edits[0].path()).unwrap_or("unknown image")
            )
        })?;
        if input.starts_with(temporary.path()) {
            fs::remove_file(&input).with_context(|| {
                format!("failed to remove intermediate archive {}", input.display())
            })?;
        }
        input = next;
    }
    Ok(edits.len())
}

pub fn write_archive_source(
    source: &Path,
    output: &Path,
) -> Result<()> {
    if paths_match(source, output)? {
        bail!("edited WZ output must differ from its source archive");
    }
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary archive file in {}",
            parent.display()
        )
    })?;
    fs::copy(source, temporary.path()).with_context(|| {
        format!(
            "failed to copy source archive {} to a temporary file",
            source.display()
        )
    })?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to flush temporary file for {}", output.display()))?;
    temporary
        .persist(output)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to install {}", output.display()))?;
    Ok(())
}

fn image_key_from_path(path: &str) -> Result<&str> {
    path.strip_prefix('/')
        .and_then(|path| path.split('/').next())
        .filter(|image| !image.is_empty())
        .with_context(|| format!("invalid staged WZ path {path:?}"))
}

fn image_key(edits: &[(String, Value)]) -> &str {
    edits
        .first()
        .and_then(|(path, _)| path.strip_prefix('/'))
        .and_then(|path| path.split('/').next())
        .unwrap_or("unknown image")
}

fn paths_match(
    source: &Path,
    output: &Path,
) -> Result<bool> {
    if source == output {
        return Ok(true);
    }
    if !output.exists() {
        return Ok(false);
    }
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve source path {}", source.display()))?;
    let output = fs::canonicalize(output)
        .with_context(|| format!("failed to resolve output path {}", output.display()))?;
    Ok(source == output)
}

pub fn write_text_atomic(
    path: &Path,
    text: &str,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    temporary
        .write_all(text.as_bytes())
        .with_context(|| format!("failed to write temporary file for {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to flush temporary file for {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to install {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_output_cannot_alias_its_source() {
        let mut edits = BTreeMap::new();
        edits.insert("/100.img/value".to_owned(), Value::from(2));
        let error = write_archive_edits(
            Path::new("same.wz"),
            Path::new("same.wz"),
            &edits,
            OpenOptions::default(),
        )
        .expect_err("source alias must fail");

        assert!(error.to_string().contains("must differ"));
    }

    #[test]
    fn text_save_atomically_replaces_existing_content() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("scripts.toml");
        fs::write(&path, "old").expect("write old content");

        write_text_atomic(&path, "new").expect("replace content");

        assert_eq!(fs::read_to_string(path).expect("read content"), "new");
    }

    #[test]
    fn archive_source_atomically_replaces_a_stale_edited_output() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("Skill.wz");
        let output = directory.path().join("Skill.edited.wz");
        fs::write(&source, b"original").expect("write source");
        fs::write(&output, b"stale edit").expect("write stale output");

        write_archive_source(&source, &output).expect("restore source content");

        assert_eq!(fs::read(output).expect("read output"), b"original");
    }
}
