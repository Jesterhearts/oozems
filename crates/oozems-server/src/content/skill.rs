use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::PlayerSkill;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::SkillDefinition;
use oozems_proto::v1::SkillEffect;
use oozems_proto::v1::SkillLevelDefinition;
use oozems_proto::v1::SkillProperty;
use oozems_proto::v1::SkillRequirement;
use oozems_proto::v1::SkillStats;
use oozems_proto::v1::SkillValue;
use oozems_proto::v1::Vec2;
use oozems_proto::v1::skill_value;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::WzAsset;
use super::wz;
use super::wz::WzContentError;

const SKILL_ARCHIVE: &str = "Skill.wz";
const SOUND_ARCHIVE: &str = "Sound.wz";
const STRING_ARCHIVE: &str = "String.wz";
const SKILL_IMAGE: &str = "Skill.img";

mod effect;

pub struct SkillContent {
    _bases: Vec<WzNodeArc>,
    jobs: HashMap<u32, WzNodeArc>,
    strings: WzNodeArc,
    fingerprint: String,
    sounds: Option<WzNodeArc>,
    sound_fingerprint: Option<String>,
    books: RwLock<HashMap<u32, SkillBook>>,
    effects: RwLock<HashMap<(u32, u32, u32), SkillEffect>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

struct SoundArchive {
    base: WzNodeArc,
    skills: WzNodeArc,
    fingerprint: String,
}

#[derive(Debug, Error)]
pub enum SkillContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("skill WZ data is invalid: {message}")]
    Invalid { message: String },
    #[error("internal skill content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

