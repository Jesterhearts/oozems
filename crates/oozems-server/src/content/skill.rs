use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::LearnedSkill;
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
use super::sound::SoundContent;
use super::wz;
use super::wz::WzContentError;

const SKILL_ARCHIVE: &str = "Skill.wz";
const STRING_ARCHIVE: &str = "String.wz";
const SKILL_IMAGE: &str = "Skill.img";

mod effect;

pub struct SkillContent {
    _bases: Vec<WzNodeArc>,
    jobs: HashMap<u32, WzNodeArc>,
    skills: HashMap<u32, IndexedSkill>,
    strings: WzNodeArc,
    fingerprint: String,
    sounds: Option<Arc<SoundContent>>,
    books: RwLock<HashMap<u32, SkillBook>>,
    definitions: RwLock<HashMap<u32, CachedSkillDefinition>>,
    effects: RwLock<HashMap<(u32, u32, u32), SkillEffect>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

struct IndexedSkill {
    job_id: u32,
    invisible: bool,
    node: WzNodeArc,
}

#[derive(Clone)]
struct CachedSkillDefinition {
    definition: SkillDefinition,
    asset: AssetDescriptor,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthoritativeSkillDefinition {
    pub definition: SkillDefinition,
    pub invisible: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SkillBookContext {
    pub book: SkillBook,
    pub authoritative_skills: Vec<AuthoritativeSkillDefinition>,
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
    pub fn open_optional(
        directory: &Path,
        sounds: Option<Arc<SoundContent>>,
    ) -> Result<Option<Self>, SkillContentError> {
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
        let skills = index_skills(&jobs)?;

        let string_root = wz::open_archive(&string_path)?;
        let string_base = wz::wrap_archive_root(&string_root)?;
        wz::parse(&string_root, format!("{} root", string_path.display()))?;
        let strings = required_child(&string_root, SKILL_IMAGE)?;
        wz::parse(&strings, format!("{} {SKILL_IMAGE}", string_path.display()))?;

        let fingerprint = wz::archive_fingerprint(&skill_path)?;
        let bases = vec![skill_base, string_base];
        tracing::info!(
            path = %skill_path.display(),
            jobs = jobs.len(),
            "WZ skill source ready"
        );
        Ok(Some(Self {
            _bases: bases,
            jobs,
            skills,
            strings,
            fingerprint,
            sounds,
            books: RwLock::new(HashMap::new()),
            definitions: RwLock::new(HashMap::new()),
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

    pub(crate) fn skill_book_context(
        &self,
        job_id: u32,
        learned_skills: &[LearnedSkill],
    ) -> Result<SkillBookContext, SkillContentError> {
        let mut book = self.skill_book(job_id)?;
        let mut displayed = book
            .skills
            .iter()
            .filter_map(|skill| skill.definition.as_ref())
            .map(|definition| definition.skill_id)
            .collect::<HashSet<_>>();
        let mut authoritative = book
            .skills
            .iter()
            .filter_map(|skill| skill.definition.clone())
            .map(|definition| {
                (
                    definition.skill_id,
                    AuthoritativeSkillDefinition {
                        definition,
                        invisible: false,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        for learned in learned_skills {
            let indexed =
                self.skills
                    .get(&learned.skill_id)
                    .ok_or_else(|| SkillContentError::Invalid {
                        message: format!(
                            "learned skill {} is absent from authoritative Skill.wz",
                            learned.skill_id
                        ),
                    })?;
            let cached = self.cached_definition(learned.skill_id)?;
            authoritative.insert(
                learned.skill_id,
                AuthoritativeSkillDefinition {
                    definition: cached.definition.clone(),
                    invisible: indexed.invisible,
                },
            );
            let unlocked = learned.level > 0 || learned.master_level > 0;
            let eligible = indexed.job_id == job_id || is_beginner_family_skill(learned.skill_id);
            if unlocked
                && eligible
                && cached.definition.max_level > 0
                && displayed.insert(learned.skill_id)
            {
                book.skills.push(PlayerSkill {
                    definition: Some(cached.definition),
                    level: 0,
                    master_level: 0,
                });
                book.assets.push(cached.asset);
            }
        }
        book.skills.sort_by_key(|skill| {
            skill
                .definition
                .as_ref()
                .map_or(u32::MAX, |definition| definition.skill_id)
        });
        let mut asset_ids = HashSet::new();
        book.assets
            .retain(|asset| asset_ids.insert(asset.id.clone()));
        let mut authoritative_skills = authoritative.into_values().collect::<Vec<_>>();
        authoritative_skills.sort_by_key(|skill| skill.definition.skill_id);
        Ok(SkillBookContext {
            book,
            authoritative_skills,
        })
    }

    pub(crate) fn authoritative_skill_ids(&self) -> BTreeSet<u32> {
        self.skills.keys().copied().collect()
    }

    pub(crate) fn authoritative_skill_names(
        &self
    ) -> Result<BTreeMap<u32, String>, SkillContentError> {
        let mut names = BTreeMap::new();
        for skill_id in self.skills.keys().copied() {
            let key = format!("{skill_id:07}");
            let Some(text) = wz::child(&self.strings, &key)? else {
                continue;
            };
            let Some(name) = wz::string_value(&text, "name")?
                .map(normalize_text)
                .filter(|name| !name.trim().is_empty())
            else {
                continue;
            };
            names.insert(skill_id, name);
        }
        Ok(names)
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
        })
    }

    fn cached_definition(
        &self,
        skill_id: u32,
    ) -> Result<CachedSkillDefinition, SkillContentError> {
        if let Some(cached) = self
            .definitions
            .read()
            .map_err(|_| lock_error("skill definition cache"))?
            .get(&skill_id)
            .cloned()
        {
            return Ok(cached);
        }
        let indexed = self
            .skills
            .get(&skill_id)
            .ok_or_else(|| SkillContentError::Invalid {
                message: format!("skill {skill_id} is absent from authoritative Skill.wz"),
            })?;
        let (definition, asset) =
            build_skill_definition(self, indexed.job_id, skill_id, &indexed.node)?;
        let cached = CachedSkillDefinition { definition, asset };
        self.definitions
            .write()
            .map_err(|_| lock_error("skill definition cache"))?
            .insert(skill_id, cached.clone());
        Ok(cached)
    }
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

fn index_skills(
    jobs: &HashMap<u32, WzNodeArc>
) -> Result<HashMap<u32, IndexedSkill>, SkillContentError> {
    let mut job_ids = jobs.keys().copied().collect::<Vec<_>>();
    job_ids.sort_unstable();
    let mut indexed = HashMap::new();
    for job_id in job_ids {
        let job = &jobs[&job_id];
        wz::parse(job, format!("{SKILL_ARCHIVE}/{job_id:03}.img"))?;
        let skills = required_child(job, "skill")?;
        for skill in wz::sorted_children(&skills)? {
            let skill_id = parse_node_id(&skill, "skill")?;
            let entry = IndexedSkill {
                job_id,
                invisible: wz::int_value(&skill, "invisible")?.unwrap_or_default() != 0,
                node: skill,
            };
            if indexed.insert(skill_id, entry).is_some() {
                return invalid(format!("skill {skill_id} appears more than once"));
            }
        }
    }
    Ok(indexed)
}

fn build_skill_book(
    content: &SkillContent,
    job_id: u32,
) -> Result<SkillBook, SkillContentError> {
    let name = skill_book_name(&content.strings, job_id)?;
    if !content.jobs.contains_key(&job_id) {
        return Ok(SkillBook {
            job_id,
            name,
            ..SkillBook::default()
        });
    };
    let mut entries = Vec::new();
    let mut assets = Vec::new();
    let mut skill_ids = content
        .skills
        .iter()
        .filter(|(_, skill)| skill.job_id == job_id && !skill.invisible)
        .map(|(skill_id, _)| *skill_id)
        .collect::<Vec<_>>();
    skill_ids.sort_unstable();
    for skill_id in skill_ids {
        let cached = content.cached_definition(skill_id)?;
        entries.push(PlayerSkill {
            definition: Some(cached.definition),
            level: 0,
            master_level: 0,
        });
        assets.push(cached.asset);
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

fn is_beginner_family_skill(skill_id: u32) -> bool {
    skill_id % 10_000_000 < 10_000
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
    use std::collections::HashMap;
    use std::sync::RwLock;

    use oozems_proto::v1::AssetDescriptor;
    use oozems_proto::v1::LearnedSkill;
    use oozems_proto::v1::SkillDefinition;
    use oozems_proto::v1::skill_value;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::CachedSkillDefinition;
    use super::IndexedSkill;
    use super::SkillContent;
    use super::build_properties;
    use super::stats_from_properties;

    #[test]
    fn authoritative_index_includes_hidden_skills_without_exposing_locked_entries() {
        let content = synthetic_skill_content();
        let ids = content.authoritative_skill_ids();
        assert!(ids.contains(&2_321_003));
        assert!(ids.contains(&1_003));

        let locked = content.skill_book(232).expect("bishop skill book");
        assert!(book_contains(&locked, 2_321_001));
        assert!(!book_contains(&locked, 2_321_003));
        let unlocked = content
            .skill_book_context(
                232,
                &[LearnedSkill {
                    skill_id: 2_321_003,
                    level: 0,
                    master_level: 15,
                }],
            )
            .expect("unlocked hidden skill book");
        assert!(book_contains(&unlocked.book, 2_321_003));
        assert!(
            unlocked
                .authoritative_skills
                .iter()
                .find(|skill| skill.definition.skill_id == 2_321_003)
                .expect("hidden authoritative skill")
                .invisible
        );

        let beginner = content
            .skill_book_context(
                112,
                &[LearnedSkill {
                    skill_id: 1_003,
                    level: 1,
                    master_level: 1,
                }],
            )
            .expect("beginner-family bypass");
        assert!(book_contains(&beginner.book, 1_003));
    }

    #[test]
    fn synthetic_wz_properties_preserve_integer_and_formula_values() {
        let node = property_node("level");
        add_value(&node, "mpCon", WzValue::Int(3));
        add_value(
            &node,
            "damage",
            WzValue::String(WzString::from_str("10+2*x", [0; 4])),
        );

        let properties = build_properties(Some(&node)).expect("synthetic skill properties");
        let stats = stats_from_properties(&properties);
        assert!(matches!(
            stats.mp_cost.and_then(|value| value.value),
            Some(skill_value::Value::Integer(3))
        ));
        assert!(matches!(
            stats.damage.and_then(|value| value.value),
            Some(skill_value::Value::Text(value)) if value == "10+2*x"
        ));
    }

    fn synthetic_skill_content() -> SkillContent {
        let node = property_node("skill");
        let skills = HashMap::from([
            (
                2_321_001,
                IndexedSkill {
                    job_id: 232,
                    invisible: false,
                    node: node.clone(),
                },
            ),
            (
                2_321_003,
                IndexedSkill {
                    job_id: 232,
                    invisible: true,
                    node: node.clone(),
                },
            ),
            (
                1_003,
                IndexedSkill {
                    job_id: 0,
                    invisible: false,
                    node,
                },
            ),
        ]);
        let definitions = [2_321_001, 2_321_003, 1_003]
            .into_iter()
            .map(|skill_id| {
                (
                    skill_id,
                    CachedSkillDefinition {
                        definition: SkillDefinition {
                            skill_id,
                            max_level: 1,
                            ..SkillDefinition::default()
                        },
                        asset: AssetDescriptor::default(),
                    },
                )
            })
            .collect();
        SkillContent {
            _bases: Vec::new(),
            jobs: HashMap::from([(232, property_node("232.img"))]),
            skills,
            strings: property_node("strings"),
            fingerprint: "synthetic".to_owned(),
            sounds: None,
            books: RwLock::new(HashMap::new()),
            definitions: RwLock::new(definitions),
            effects: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }
    }

    fn book_contains(
        book: &oozems_proto::v1::SkillBook,
        skill_id: u32,
    ) -> bool {
        book.skills.iter().any(|skill| {
            skill
                .definition
                .as_ref()
                .is_some_and(|definition| definition.skill_id == skill_id)
        })
    }

    fn property_node(name: &str) -> wz_reader::WzNodeArc {
        WzNode::from_str(name, WzObjectType::Property(WzSubProperty::Property), None).into_lock()
    }

    fn add_value(
        parent: &wz_reader::WzNodeArc,
        name: &str,
        value: WzValue,
    ) {
        let child = WzNode::from_str(name, WzObjectType::Value(value), Some(parent)).into_lock();
        parent
            .write()
            .expect("property lock")
            .children
            .insert(name.into(), child);
    }
}
