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
use oozems_proto::v1::SkillActivation as ProtoSkillActivation;
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
use oozems_skill_semantics::NormalizedSkillStat;
use oozems_skill_semantics::OverloadedSkillProperty;
use oozems_skill_semantics::SkillActivation;
use oozems_skill_semantics::SkillArchiveFacts;
use oozems_skill_semantics::SkillPropertyScope;
use oozems_skill_semantics::SkillSemanticCatalog;
use oozems_skill_semantics::SkillSemanticError;
use oozems_skill_semantics::SkillValueTransform;
use oozems_skill_semantics::validate_archive;
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
    semantics: SkillSemanticCatalog,
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
    #[error(transparent)]
    Semantics(#[from] SkillSemanticError),
    #[error("skill WZ data is invalid: {message}")]
    Invalid { message: String },
    #[error("internal skill content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

impl SkillContent {
    pub fn open_optional(
        directory: &Path,
        sounds: Option<Arc<SoundContent>>,
        semantics: SkillSemanticCatalog,
    ) -> Result<Option<Self>, SkillContentError> {
        let skill_path = directory.join(SKILL_ARCHIVE);
        if !skill_path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: skill_path.clone(),
                source,
            })?
        {
            if !semantics.is_empty() {
                return invalid(format!(
                    "{SKILL_ARCHIVE} is absent but skill semantic mappings are configured"
                ));
            }
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
        validate_archive(&semantics, &skill_archive_facts(&skills, &semantics)?)?;

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
            semantics,
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
            let eligible = crate::jobs::is_job_ancestor(indexed.job_id, job_id)
                || is_beginner_family_skill(learned.skill_id);
            if !eligible {
                return invalid(format!(
                    "learned skill {} belongs to unrelated job {} instead of job {job_id}",
                    learned.skill_id, indexed.job_id
                ));
            }
            let cached = self.cached_definition(learned.skill_id)?;
            authoritative.insert(
                learned.skill_id,
                AuthoritativeSkillDefinition {
                    definition: cached.definition.clone(),
                    invisible: indexed.invisible,
                },
            );
            let unlocked = learned.level > 0 || learned.master_level > 0;
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

fn skill_archive_facts(
    skills: &HashMap<u32, IndexedSkill>,
    semantics: &SkillSemanticCatalog,
) -> Result<SkillArchiveFacts, SkillContentError> {
    let mut facts = SkillArchiveFacts::default();
    for skill_id in skills.keys().copied() {
        facts.add_skill(skill_id);
    }
    let mut configured = BTreeMap::<u32, BTreeSet<OverloadedSkillProperty>>::new();
    for (skill_id, property) in semantics.configured_properties() {
        configured.entry(skill_id).or_default().insert(property);
    }
    for (skill_id, properties) in configured {
        let Some(skill) = skills.get(&skill_id) else {
            continue;
        };
        let Some(levels) = wz::child(&skill.node, "level")? else {
            continue;
        };
        let levels = wz::children(&levels)?;
        facts.set_level_count(skill_id, levels.len());
        for level in levels {
            for property in &properties {
                let Some(property_node) = wz::child(&level, property.name())? else {
                    continue;
                };
                let semantic = semantics
                    .property_semantic(skill_id, SkillPropertyScope::Level, property.name())
                    .expect("configured property has a semantic mapping");
                if let SkillValueTransform::Numeric { .. } = semantic.transform() {
                    let value = read_skill_value(&property_node)?;
                    let Some(value) = value
                        .as_ref()
                        .and_then(|value| normalize_skill_value(semantic.transform(), value))
                    else {
                        return invalid(format!(
                            "skill {skill_id} property {} has a nonnumeric mapped value",
                            property.name()
                        ));
                    };
                    let Some(number) = skill_value_number(&value) else {
                        return invalid(format!(
                            "skill {skill_id} property {} has a nonnumeric mapped value",
                            property.name()
                        ));
                    };
                    if semantic
                        .normalized_stats()
                        .iter()
                        .any(|stat| !stat.accepts_number(number))
                    {
                        return invalid(format!(
                            "skill {skill_id} property {} normalizes outside its stat range",
                            property.name()
                        ));
                    }
                }
                facts.add_level_property(skill_id, *property);
            }
        }
    }
    Ok(facts)
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
        .filter(|(_, skill)| crate::jobs::is_job_ancestor(skill.job_id, job_id) && !skill.invisible)
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
    let levels = build_levels(&content.semantics, skill_id, skill, text.as_ref())?;
    let common_properties = build_properties(wz::child(skill, "common")?.as_ref())?;
    let common_stats = stats_from_properties(
        &content.semantics,
        skill_id,
        SkillPropertyScope::Common,
        &common_properties,
    );
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
            activation: proto_skill_activation(content.semantics.skill_activation(skill_id)) as i32,
        },
        asset,
    ))
}