impl SkillContent {
    pub fn open_optional(directory: &Path) -> Result<Option<Self>, SkillContentError> {
        let skill_path = directory.join(SKILL_ARCHIVE);
        if !skill_path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: skill_path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %skill_path.display(), "Skill.wz is absent; skill books will be empty");
            return Ok(None);
        }

        let string_path = directory.join(STRING_ARCHIVE);
        if !string_path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: string_path.clone(),
                source,
            })?
        {
            return invalid(format!(
                "{} is required when {} is present",
                string_path.display(),
                skill_path.display()
            ));
        }

        let skill_root = wz::open_archive(&skill_path)?;
        let skill_base = wz::wrap_archive_root(&skill_root)?;
        wz::parse(&skill_root, format!("{} root", skill_path.display()))?;
        let jobs = index_jobs(&skill_root)?;

        let string_root = wz::open_archive(&string_path)?;
        let string_base = wz::wrap_archive_root(&string_root)?;
        wz::parse(&string_root, format!("{} root", string_path.display()))?;
        let strings = required_child(&string_root, SKILL_IMAGE)?;
        wz::parse(&strings, format!("{} {SKILL_IMAGE}", string_path.display()))?;

        let fingerprint = wz::archive_fingerprint(&skill_path)?;
        let sound_archive = open_sound_archive(directory)?;
        let mut bases = vec![skill_base, string_base];
        let (sounds, sound_fingerprint) = match sound_archive {
            Some(sound) => {
                bases.push(sound.base);
                (Some(sound.skills), Some(sound.fingerprint))
            }
            None => (None, None),
        };
        tracing::info!(
            path = %skill_path.display(),
            jobs = jobs.len(),
            "WZ skill source ready"
        );
        Ok(Some(Self {
            _bases: bases,
            jobs,
            strings,
            fingerprint,
            sounds,
            sound_fingerprint,
            books: RwLock::new(HashMap::new()),
            effects: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
    }

    pub fn skill_book(
        &self,
        job_id: u32,
    ) -> Result<SkillBook, SkillContentError> {
        if let Some(book) = self
            .books
            .read()
            .map_err(|_| lock_error("skill book cache"))?
            .get(&job_id)
            .cloned()
        {
            return Ok(book);
        }

        let book = build_skill_book(self, job_id)?;
        self.books
            .write()
            .map_err(|_| lock_error("skill book cache"))?
            .insert(job_id, book.clone());
        Ok(book)
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }

    pub fn skill_effect(
        &self,
        job_id: u32,
        skill_id: u32,
        level: u32,
    ) -> Result<SkillEffect, SkillContentError> {
        let key = (job_id, skill_id, level);
        if let Some(effect) = self
            .effects
            .read()
            .map_err(|_| lock_error("skill effect cache"))?
            .get(&key)
            .cloned()
        {
            return Ok(effect);
        }

        let effect = effect::build(self, job_id, skill_id, level)?;
        self.effects
            .write()
            .map_err(|_| lock_error("skill effect cache"))?
            .insert(key, effect.clone());
        Ok(effect)
    }

    fn register_asset(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, SkillContentError> {
        let version = hex::encode(Sha256::digest(
            format!("skill\0{}\0{source_path}", self.fingerprint).as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new(id.clone(), Arc::clone(node)));
        self.assets
            .write()
            .map_err(|_| lock_error("skill asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
            content_hash: version,
        })
    }

    fn register_sound(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, SkillContentError> {
        let fingerprint =
            self.sound_fingerprint
                .as_deref()
                .ok_or_else(|| SkillContentError::Invalid {
                    message: "cannot register a sound without Sound.wz".to_owned(),
                })?;
        let version = hex::encode(Sha256::digest(
            format!("skill-sound\0{fingerprint}\0{source_path}").as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new_sound(id.clone(), Arc::clone(node))?);
        let extension = asset.extension();
        self.assets
            .write()
            .map_err(|_| lock_error("skill asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.{extension}"),
            content_hash: version,
        })
    }
}

fn open_sound_archive(directory: &Path) -> Result<Option<SoundArchive>, SkillContentError> {
    let path = directory.join(SOUND_ARCHIVE);
    if !path
        .try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.clone(),
            source,
        })?
    {
        tracing::warn!(path = %path.display(), "Sound.wz is absent; skills will be silent");
        return Ok(None);
    }

    let root = wz::open_archive(&path)?;
    let base = wz::wrap_archive_root(&root)?;
    wz::parse(&root, format!("{} root", path.display()))?;
    let sounds = required_child(&root, SKILL_IMAGE)?;
    wz::parse(&sounds, format!("{} {SKILL_IMAGE}", path.display()))?;
    let fingerprint = wz::archive_fingerprint(&path)?;
    Ok(Some(SoundArchive {
        base,
        skills: sounds,
        fingerprint,
    }))
}

fn index_jobs(root: &WzNodeArc) -> Result<HashMap<u32, WzNodeArc>, SkillContentError> {
    let mut jobs = HashMap::new();
    for node in wz::children(root)? {
        let name = wz::node_name(&node)?;
        let Some(job_id) = name
            .strip_suffix(".img")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if jobs.insert(job_id, node).is_some() {
            return invalid(format!("job {job_id} appears more than once"));
        }
    }
    Ok(jobs)
}

fn build_skill_book(
    content: &SkillContent,
    job_id: u32,
) -> Result<SkillBook, SkillContentError> {
    let name = skill_book_name(&content.strings, job_id)?;
    let Some(job) = content.jobs.get(&job_id) else {
        return Ok(SkillBook {
            job_id,
            name,
            ..SkillBook::default()
        });
    };
    wz::parse(job, format!("{SKILL_ARCHIVE}/{job_id:03}.img"))?;
    let skills = required_child(job, "skill")?;
    let mut entries = Vec::new();
    let mut assets = Vec::new();
    for skill in wz::sorted_children(&skills)? {
        if wz::int_value(&skill, "invisible")?.unwrap_or_default() != 0 {
            continue;
        }
        let skill_id = parse_node_id(&skill, "skill")?;
        let (definition, asset) = build_skill_definition(content, job_id, skill_id, &skill)?;
        entries.push(PlayerSkill {
            definition: Some(definition),
            level: 0,
        });
        assets.push(asset);
    }
    let mut asset_ids = HashSet::new();
    assets.retain(|asset| asset_ids.insert(asset.id.clone()));
    Ok(SkillBook {
        job_id,
        name,
        available_points: 0,
        skills: entries,
        assets,
    })
}

fn build_skill_definition(
    content: &SkillContent,
    job_id: u32,
    skill_id: u32,
    skill: &WzNodeArc,
) -> Result<(SkillDefinition, AssetDescriptor), SkillContentError> {
    let skill_name = format!("{skill_id:07}");
    let text = wz::child(&content.strings, &skill_name)?;
    let name = text
        .as_ref()
        .map(|node| wz::string_value(node, "name"))
        .transpose()?
        .flatten()
        .map(normalize_text)
        .unwrap_or_else(|| skill_name.clone());
    let description = text
        .as_ref()
        .map(|node| wz::string_value(node, "desc"))
        .transpose()?
        .flatten()
        .map(normalize_text)
        .unwrap_or_default();

    let icon = required_child(skill, "icon")?;
    let (icon_width, icon_height) = png_dimensions(&icon, &skill_name)?;
    let asset = content.register_asset(
        &format!("{SKILL_ARCHIVE}/{job_id:03}.img/skill/{skill_name}/icon"),
        &icon,
    )?;
    let levels = build_levels(skill, text.as_ref())?;
    let common_properties = build_properties(wz::child(skill, "common")?.as_ref())?;
    let common_stats = stats_from_properties(&common_properties);
    let metadata = build_metadata(skill)?;
    let requirements = build_requirements(skill)?;
    let max_level = levels
        .iter()
        .map(|level| level.level)
        .max()
        .unwrap_or_default()
        .max(unsigned_literal_property(&metadata, "maxLevel").unwrap_or_default())
        .max(unsigned_literal_property(&common_properties, "maxLevel").unwrap_or_default());

    Ok((
        SkillDefinition {
            skill_id,
            job_id,
            name,
            description,
            max_level,
            icon_asset_id: asset.id.clone(),
            icon_width,
            icon_height,
            levels,
            common_stats: Some(common_stats),
            common_properties,
            metadata,
            requirements,
        },
        asset,
    ))
}

fn build_levels(
    skill: &WzNodeArc,
    text: Option<&WzNodeArc>,
) -> Result<Vec<SkillLevelDefinition>, SkillContentError> {
    let Some(levels) = wz::child(skill, "level")? else {
        return Ok(Vec::new());
    };
    wz::sorted_children(&levels)?
        .into_iter()
        .map(|level| {
            let level_number = parse_node_id(&level, "skill level")?;
            let properties = build_properties(Some(&level))?;
            let description = level_description(text, level_number, &properties)?;
            Ok(SkillLevelDefinition {
                level: level_number,
                description,
                stats: Some(stats_from_properties(&properties)),
                properties,
            })
        })
        .collect()
}

fn level_description(
    text: Option<&WzNodeArc>,
    level: u32,
    properties: &[SkillProperty],
) -> Result<String, SkillContentError> {
    let Some(text) = text else {
        return Ok(String::new());
    };
    let key = text_property(properties, "hs")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("h{level}"));
    Ok(wz::string_value(text, &key)?
        .map(normalize_text)
        .unwrap_or_default())
}

fn build_metadata(skill: &WzNodeArc) -> Result<Vec<SkillProperty>, SkillContentError> {
    let mut properties = Vec::new();
    for child in wz::sorted_children(skill)? {
        if let Some(property) = build_property(&child)? {
            properties.push(property);
        }
    }
    Ok(properties)
}

fn build_properties(node: Option<&WzNodeArc>) -> Result<Vec<SkillProperty>, SkillContentError> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let mut properties = Vec::new();
    for child in wz::sorted_children(node)? {
        if let Some(property) = build_property(&child)? {
            properties.push(property);
        }
    }
    Ok(properties)
}

