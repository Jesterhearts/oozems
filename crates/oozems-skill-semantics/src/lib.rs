#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillPropertyScope {
    Common,
    Level,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillActivation {
    Active,
    Passive,
    Reactive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverloadedSkillProperty {
    Damage,
    X,
    Y,
    Z,
}

impl OverloadedSkillProperty {
    pub fn name(self) -> &'static str {
        match self {
            Self::Damage => "damage",
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "damage" => Some(Self::Damage),
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "z" => Some(Self::Z),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedSkillStat {
    HpCost,
    MpCost,
    Hp,
    Mp,
    WeaponAttack,
    MagicAttack,
    Accuracy,
    Avoidability,
    WeaponDefense,
    MagicDefense,
    Speed,
    Jump,
    Strength,
    Damage,
    FixedDamage,
    CriticalDamage,
    Mastery,
    AttackCount,
    MobCount,
    Duration,
    Cooldown,
    Range,
    SuccessProbability,
    HpRecoveryPerFiveSeconds,
    MaxHpPerLevel,
    MaxHpPerAbilityPoint,
    MaxMpPerLevel,
    MaxMpPerAbilityPoint,
    ThrowingStarCapacity,
    BulletCapacity,
    CriticalChance,
    MaxHpConsumptionPercent,
    HpToMpConversionPercent,
    ComboStatIncrement,
    WeaponAttackPerComboThreshold,
    DefensePerComboThreshold,
    EnemySpeedPenalty,
    EnemySlowDuration,
    OutgoingDamagePercent,
}

impl NormalizedSkillStat {
    pub fn accepts_number(
        self,
        value: f64,
    ) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::WeaponAttack
            | Self::MagicAttack
            | Self::Accuracy
            | Self::Avoidability
            | Self::WeaponDefense
            | Self::MagicDefense
            | Self::Speed
            | Self::Jump
            | Self::Strength
            | Self::EnemySpeedPenalty => {
                value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)
            }
            Self::Duration => value == -1.0 || (value >= 0.0 && value <= f64::from(u32::MAX)),
            Self::HpCost
            | Self::MpCost
            | Self::Hp
            | Self::Mp
            | Self::Damage
            | Self::FixedDamage
            | Self::CriticalDamage
            | Self::Mastery
            | Self::AttackCount
            | Self::MobCount
            | Self::Cooldown
            | Self::Range
            | Self::SuccessProbability
            | Self::HpRecoveryPerFiveSeconds
            | Self::MaxHpPerLevel
            | Self::MaxHpPerAbilityPoint
            | Self::MaxMpPerLevel
            | Self::MaxMpPerAbilityPoint
            | Self::ThrowingStarCapacity
            | Self::BulletCapacity
            | Self::ComboStatIncrement
            | Self::WeaponAttackPerComboThreshold
            | Self::DefensePerComboThreshold
            | Self::EnemySlowDuration
            | Self::OutgoingDamagePercent => value >= 0.0 && value <= f64::from(u32::MAX),
            Self::CriticalChance
            | Self::MaxHpConsumptionPercent
            | Self::HpToMpConversionPercent => (0.0..=100.0).contains(&value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillValueTransform {
    Preserve,
    Numeric { offset: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillPropertySemantic<'a> {
    label: &'a str,
    normalized_stats: &'a [NormalizedSkillStat],
    transform: SkillValueTransform,
}

impl<'a> SkillPropertySemantic<'a> {
    pub fn label(self) -> &'a str {
        self.label
    }

    pub fn normalized_stats(self) -> &'a [NormalizedSkillStat] {
        self.normalized_stats
    }

    pub fn transform(self) -> SkillValueTransform {
        self.transform
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkillSemanticCatalog {
    level_properties: BTreeMap<(u32, OverloadedSkillProperty), SkillSemanticDefinition>,
    activations: BTreeMap<u32, SkillActivation>,
}

#[derive(Clone, Debug)]
struct SkillSemanticDefinition {
    label: String,
    normalized_stats: Vec<NormalizedSkillStat>,
    transform: SkillValueTransform,
}

impl SkillSemanticCatalog {
    pub fn len(&self) -> usize {
        self.level_properties.len()
    }

    pub fn is_empty(&self) -> bool {
        self.level_properties.is_empty()
    }

    pub fn configured_properties(
        &self
    ) -> impl Iterator<Item = (u32, OverloadedSkillProperty)> + '_ {
        self.level_properties.keys().copied()
    }

    pub fn configured_activations(&self) -> impl Iterator<Item = (u32, SkillActivation)> + '_ {
        self.activations
            .iter()
            .map(|(skill_id, activation)| (*skill_id, *activation))
    }

    pub fn skill_activation(
        &self,
        skill_id: u32,
    ) -> Option<SkillActivation> {
        self.activations.get(&skill_id).copied()
    }

    pub fn property_semantic(
        &self,
        skill_id: u32,
        scope: SkillPropertyScope,
        property_name: &str,
    ) -> Option<SkillPropertySemantic<'_>> {
        if scope == SkillPropertyScope::Level
            && let Some(property) = OverloadedSkillProperty::from_name(property_name)
            && let Some(definition) = self.level_properties.get(&(skill_id, property))
        {
            return Some(SkillPropertySemantic {
                label: &definition.label,
                normalized_stats: &definition.normalized_stats,
                transform: definition.transform,
            });
        }
        conventional_property_semantic(property_name)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkillArchiveFacts {
    skill_ids: BTreeSet<u32>,
    level_counts: BTreeMap<u32, usize>,
    direct_level_property_counts: BTreeMap<(u32, OverloadedSkillProperty), usize>,
}

impl SkillArchiveFacts {
    pub fn add_skill(
        &mut self,
        skill_id: u32,
    ) {
        self.skill_ids.insert(skill_id);
    }

    pub fn add_level_property(
        &mut self,
        skill_id: u32,
        property: OverloadedSkillProperty,
    ) {
        *self
            .direct_level_property_counts
            .entry((skill_id, property))
            .or_default() += 1;
    }

    pub fn set_level_count(
        &mut self,
        skill_id: u32,
        level_count: usize,
    ) {
        self.level_counts.insert(skill_id, level_count);
    }
}

#[derive(Debug, Error)]
pub enum SkillSemanticError {
    #[error("failed to read skill semantic mappings from {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse skill semantic mappings from {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("skill semantic mappings are invalid: {message}")]
    Invalid { message: String },
    #[error("skill semantic mappings do not match Skill.wz: {message}")]
    ArchiveMismatch { message: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillSemanticFile {
    schema_version: u32,
    #[serde(default)]
    skills: Vec<SkillFile>,
    #[serde(default)]
    level_properties: Vec<LevelPropertyFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillFile {
    skill_ids: Vec<u32>,
    activation: SkillActivation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LevelPropertyFile {
    skill_ids: Vec<u32>,
    property: OverloadedPropertyFile,
    label: String,
    #[serde(default)]
    normalized_stats: Vec<NormalizedSkillStat>,
    transform: TransformFile,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OverloadedPropertyFile {
    Damage,
    X,
    Y,
    Z,
}

impl From<OverloadedPropertyFile> for OverloadedSkillProperty {
    fn from(value: OverloadedPropertyFile) -> Self {
        match value {
            OverloadedPropertyFile::Damage => Self::Damage,
            OverloadedPropertyFile::X => Self::X,
            OverloadedPropertyFile::Y => Self::Y,
            OverloadedPropertyFile::Z => Self::Z,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TransformFile {
    Preserve,
    Numeric {
        #[serde(default)]
        offset: i64,
    },
}

pub fn load_optional(path: &Path) -> Result<SkillSemanticCatalog, SkillSemanticError> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(SkillSemanticCatalog::default());
        }
        Err(source) => {
            return Err(SkillSemanticError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let file = toml::from_str(&source).map_err(|source| SkillSemanticError::Parse {
        path: path.to_owned(),
        source,
    })?;
    compile(file)
}

pub fn parse(source: &str) -> Result<SkillSemanticCatalog, SkillSemanticError> {
    let file = toml::from_str(source).map_err(|source| SkillSemanticError::Parse {
        path: PathBuf::from("<memory>"),
        source,
    })?;
    compile(file)
}

pub fn validate_archive(
    catalog: &SkillSemanticCatalog,
    facts: &SkillArchiveFacts,
) -> Result<(), SkillSemanticError> {
    for (skill_id, _) in catalog.configured_activations() {
        if !facts.skill_ids.contains(&skill_id) {
            return Err(SkillSemanticError::ArchiveMismatch {
                message: format!("configured skill {skill_id} does not exist"),
            });
        }
    }
    for (skill_id, property) in catalog.configured_properties() {
        if !facts.skill_ids.contains(&skill_id) {
            return Err(SkillSemanticError::ArchiveMismatch {
                message: format!("configured skill {skill_id} does not exist"),
            });
        }
        let level_count = facts
            .level_counts
            .get(&skill_id)
            .copied()
            .unwrap_or_default();
        let property_count = facts
            .direct_level_property_counts
            .get(&(skill_id, property))
            .copied()
            .unwrap_or_default();
        if level_count == 0 || property_count != level_count {
            return Err(SkillSemanticError::ArchiveMismatch {
                message: format!(
                    "configured property {} is present on {property_count} of {level_count} \
                     direct levels of skill {skill_id}",
                    property.name(),
                ),
            });
        }
    }
    Ok(())
}

fn compile(file: SkillSemanticFile) -> Result<SkillSemanticCatalog, SkillSemanticError> {
    if file.schema_version != SCHEMA_VERSION {
        return invalid(format!(
            "schema_version must be {SCHEMA_VERSION}, not {}",
            file.schema_version
        ));
    }
    let mut activations = BTreeMap::new();
    for (index, rule) in file.skills.into_iter().enumerate() {
        let rule_number = index + 1;
        if rule.skill_ids.is_empty() {
            return invalid(format!("skills entry {rule_number} has no skill_ids"));
        }
        let mut skill_ids = BTreeSet::new();
        for skill_id in rule.skill_ids {
            if skill_id == 0 {
                return invalid(format!("skills entry {rule_number} contains skill ID zero"));
            }
            if !skill_ids.insert(skill_id) {
                return invalid(format!(
                    "skills entry {rule_number} repeats skill {skill_id}"
                ));
            }
        }
        for skill_id in skill_ids {
            if activations.insert(skill_id, rule.activation).is_some() {
                return invalid(format!(
                    "skill {skill_id} activation is configured more than once"
                ));
            }
        }
    }

    let mut level_properties = BTreeMap::new();
    for (index, rule) in file.level_properties.into_iter().enumerate() {
        let rule_number = index + 1;
        if rule.skill_ids.is_empty() {
            return invalid(format!(
                "level_properties entry {rule_number} has no skill_ids"
            ));
        }
        let mut skill_ids = BTreeSet::new();
        for skill_id in rule.skill_ids {
            if skill_id == 0 {
                return invalid(format!(
                    "level_properties entry {rule_number} contains skill ID zero"
                ));
            }
            if !skill_ids.insert(skill_id) {
                return invalid(format!(
                    "level_properties entry {rule_number} repeats skill {skill_id}"
                ));
            }
        }
        if rule.label.is_empty() || rule.label.trim() != rule.label {
            return invalid(format!(
                "level_properties entry {rule_number} label must be nonempty and unpadded"
            ));
        }
        if rule.label.chars().any(char::is_control) {
            return invalid(format!(
                "level_properties entry {rule_number} label contains a control character"
            ));
        }
        let normalized_stat_count = rule.normalized_stats.len();
        let normalized_stats = rule.normalized_stats.into_iter().collect::<BTreeSet<_>>();
        let transform = match rule.transform {
            TransformFile::Preserve if normalized_stats.is_empty() => SkillValueTransform::Preserve,
            TransformFile::Preserve => {
                return invalid(format!(
                    "level_properties entry {rule_number} must use a numeric transform when it \
                     has normalized_stats"
                ));
            }
            TransformFile::Numeric { offset } if !normalized_stats.is_empty() => {
                SkillValueTransform::Numeric { offset }
            }
            TransformFile::Numeric { .. } => {
                return invalid(format!(
                    "level_properties entry {rule_number} numeric transform has no \
                     normalized_stats"
                ));
            }
        };
        if normalized_stats.len() != normalized_stat_count {
            return invalid(format!(
                "level_properties entry {rule_number} repeats a normalized stat"
            ));
        }
        let property: OverloadedSkillProperty = rule.property.into();
        let definition = SkillSemanticDefinition {
            label: rule.label,
            normalized_stats: normalized_stats.into_iter().collect(),
            transform,
        };
        for skill_id in skill_ids {
            if level_properties
                .insert((skill_id, property), definition.clone())
                .is_some()
            {
                return invalid(format!(
                    "skill {skill_id} property {} is configured more than once",
                    property.name()
                ));
            }
        }
    }
    Ok(SkillSemanticCatalog {
        level_properties,
        activations,
    })
}

fn conventional_property_semantic(property_name: &str) -> Option<SkillPropertySemantic<'static>> {
    use NormalizedSkillStat as Stat;

    let (label, normalized_stats) = match property_name {
        "hpCon" => ("HP cost", &[Stat::HpCost][..]),
        "mpCon" => ("MP cost", &[Stat::MpCost][..]),
        "hp" => ("HP", &[Stat::Hp][..]),
        "mp" => ("MP", &[Stat::Mp][..]),
        "pad" => ("Weapon attack", &[Stat::WeaponAttack][..]),
        "mad" => ("Magic attack", &[Stat::MagicAttack][..]),
        "acc" => ("Accuracy", &[Stat::Accuracy][..]),
        "eva" => ("Avoidability", &[Stat::Avoidability][..]),
        "pdd" => ("Weapon defense", &[Stat::WeaponDefense][..]),
        "mdd" => ("Magic defense", &[Stat::MagicDefense][..]),
        "speed" => ("Speed", &[Stat::Speed][..]),
        "jump" => ("Jump", &[Stat::Jump][..]),
        "str" => ("Strength", &[Stat::Strength][..]),
        "damage" => ("Damage", &[Stat::Damage][..]),
        "fixdamage" => ("Fixed damage", &[Stat::FixedDamage][..]),
        "criticalDamage" => ("Critical damage", &[Stat::CriticalDamage][..]),
        "mastery" => ("Mastery", &[Stat::Mastery][..]),
        "attackCount" => ("Attack count", &[Stat::AttackCount][..]),
        "mobCount" => ("Target count", &[Stat::MobCount][..]),
        "time" => ("Duration", &[Stat::Duration][..]),
        "cooltime" => ("Cooldown", &[Stat::Cooldown][..]),
        "range" => ("Range", &[Stat::Range][..]),
        "prop" => ("Success chance", &[Stat::SuccessProbability][..]),
        "hs" => ("Level description selector", &[][..]),
        _ => return None,
    };
    Some(SkillPropertySemantic {
        label,
        normalized_stats,
        transform: SkillValueTransform::Preserve,
    })
}

fn invalid<T>(message: impl Into<String>) -> Result<T, SkillSemanticError> {
    Err(SkillSemanticError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::NormalizedSkillStat;
    use super::OverloadedSkillProperty;
    use super::SkillArchiveFacts;
    use super::SkillPropertyScope;
    use super::SkillValueTransform;
    use super::load_optional;
    use super::parse;
    use super::validate_archive;

    const MAPPINGS: &str = r#"
schema_version = 1

[[level_properties]]
skill_ids = [40, 50]
property = "x"
label = "Accuracy"
normalized_stats = ["accuracy"]
transform = { type = "numeric" }

[[level_properties]]
skill_ids = [40]
property = "z"
label = "Accuracy and avoidability"
normalized_stats = ["accuracy", "avoidability"]
transform = { type = "numeric", offset = -1 }

[[level_properties]]
skill_ids = [40]
property = "y"
label = "Capacity"
transform = { type = "preserve" }
"#;

    #[test]
    fn grouped_archive_mappings_expand_and_keep_transforms() {
        let catalog = parse(MAPPINGS).expect("valid mappings");
        let accuracy = catalog
            .property_semantic(50, SkillPropertyScope::Level, "x")
            .expect("accuracy semantic");
        let combined = catalog
            .property_semantic(40, SkillPropertyScope::Level, "z")
            .expect("combined semantic");

        assert_eq!(catalog.len(), 4);
        assert_eq!(
            accuracy.normalized_stats(),
            &[NormalizedSkillStat::Accuracy]
        );
        assert_eq!(
            combined.normalized_stats(),
            &[
                NormalizedSkillStat::Accuracy,
                NormalizedSkillStat::Avoidability,
            ]
        );
        assert_eq!(
            combined.transform(),
            SkillValueTransform::Numeric { offset: -1 }
        );
        assert!(
            catalog
                .property_semantic(40, SkillPropertyScope::Common, "x")
                .is_none()
        );
    }

    #[test]
    fn conventional_properties_do_not_require_archive_mappings() {
        let catalog = super::SkillSemanticCatalog::default();
        let semantic = catalog
            .property_semantic(999, SkillPropertyScope::Level, "acc")
            .expect("conventional accuracy semantic");

        assert_eq!(semantic.label(), "Accuracy");
        assert_eq!(
            semantic.normalized_stats(),
            &[NormalizedSkillStat::Accuracy]
        );
    }

    #[test]
    fn archive_validation_rejects_stale_skills_and_properties() {
        let catalog = parse(MAPPINGS).expect("valid mappings");
        let mut facts = SkillArchiveFacts::default();
        facts.add_skill(40);
        facts.add_skill(50);
        facts.set_level_count(40, 1);
        facts.set_level_count(50, 2);
        facts.add_level_property(40, OverloadedSkillProperty::X);
        facts.add_level_property(40, OverloadedSkillProperty::Y);
        facts.add_level_property(40, OverloadedSkillProperty::Z);

        assert!(validate_archive(&catalog, &facts).is_err());
        facts.add_level_property(50, OverloadedSkillProperty::X);
        assert!(validate_archive(&catalog, &facts).is_err());
        facts.add_level_property(50, OverloadedSkillProperty::X);
        validate_archive(&catalog, &facts).expect("matching archive facts");
    }

    #[test]
    fn normalized_stats_enforce_their_runtime_numeric_domains() {
        assert!(NormalizedSkillStat::Accuracy.accepts_number(-10.0));
        assert!(NormalizedSkillStat::Duration.accepts_number(-1.0));
        assert!(!NormalizedSkillStat::Duration.accepts_number(-2.0));
        assert!(!NormalizedSkillStat::HpRecoveryPerFiveSeconds.accepts_number(-1.0));
        assert!(!NormalizedSkillStat::Damage.accepts_number(f64::INFINITY));
    }

    #[test]
    fn invalid_or_duplicate_rules_are_rejected() {
        let duplicate = format!(
            "{MAPPINGS}\n[[level_properties]]\nskill_ids = [40]\nproperty = \"x\"\nlabel = \
             \"Other\"\nnormalized_stats = [\"accuracy\"]\ntransform = {{ type = \"numeric\" }}\n"
        );
        assert!(parse(&duplicate).is_err());
        assert!(parse("schema_version = 2").is_err());
        assert!(parse("schema_version = 1\nunknown = true").is_err());
    }

    #[test]
    fn missing_file_is_an_empty_catalog() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let catalog = load_optional(&directory.path().join("missing.toml"))
            .expect("optional missing mapping file");

        assert!(catalog.is_empty());
    }

    #[test]
    fn bundled_v83_example_is_valid_configuration() {
        let catalog = parse(include_str!("../../../examples/v83/skill-semantics.toml"))
            .expect("v83 semantic mappings");

        assert!(!catalog.is_empty());
    }
}