fn build_levels(
    semantics: &SkillSemanticCatalog,
    skill_id: u32,
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
                stats: Some(stats_from_properties(
                    semantics,
                    skill_id,
                    SkillPropertyScope::Level,
                    &properties,
                )),
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

fn stats_from_properties(
    semantics: &SkillSemanticCatalog,
    skill_id: u32,
    scope: SkillPropertyScope,
    properties: &[SkillProperty],
) -> SkillStats {
    let mut stats = SkillStats {
        x: property_value(properties, "x"),
        y: property_value(properties, "y"),
        z: property_value(properties, "z"),
        ..SkillStats::default()
    };

    for overloaded in [false, true] {
        for property in properties {
            if matches!(property.name.as_str(), "x" | "y" | "z") != overloaded {
                continue;
            }
            let Some(semantic) = semantics.property_semantic(skill_id, scope, &property.name)
            else {
                continue;
            };
            let Some(value) = property
                .value
                .as_ref()
                .and_then(|value| normalize_skill_value(semantic.transform(), value))
            else {
                continue;
            };
            for normalized_stat in semantic.normalized_stats() {
                set_normalized_stat(&mut stats, *normalized_stat, value.clone());
            }
        }
    }
    stats
}

fn normalize_skill_value(
    transform: SkillValueTransform,
    value: &SkillValue,
) -> Option<SkillValue> {
    match (transform, value.value.as_ref()?) {
        (SkillValueTransform::Preserve, _) => Some(value.clone()),
        (SkillValueTransform::Numeric { offset }, skill_value::Value::Integer(value)) => {
            value.checked_add(offset).map(|value| SkillValue {
                value: Some(skill_value::Value::Integer(value)),
            })
        }
        (SkillValueTransform::Numeric { offset }, skill_value::Value::Decimal(value)) => {
            let value = *value + offset as f64;
            value.is_finite().then_some(SkillValue {
                value: Some(skill_value::Value::Decimal(value)),
            })
        }
        (SkillValueTransform::Numeric { offset }, skill_value::Value::Text(value)) => {
            if let Ok(value) = value.trim().parse::<i64>() {
                return value.checked_add(offset).map(|value| SkillValue {
                    value: Some(skill_value::Value::Integer(value)),
                });
            }
            let value = value.trim().parse::<f64>().ok()? + offset as f64;
            value.is_finite().then_some(SkillValue {
                value: Some(skill_value::Value::Decimal(value)),
            })
        }
        (SkillValueTransform::Numeric { .. }, skill_value::Value::Vector(_)) => None,
    }
}

fn skill_value_number(value: &SkillValue) -> Option<f64> {
    match value.value.as_ref()? {
        skill_value::Value::Integer(value) => Some(*value as f64),
        skill_value::Value::Decimal(value) => Some(*value),
        skill_value::Value::Text(value) => value.trim().parse().ok(),
        skill_value::Value::Vector(_) => None,
    }
}

fn set_normalized_stat(
    stats: &mut SkillStats,
    normalized_stat: NormalizedSkillStat,
    value: SkillValue,
) {
    let destination = match normalized_stat {
        NormalizedSkillStat::HpCost => &mut stats.hp_cost,
        NormalizedSkillStat::MpCost => &mut stats.mp_cost,
        NormalizedSkillStat::Hp => &mut stats.hp,
        NormalizedSkillStat::Mp => &mut stats.mp,
        NormalizedSkillStat::WeaponAttack => &mut stats.weapon_attack,
        NormalizedSkillStat::MagicAttack => &mut stats.magic_attack,
        NormalizedSkillStat::Accuracy => &mut stats.accuracy,
        NormalizedSkillStat::Avoidability => &mut stats.avoidability,
        NormalizedSkillStat::WeaponDefense => &mut stats.weapon_defense,
        NormalizedSkillStat::MagicDefense => &mut stats.magic_defense,
        NormalizedSkillStat::Speed => &mut stats.speed,
        NormalizedSkillStat::Jump => &mut stats.jump,
        NormalizedSkillStat::Strength => &mut stats.strength,
        NormalizedSkillStat::Damage => &mut stats.damage,
        NormalizedSkillStat::FixedDamage => &mut stats.fixed_damage,
        NormalizedSkillStat::CriticalDamage => &mut stats.critical_damage,
        NormalizedSkillStat::Mastery => &mut stats.mastery,
        NormalizedSkillStat::AttackCount => &mut stats.attack_count,
        NormalizedSkillStat::MobCount => &mut stats.mob_count,
        NormalizedSkillStat::Duration => &mut stats.duration,
        NormalizedSkillStat::Cooldown => &mut stats.cooldown,
        NormalizedSkillStat::Range => &mut stats.range,
        NormalizedSkillStat::SuccessProbability => &mut stats.success_probability,
        NormalizedSkillStat::HpRecoveryPerFiveSeconds => &mut stats.hp_recovery_per_five_seconds,
        NormalizedSkillStat::MaxHpPerLevel => &mut stats.max_hp_per_level,
        NormalizedSkillStat::MaxHpPerAbilityPoint => &mut stats.max_hp_per_ability_point,
        NormalizedSkillStat::MaxMpPerLevel => &mut stats.max_mp_per_level,
        NormalizedSkillStat::MaxMpPerAbilityPoint => &mut stats.max_mp_per_ability_point,
        NormalizedSkillStat::ThrowingStarCapacity => &mut stats.throwing_star_capacity,
        NormalizedSkillStat::BulletCapacity => &mut stats.bullet_capacity,
        NormalizedSkillStat::CriticalChance => &mut stats.critical_chance,
        NormalizedSkillStat::MaxHpConsumptionPercent => &mut stats.max_hp_consumption_percent,
        NormalizedSkillStat::HpToMpConversionPercent => &mut stats.hp_to_mp_conversion_percent,
        NormalizedSkillStat::ComboStatIncrement => &mut stats.combo_stat_increment,
        NormalizedSkillStat::WeaponAttackPerComboThreshold => {
            &mut stats.weapon_attack_per_combo_threshold
        }
        NormalizedSkillStat::DefensePerComboThreshold => &mut stats.defense_per_combo_threshold,
        NormalizedSkillStat::EnemySpeedPenalty => &mut stats.enemy_speed_penalty,
        NormalizedSkillStat::EnemySlowDuration => &mut stats.enemy_slow_duration,
        NormalizedSkillStat::OutgoingDamagePercent => &mut stats.outgoing_damage_percent,
    };
    if destination.is_none() {
        *destination = Some(value);
    }
}

fn proto_skill_activation(activation: Option<SkillActivation>) -> ProtoSkillActivation {
    match activation.unwrap_or(SkillActivation::Active) {
        SkillActivation::Active => ProtoSkillActivation::Active,
        SkillActivation::Passive => ProtoSkillActivation::Passive,
        SkillActivation::Reactive => ProtoSkillActivation::Reactive,
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
    use oozems_proto::v1::SkillValue;
    use oozems_proto::v1::Vec2;
    use oozems_proto::v1::skill_value;
    use oozems_skill_semantics::SkillPropertyScope;
    use oozems_skill_semantics::SkillSemanticCatalog;
    use oozems_skill_semantics::SkillValueTransform;
    use oozems_skill_semantics::validate_archive;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::CachedSkillDefinition;
    use super::IndexedSkill;
    use super::SkillContent;
    use super::build_properties;
    use super::normalize_skill_value;
    use super::skill_archive_facts;
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
        let stats = stats_from_properties(
            &SkillSemanticCatalog::default(),
            1_002,
            SkillPropertyScope::Level,
            &properties,
        );
        assert!(matches!(
            stats.mp_cost.and_then(|value| value.value),
            Some(skill_value::Value::Integer(3))
        ));
        assert!(matches!(
            stats.damage.and_then(|value| value.value),
            Some(skill_value::Value::Text(value)) if value == "10+2*x"
        ));
    }

    #[test]
    fn canonical_stats_take_precedence_while_overloaded_values_remain_raw() {
        let node = property_node("level");
        add_value(&node, "x", WzValue::Int(7));
        add_value(&node, "y", WzValue::Int(9));
        add_value(&node, "acc", WzValue::Int(12));

        let properties = build_properties(Some(&node)).expect("synthetic skill properties");
        let stats = stats_from_properties(
            &test_semantics(),
            40,
            SkillPropertyScope::Level,
            &properties,
        );

        assert_eq!(integer_stat(stats.x.as_ref()), Some(7));
        assert_eq!(integer_stat(stats.y.as_ref()), Some(9));
        assert_eq!(integer_stat(stats.accuracy.as_ref()), Some(12));
        assert_eq!(integer_stat(stats.avoidability.as_ref()), Some(9));
    }

    #[test]
    fn one_overloaded_value_can_normalize_to_multiple_stats() {
        let node = property_node("level");
        add_value(&node, "z", WzValue::Int(4));

        let properties = build_properties(Some(&node)).expect("synthetic skill properties");
        let stats = stats_from_properties(
            &test_semantics(),
            40,
            SkillPropertyScope::Level,
            &properties,
        );

        assert_eq!(integer_stat(stats.z.as_ref()), Some(4));
        assert_eq!(integer_stat(stats.accuracy.as_ref()), Some(3));
        assert_eq!(integer_stat(stats.avoidability.as_ref()), Some(3));
    }

    #[test]
    fn mapped_periodic_recovery_populates_the_typed_stat() {
        let node = property_node("level");
        add_value(&node, "x", WzValue::Int(4));
        let properties = build_properties(Some(&node)).expect("synthetic skill properties");
        let stats = stats_from_properties(
            &periodic_recovery_semantics(),
            40,
            SkillPropertyScope::Level,
            &properties,
        );

        assert_eq!(
            integer_stat(stats.hp_recovery_per_five_seconds.as_ref()),
            Some(4)
        );
    }

    #[test]
    fn unmapped_and_nonnumeric_overloaded_values_are_not_guessed() {
        let node = property_node("level");
        add_value(
            &node,
            "x",
            WzValue::String(WzString::from_str("10+2*x", [0; 4])),
        );
        let properties = build_properties(Some(&node)).expect("synthetic skill properties");

        let unmapped = stats_from_properties(
            &SkillSemanticCatalog::default(),
            40,
            SkillPropertyScope::Level,
            &properties,
        );
        let mapped = stats_from_properties(
            &test_semantics(),
            40,
            SkillPropertyScope::Level,
            &properties,
        );
        let common = stats_from_properties(
            &test_semantics(),
            40,
            SkillPropertyScope::Common,
            &properties,
        );

        assert!(unmapped.x.is_some());
        assert!(unmapped.accuracy.is_none());
        assert!(mapped.x.is_some());
        assert!(mapped.accuracy.is_none());
        assert!(common.x.is_some());
        assert!(common.accuracy.is_none());
    }

    #[test]
    fn configured_semantics_require_skill_wz() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let error = SkillContent::open_optional(directory.path(), None, accuracy_semantics())
            .err()
            .expect("configured mappings without Skill.wz must fail");

        assert!(
            error
                .to_string()
                .contains("Skill.wz is absent but skill semantic mappings are configured")
        );
    }

    #[test]
    fn server_archive_facts_validate_direct_numeric_properties() {
        let semantics = accuracy_semantics();
        let skills = synthetic_indexed_skill(Some(WzValue::Int(7)));
        let facts = skill_archive_facts(&skills, &semantics).expect("archive facts");

        validate_archive(&semantics, &facts).expect("matching direct level property");

        let missing = synthetic_indexed_skill(None);
        let facts = skill_archive_facts(&missing, &semantics).expect("missing property facts");
        assert!(validate_archive(&semantics, &facts).is_err());

        let partial = synthetic_indexed_skill_levels(&[Some(WzValue::Int(7)), None]);
        let facts = skill_archive_facts(&partial, &semantics).expect("partial property facts");
        assert!(validate_archive(&semantics, &facts).is_err());
    }

    #[test]
    fn server_archive_facts_reject_nonnumeric_mapped_properties() {
        let semantics = accuracy_semantics();
        let skills =
            synthetic_indexed_skill(Some(WzValue::String(WzString::from_str("10+2*x", [0; 4]))));

        assert!(skill_archive_facts(&skills, &semantics).is_err());
    }

    #[test]
    fn server_archive_facts_reject_out_of_range_normalized_properties() {
        let semantics = periodic_recovery_semantics();
        let skills = synthetic_indexed_skill(Some(WzValue::Int(-1)));

        assert!(skill_archive_facts(&skills, &semantics).is_err());
    }

    #[test]
    fn numeric_normalization_accepts_scalars_and_rejects_invalid_values() {
        let transform = SkillValueTransform::Numeric { offset: -1 };
        let normalize = |value| {
            normalize_skill_value(transform, &SkillValue { value: Some(value) })
                .and_then(|value| value.value)
        };

        assert_eq!(
            normalize(skill_value::Value::Decimal(12.5)),
            Some(skill_value::Value::Decimal(11.5))
        );
        assert_eq!(
            normalize(skill_value::Value::Text("12".to_owned())),
            Some(skill_value::Value::Integer(11))
        );
        assert_eq!(
            normalize(skill_value::Value::Text("12.5".to_owned())),
            Some(skill_value::Value::Decimal(11.5))
        );
        assert!(normalize(skill_value::Value::Text("NaN".to_owned())).is_none());
        assert!(normalize(skill_value::Value::Vector(Vec2::default())).is_none());
        assert!(
            normalize_skill_value(
                SkillValueTransform::Numeric { offset: 1 },
                &SkillValue {
                    value: Some(skill_value::Value::Integer(i64::MAX)),
                },
            )
            .is_none()
        );
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
            semantics: SkillSemanticCatalog::default(),
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

    fn integer_stat(value: Option<&oozems_proto::v1::SkillValue>) -> Option<i64> {
        match value?.value.as_ref()? {
            skill_value::Value::Integer(value) => Some(*value),
            _ => None,
        }
    }

    fn accuracy_semantics() -> SkillSemanticCatalog {
        oozems_skill_semantics::parse(
            r#"
schema_version = 1

[[level_properties]]
skill_ids = [40]
property = "x"
label = "Accuracy"
normalized_stats = ["accuracy"]
transform = { type = "numeric" }
"#,
        )
        .expect("accuracy semantic mapping")
    }

    fn periodic_recovery_semantics() -> SkillSemanticCatalog {
        oozems_skill_semantics::parse(
            r#"
schema_version = 1

[[level_properties]]
skill_ids = [40]
property = "x"
label = "HP recovery per five-second tick"
normalized_stats = ["hp_recovery_per_five_seconds"]
transform = { type = "numeric" }
"#,
        )
        .expect("periodic recovery semantic mapping")
    }

    fn test_semantics() -> SkillSemanticCatalog {
        oozems_skill_semantics::parse(
            r#"
schema_version = 1

[[level_properties]]
skill_ids = [40]
property = "x"
label = "Accuracy"
normalized_stats = ["accuracy"]
transform = { type = "numeric" }

[[level_properties]]
skill_ids = [40]
property = "y"
label = "Avoidability"
normalized_stats = ["avoidability"]
transform = { type = "numeric" }

[[level_properties]]
skill_ids = [40]
property = "z"
label = "Accuracy and avoidability"
normalized_stats = ["accuracy", "avoidability"]
transform = { type = "numeric", offset = -1 }
"#,
        )
        .expect("test semantic mappings")
    }

    fn synthetic_indexed_skill(value: Option<WzValue>) -> HashMap<u32, IndexedSkill> {
        synthetic_indexed_skill_levels(&[value])
    }

    fn synthetic_indexed_skill_levels(values: &[Option<WzValue>]) -> HashMap<u32, IndexedSkill> {
        let skill = property_node("skill");
        let levels = add_property(&skill, "level");
        for (index, value) in values.iter().enumerate() {
            let level = add_property(&levels, &(index + 1).to_string());
            if let Some(value) = value {
                add_value(&level, "x", value.clone());
            }
        }
        HashMap::from([(
            40,
            IndexedSkill {
                job_id: 0,
                invisible: false,
                node: skill,
            },
        )])
    }

    fn property_node(name: &str) -> wz_reader::WzNodeArc {
        WzNode::from_str(name, WzObjectType::Property(WzSubProperty::Property), None).into_lock()
    }

    fn add_property(
        parent: &wz_reader::WzNodeArc,
        name: &str,
    ) -> wz_reader::WzNodeArc {
        let child = WzNode::from_str(
            name,
            WzObjectType::Property(WzSubProperty::Property),
            Some(parent),
        )
        .into_lock();
        parent
            .write()
            .expect("property lock")
            .children
            .insert(name.into(), child.clone());
        child
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