fn build_property(node: &WzNodeArc) -> Result<Option<SkillProperty>, SkillContentError> {
    let Some(value) = read_skill_value(node)? else {
        return Ok(None);
    };
    Ok(Some(SkillProperty {
        name: wz::node_name(node)?,
        value: Some(value),
    }))
}

fn read_skill_value(node: &WzNodeArc) -> Result<Option<SkillValue>, SkillContentError> {
    let read = node
        .read()
        .map_err(|_| lock_error("skill property value"))?;
    let value = if let Some(value) = read.try_as_int() {
        skill_value::Value::Integer(i64::from(*value))
    } else if let Some(value) = read.try_as_short() {
        skill_value::Value::Integer(i64::from(*value))
    } else if let Some(value) = read.try_as_long() {
        skill_value::Value::Integer(*value)
    } else if let Some(value) = read.try_as_float() {
        skill_value::Value::Decimal(f64::from(*value))
    } else if let Some(value) = read.try_as_double() {
        skill_value::Value::Decimal(*value)
    } else if let Some(value) = read.try_as_string() {
        skill_value::Value::Text(value.get_string().map_err(|error| {
            SkillContentError::Invalid {
                message: format!("a skill string property could not be decoded: {error}"),
            }
        })?)
    } else if let Some(value) = read.try_as_vector2d() {
        skill_value::Value::Vector(Vec2 {
            x: value.0 as f32,
            y: value.1 as f32,
        })
    } else {
        return Ok(None);
    };
    Ok(Some(SkillValue { value: Some(value) }))
}

fn stats_from_properties(properties: &[SkillProperty]) -> SkillStats {
    SkillStats {
        hp_cost: property_value(properties, "hpCon"),
        mp_cost: property_value(properties, "mpCon"),
        hp: property_value(properties, "hp"),
        mp: property_value(properties, "mp"),
        weapon_attack: property_value(properties, "pad"),
        magic_attack: property_value(properties, "mad"),
        accuracy: property_value(properties, "acc"),
        avoidability: property_value(properties, "eva"),
        weapon_defense: property_value(properties, "pdd"),
        magic_defense: property_value(properties, "mdd"),
        speed: property_value(properties, "speed"),
        jump: property_value(properties, "jump"),
        strength: property_value(properties, "str"),
        damage: property_value(properties, "damage"),
        fixed_damage: property_value(properties, "fixdamage"),
        critical_damage: property_value(properties, "criticalDamage"),
        mastery: property_value(properties, "mastery"),
        attack_count: property_value(properties, "attackCount"),
        mob_count: property_value(properties, "mobCount"),
        duration: property_value(properties, "time"),
        cooldown: property_value(properties, "cooltime"),
        range: property_value(properties, "range"),
        success_probability: property_value(properties, "prop"),
        x: property_value(properties, "x"),
        y: property_value(properties, "y"),
        z: property_value(properties, "z"),
    }
}

fn property_value(
    properties: &[SkillProperty],
    name: &str,
) -> Option<SkillValue> {
    properties
        .iter()
        .find(|property| property.name == name)
        .and_then(|property| property.value.clone())
}

fn unsigned_literal_property(
    properties: &[SkillProperty],
    name: &str,
) -> Option<u32> {
    match property_value(properties, name)?.value? {
        skill_value::Value::Integer(value) => u32::try_from(value).ok(),
        skill_value::Value::Text(value) => value.parse().ok(),
        _ => None,
    }
}

fn text_property<'a>(
    properties: &'a [SkillProperty],
    name: &str,
) -> Option<&'a str> {
    properties
        .iter()
        .find(|property| property.name == name)?
        .value
        .as_ref()?
        .value
        .as_ref()
        .and_then(|value| match value {
            skill_value::Value::Text(value) => Some(value.as_str()),
            _ => None,
        })
}

fn build_requirements(skill: &WzNodeArc) -> Result<Vec<SkillRequirement>, SkillContentError> {
    let Some(requirements) = wz::child(skill, "req")? else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for requirement in wz::sorted_children(&requirements)? {
        let skill_id = parse_node_id(&requirement, "required skill")?;
        let value = read_skill_value(&requirement)?;
        let Some(level) = value.as_ref().and_then(requirement_level) else {
            return invalid(format!(
                "required skill {skill_id} does not have an integer level: {value:?}"
            ));
        };
        result.push(SkillRequirement { skill_id, level });
    }
    Ok(result)
}

fn requirement_level(value: &SkillValue) -> Option<u32> {
    match value.value.as_ref()? {
        skill_value::Value::Integer(value) => u32::try_from(*value).ok(),
        skill_value::Value::Text(value) => value.parse().ok(),
        _ => None,
    }
}

fn skill_book_name(
    strings: &WzNodeArc,
    job_id: u32,
) -> Result<String, SkillContentError> {
    let job_name = format!("{job_id:03}");
    Ok(wz::child(strings, &job_name)?
        .as_ref()
        .map(|node| wz::string_value(node, "bookName"))
        .transpose()?
        .flatten()
        .map(normalize_text)
        .unwrap_or_else(|| "Skills".to_owned()))
}

fn normalize_text(value: String) -> String {
    value.replace("\\n", "\n")
}

fn parse_node_id(
    node: &WzNodeArc,
    kind: &str,
) -> Result<u32, SkillContentError> {
    let name = wz::node_name(node)?;
    name.parse::<u32>().map_err(|_| SkillContentError::Invalid {
        message: format!("{kind} node {name:?} does not have a numeric ID"),
    })
}

fn png_dimensions(
    node: &WzNodeArc,
    skill_name: &str,
) -> Result<(f32, f32), SkillContentError> {
    let read = node
        .read()
        .map_err(|_| lock_error("skill icon dimensions"))?;
    let png = read
        .try_as_png()
        .ok_or_else(|| SkillContentError::Invalid {
            message: format!("skill {skill_name} icon is not a PNG sprite"),
        })?;
    if png.width == 0 || png.height == 0 {
        return invalid(format!("skill {skill_name} icon is empty"));
    }
    Ok((png.width as f32, png.height as f32))
}

fn required_child(
    node: &WzNodeArc,
    name: &str,
) -> Result<WzNodeArc, SkillContentError> {
    wz::child(node, name)?.ok_or_else(|| SkillContentError::Invalid {
        message: format!("required node {name:?} is missing"),
    })
}

fn lock_error(context: &'static str) -> SkillContentError {
    SkillContentError::Lock { context }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, SkillContentError> {
    Err(SkillContentError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oozems_proto::v1::SkillAnimationPlacement;
    use oozems_proto::v1::skill_value;

    use super::SkillContent;
    use super::SkillStats;

    #[test]
    fn local_archives_load_all_job_skills_and_typed_values_when_present() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Skill.wz").exists() || !directory.join("String.wz").exists() {
            return;
        }
        let content = SkillContent::open_optional(&directory)
            .expect("sample skill archives should be valid")
            .expect("sample Skill.wz should be present");
        let book = content.skill_book(0).expect("beginner skill book");

        assert_eq!(book.name, "Beginner's Basics");
        assert_eq!(book.skills.len(), 3);
        let snails = book.skills[0]
            .definition
            .as_ref()
            .expect("skill definition");
        assert_eq!(
            (snails.skill_id, snails.name.as_str()),
            (1_000, "Three Snails")
        );
        assert_eq!(snails.max_level, 3);
        assert_eq!(snails.icon_width, 32.0);
        assert_eq!(book.assets.len(), 3);
        let first_level = &snails.levels[0];
        assert_eq!(first_level.level, 1);
        assert_eq!(
            integer(
                first_level
                    .stats
                    .as_ref()
                    .and_then(|stats| stats.mp_cost.as_ref())
            ),
            3
        );
        assert_eq!(
            integer(
                first_level
                    .stats
                    .as_ref()
                    .and_then(|stats| stats.fixed_damage.as_ref())
            ),
            10
        );
        assert_eq!(first_level.description, "MP -3; Damage 10");
        for descriptor in &book.assets {
            assert!(content.get_asset(&descriptor.id).is_some());
        }

        let rogue = content.skill_book(400).expect("rogue skill book");
        let double_stab = rogue
            .skills
            .iter()
            .filter_map(|skill| skill.definition.as_ref())
            .find(|skill| skill.name == "Double Stab")
            .expect("Double Stab skill");
        assert_eq!(double_stab.skill_id, 4_001_334);

        let corsair = content.skill_book(522).expect("corsair skill book");
        let octopi = corsair
            .skills
            .iter()
            .filter_map(|skill| skill.definition.as_ref())
            .find(|skill| skill.name == "Wrath of the Octopi")
            .expect("Wrath of the Octopi skill");
        assert_eq!(octopi.skill_id, 5_220_002);

        assert!(book_has_stats(&content, 110, |stats| {
            stats.weapon_attack.is_some()
        }));
        let mut job_ids = content.jobs.keys().copied().collect::<Vec<_>>();
        job_ids.sort_unstable();
        for job_id in &job_ids {
            content.skill_book(*job_id).expect("job skill book");
        }
        assert!(
            job_ids
                .iter()
                .any(|job_id| book_has_stats(&content, *job_id, |stats| stats.accuracy.is_some()))
        );
        assert!(
            job_ids
                .into_iter()
                .any(|job_id| book_has_text_formula(&content, job_id))
        );
        assert!(book_has_stats(&content, 210, |stats| {
            stats.magic_attack.is_some()
        }));
    }

    #[test]
    fn local_archives_build_three_snails_animation_and_sound() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Skill.wz").exists() || !directory.join("Sound.wz").exists() {
            return;
        }
        let content = SkillContent::open_optional(&directory)
            .expect("sample skill archives should be valid")
            .expect("sample Skill.wz should be present");
        let effect = content
            .skill_effect(0, 1_000, 1)
            .expect("Three Snails effect");
        assert_eq!(effect.animations.len(), 2);
        assert_eq!(
            effect.animations[0].placement,
            SkillAnimationPlacement::Projectile as i32
        );
        assert_eq!(effect.animations[0].frames.len(), 3);
        assert_eq!(
            effect.animations[1].placement,
            SkillAnimationPlacement::Target as i32
        );
        assert_eq!(effect.animations[1].start_delay_ms, 270);
        assert_eq!(effect.animations[1].frames.len(), 6);
        assert_eq!(effect.assets.len(), 9);
        for descriptor in &effect.assets {
            let asset = content
                .get_asset(&descriptor.id)
                .expect("registered animation asset");
            assert!(!asset.png_bytes().expect("animation PNG bytes").is_empty());
        }
        let sound = effect.sound.expect("Three Snails use sound");
        assert!(sound.url.ends_with(".mp3"));
        let sound_asset = content
            .get_asset(&sound.id)
            .expect("registered sound asset");
        assert_eq!(sound_asset.extension(), "mp3");
        assert_eq!(sound_asset.content_type(), "audio/mpeg");
        assert!(!sound_asset.asset_bytes().expect("MP3 bytes").is_empty());
    }

    fn book_has_stats(
        content: &SkillContent,
        job_id: u32,
        predicate: impl Fn(&SkillStats) -> bool,
    ) -> bool {
        content
            .skill_book(job_id)
            .expect("job skill book")
            .skills
            .iter()
            .filter_map(|skill| skill.definition.as_ref())
            .any(|definition| {
                definition.common_stats.as_ref().is_some_and(&predicate)
                    || definition
                        .levels
                        .iter()
                        .filter_map(|level| level.stats.as_ref())
                        .any(&predicate)
            })
    }

    fn book_has_text_formula(
        content: &SkillContent,
        job_id: u32,
    ) -> bool {
        content
            .skill_book(job_id)
            .expect("job skill book")
            .skills
            .iter()
            .filter_map(|skill| skill.definition.as_ref())
            .any(|definition| {
                definition
                    .common_properties
                    .iter()
                    .chain(
                        definition
                            .levels
                            .iter()
                            .flat_map(|level| level.properties.iter()),
                    )
                    .any(|property| {
                        matches!(
                            property
                                .value
                                .as_ref()
                                .and_then(|value| value.value.as_ref()),
                            Some(skill_value::Value::Text(_))
                        ) && matches!(
                            property.name.as_str(),
                            "acc" | "attackCount" | "damage" | "time"
                        )
                    })
            })
    }

    fn integer(value: Option<&oozems_proto::v1::SkillValue>) -> i64 {
        match value.and_then(|value| value.value.as_ref()) {
            Some(skill_value::Value::Integer(value)) => *value,
            _ => panic!("expected integer skill value"),
        }
    }
}
