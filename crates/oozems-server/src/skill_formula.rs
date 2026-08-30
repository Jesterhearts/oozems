use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use crate::formula_parser::BinaryOperator;
use crate::formula_parser::Expression;
use crate::formula_parser::Parser;

const VARIABLES: &[&str] = &[
    "Accuracy",
    "AccuracyRatio",
    "AdvancedComboDamage",
    "AmpBulletDamage",
    "AttackRate",
    "Avoidability",
    "BasicAttack",
    "BattleshipLevel",
    "CharacterLevel",
    "ChargeLevel",
    "ComboLevel",
    "DamageBeforeDefense",
    "DamageDealt",
    "Dexterity",
    "HealLevel",
    "HitNumber",
    "Intelligence",
    "JobMultiplier",
    "Luck",
    "Magic",
    "MagicDefense",
    "Mastery",
    "Mesos",
    "MonsterHealth",
    "MonsterLevel",
    "MonsterExperience",
    "Orbs",
    "PlayerLevel",
    "PartyBonus",
    "PartyExperiencePortion",
    "PrimaryStat",
    "SecondaryStat",
    "SkillDamage",
    "SkillLevel",
    "SpellAttack",
    "Strength",
    "TargetCount",
    "TargetMultiplier",
    "TotalHits",
    "TotalPartyLevel",
    "WeaponAttack",
    "WeaponDefense",
];

#[derive(Clone)]
pub struct FormulaCatalog {
    source_url: String,
    weapons: FormulaCategory<String>,
    skills: FormulaCategory<u32>,
    summons: FormulaCategory<u32>,
    defenses: FormulaCategory<String>,
    accuracy: FormulaCategory<String>,
    experience: FormulaCategory<String>,
    stats: FormulaCategory<String>,
    recovery: FormulaCategory<String>,
}

#[derive(Clone)]
struct FormulaCategory<Key> {
    profiles: BTreeMap<String, FormulaProfile>,
    selections: BTreeMap<Key, String>,
}

#[derive(Clone, Debug)]
pub struct FormulaProfile {
    path: String,
    name: String,
    properties: BTreeMap<String, Expression<SkillAtom>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormulaDamageRange {
    pub minimum: f64,
    pub maximum: f64,
}

impl FormulaProfile {
    pub fn name(&self) -> &str {
        &self.name
    }
}

type ProfileFiles = BTreeMap<String, BTreeMap<String, FormulaSource>>;
type SelectionFiles = BTreeMap<String, ProfileSelectionFile>;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FormulaSource {
    Text(String),
    Integer(i64),
    Decimal(f64),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormulaFile {
    source_url: String,
    #[serde(default)]
    weapon_profiles: ProfileFiles,
    #[serde(default)]
    weapons: SelectionFiles,
    #[serde(default)]
    skill_profiles: ProfileFiles,
    #[serde(default)]
    skills: SelectionFiles,
    #[serde(default)]
    summon_profiles: ProfileFiles,
    #[serde(default)]
    summon: SelectionFiles,
    #[serde(default)]
    defense_profiles: ProfileFiles,
    #[serde(default)]
    defenses: SelectionFiles,
    #[serde(default)]
    accuracy_profiles: ProfileFiles,
    #[serde(default)]
    accuracy: SelectionFiles,
    #[serde(default)]
    experience_profiles: ProfileFiles,
    #[serde(default)]
    experience: SelectionFiles,
    #[serde(default)]
    stat_profiles: ProfileFiles,
    #[serde(default)]
    stats: SelectionFiles,
    #[serde(default)]
    recovery_profiles: ProfileFiles,
    #[serde(default)]
    recovery: SelectionFiles,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileSelectionFile {
    profile: String,
}

#[derive(Clone, Debug)]
enum SkillAtom {
    Literal(f64),
    Variable(String),
    Function {
        function: Function,
        arguments: Vec<Expression<SkillAtom>>,
    },
}

#[derive(Clone, Copy, Debug)]
enum Function {
    Floor,
    Truncate,
    Minimum,
    Maximum,
}

#[derive(Debug, Error)]
pub enum FormulaConfigError {
    #[error("failed to read formula configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse formula configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("formula configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Error, PartialEq)]
pub enum FormulaEvaluationError {
    #[error("formula profile {profile:?} does not define property {property:?}")]
    MissingProperty { profile: String, property: String },
    #[error("formula {formula:?} is missing variable {variable:?}")]
    MissingVariable { formula: String, variable: String },
    #[error("formula {formula:?} failed: {message}")]
    Arithmetic { formula: String, message: String },
}

impl FormulaCatalog {
    pub fn load(path: &Path) -> Result<Self, FormulaConfigError> {
        let source = fs::read_to_string(path).map_err(|source| FormulaConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let file =
            toml::from_str::<FormulaFile>(&source).map_err(|source| FormulaConfigError::Parse {
                path: path.to_owned(),
                source,
            })?;
        compile_file(file).map_err(|message| FormulaConfigError::Invalid {
            path: path.to_owned(),
            message,
        })
    }

    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    pub fn len(&self) -> usize {
        category_formula_count(&self.weapons)
            + category_formula_count(&self.skills)
            + category_formula_count(&self.summons)
            + category_formula_count(&self.defenses)
            + category_formula_count(&self.accuracy)
            + category_formula_count(&self.experience)
            + category_formula_count(&self.stats)
            + category_formula_count(&self.recovery)
    }

    pub fn profile_count(&self) -> usize {
        self.weapons.profiles.len()
            + self.skills.profiles.len()
            + self.summons.profiles.len()
            + self.defenses.profiles.len()
            + self.accuracy.profiles.len()
            + self.experience.profiles.len()
            + self.stats.profiles.len()
            + self.recovery.profiles.len()
    }

    pub fn mapping_count(&self) -> usize {
        self.weapons.selections.len()
            + self.skills.selections.len()
            + self.summons.selections.len()
            + self.defenses.selections.len()
            + self.accuracy.selections.len()
            + self.experience.selections.len()
            + self.stats.selections.len()
            + self.recovery.selections.len()
    }

    pub fn skill_profile(
        &self,
        skill_id: u32,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.skills, &skill_id)
    }

    pub fn weapon_profile(
        &self,
        identifier: &str,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.weapons, identifier)
    }

    pub fn recovery_profile(
        &self,
        identifier: &str,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.recovery, identifier)
    }

    pub fn defense_profile(
        &self,
        identifier: &str,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.defenses, identifier)
    }

    pub fn accuracy_profile(
        &self,
        identifier: &str,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.accuracy, identifier)
    }

    pub fn stat_profile(
        &self,
        identifier: &str,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.stats, identifier)
    }

    // Summon effects are not applied yet, but the validated route is part of
    // this configuration boundary so the later effect pipeline cannot bypass
    // it.
    #[allow(dead_code)]
    pub fn summon_profile(
        &self,
        skill_id: u32,
    ) -> Option<&FormulaProfile> {
        selected_profile(&self.summons, &skill_id)
    }
}

fn compile_file(file: FormulaFile) -> Result<FormulaCatalog, String> {
    if file.source_url.trim().is_empty() || !file.source_url.is_ascii() {
        return Err("source_url must be a non-empty ASCII URL".to_owned());
    }
    let weapons = compile_identifier_category(
        "weapon_profiles",
        "weapons",
        file.weapon_profiles,
        file.weapons,
    )?;
    let skills =
        compile_numeric_category("skill_profiles", "skills", file.skill_profiles, file.skills)?;
    let summons = compile_numeric_category(
        "summon_profiles",
        "summon",
        file.summon_profiles,
        file.summon,
    )?;
    let defenses = compile_identifier_category(
        "defense_profiles",
        "defenses",
        file.defense_profiles,
        file.defenses,
    )?;
    let accuracy = compile_identifier_category(
        "accuracy_profiles",
        "accuracy",
        file.accuracy_profiles,
        file.accuracy,
    )?;
    let experience = compile_identifier_category(
        "experience_profiles",
        "experience",
        file.experience_profiles,
        file.experience,
    )?;
    let stats =
        compile_identifier_category("stat_profiles", "stats", file.stat_profiles, file.stats)?;
    let recovery = compile_identifier_category(
        "recovery_profiles",
        "recovery",
        file.recovery_profiles,
        file.recovery,
    )?;
    validate_required_properties(
        selected_profile(&weapons, "bare_hands")
            .ok_or_else(|| "weapons.bare_hands must select a profile".to_owned())?,
        &["attack", "minimum", "maximum"],
    )?;
    validate_selected_properties(&recovery, &["hp", "mp"])?;
    Ok(FormulaCatalog {
        source_url: file.source_url,
        weapons,
        skills,
        summons,
        defenses,
        accuracy,
        experience,
        stats,
        recovery,
    })
}

fn validate_selected_properties<Key: Ord>(
    category: &FormulaCategory<Key>,
    properties: &[&str],
) -> Result<(), String> {
    for profile_name in category.selections.values() {
        let profile = category
            .profiles
            .get(profile_name)
            .expect("profile selections are validated during compilation");
        validate_required_properties(profile, properties)?;
    }
    Ok(())
}

fn compile_identifier_category(
    profile_table: &'static str,
    selection_table: &'static str,
    profiles: ProfileFiles,
    selections: SelectionFiles,
) -> Result<FormulaCategory<String>, String> {
    compile_category(
        profile_table,
        selection_table,
        profiles,
        selections,
        |source| {
            validate_identifier("selector", source)?;
            Ok(source.to_owned())
        },
    )
}

fn compile_numeric_category(
    profile_table: &'static str,
    selection_table: &'static str,
    profiles: ProfileFiles,
    selections: SelectionFiles,
) -> Result<FormulaCategory<u32>, String> {
    compile_category(
        profile_table,
        selection_table,
        profiles,
        selections,
        |source| parse_numeric_id(selection_table, source),
    )
}

fn compile_category<Key: Ord>(
    profile_table: &'static str,
    selection_table: &'static str,
    profiles: ProfileFiles,
    selections: SelectionFiles,
    parse_key: impl Fn(&str) -> Result<Key, String>,
) -> Result<FormulaCategory<Key>, String> {
    let profiles = compile_profiles(profile_table, profiles)?;
    let selections = selections
        .into_iter()
        .map(|(source_key, selection)| {
            let key = parse_key(&source_key)?;
            if !profiles.contains_key(&selection.profile) {
                return Err(format!(
                    "{selection_table}.{source_key} selects unknown {profile_table} profile {:?}",
                    selection.profile
                ));
            }
            Ok((key, selection.profile))
        })
        .collect::<Result<_, String>>()?;
    Ok(FormulaCategory {
        profiles,
        selections,
    })
}

fn compile_profiles(
    table: &str,
    profiles: ProfileFiles,
) -> Result<BTreeMap<String, FormulaProfile>, String> {
    profiles
        .into_iter()
        .map(|(name, properties)| {
            validate_identifier("profile", &name)?;
            if properties.is_empty() {
                return Err(format!("{table}.{name} must define at least one formula"));
            }
            let path = format!("{table}.{name}");
            let properties = properties
                .into_iter()
                .map(|(property, source)| {
                    validate_identifier("property", &property)?;
                    let formula = format!("{path}.{property}");
                    let expression = compile_formula_source(&formula, source)?;
                    validate_variables(&expression)
                        .map_err(|error| format!("formula {formula:?} is invalid: {error}"))?;
                    Ok((property, expression))
                })
                .collect::<Result<_, String>>()?;
            Ok((
                name.clone(),
                FormulaProfile {
                    path,
                    name,
                    properties,
                },
            ))
        })
        .collect()
}

fn compile_formula_source(
    formula: &str,
    source: FormulaSource,
) -> Result<Expression<SkillAtom>, String> {
    match source {
        FormulaSource::Text(source) => parse_formula(&source)
            .map_err(|error| format!("formula {formula:?} is invalid: {error}")),
        FormulaSource::Integer(value) => Ok(Expression::Atom(SkillAtom::Literal(value as f64))),
        FormulaSource::Decimal(value) if value.is_finite() => {
            Ok(Expression::Atom(SkillAtom::Literal(value)))
        }
        FormulaSource::Decimal(_) => Err(format!("formula {formula:?} must be finite")),
    }
}

fn validate_required_properties(
    profile: &FormulaProfile,
    properties: &[&str],
) -> Result<(), String> {
    for property in properties {
        if !profile.properties.contains_key(*property) {
            return Err(format!(
                "required formula {}.{property} is missing",
                profile.path
            ));
        }
    }
    Ok(())
}

fn parse_numeric_id(
    table: &str,
    source: &str,
) -> Result<u32, String> {
    let id = source
        .parse::<u32>()
        .map_err(|_| format!("{table} key {source:?} must be a decimal u32 ID"))?;
    if id.to_string() != source {
        return Err(format!(
            "{table} key {source:?} must use the canonical decimal ID {id}"
        ));
    }
    Ok(id)
}

fn validate_identifier(
    kind: &str,
    name: &str,
) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    valid
        .then_some(())
        .ok_or_else(|| format!("{kind} {name:?} must use lowercase ASCII snake case"))
}

fn selected_profile<'a, Key, Lookup>(
    category: &'a FormulaCategory<Key>,
    key: &Lookup,
) -> Option<&'a FormulaProfile>
where
    Key: Ord + std::borrow::Borrow<Lookup>,
    Lookup: Ord + ?Sized,
{
    category
        .selections
        .get(key)
        .and_then(|name| category.profiles.get(name))
}

fn category_formula_count<Key>(category: &FormulaCategory<Key>) -> usize {
    category
        .profiles
        .values()
        .map(|profile| profile.properties.len())
        .sum()
}

pub fn evaluate_profile_property(
    profile: &FormulaProfile,
    property: &str,
    variables: &[(&str, f64)],
) -> Result<f64, FormulaEvaluationError> {
    let expression = profile.properties.get(property).ok_or_else(|| {
        FormulaEvaluationError::MissingProperty {
            profile: profile.path.clone(),
            property: property.to_owned(),
        }
    })?;
    let formula = format!("{}.{property}", profile.path);
    let values = variables.iter().copied().collect::<BTreeMap<_, _>>();
    let value = evaluate_expression(&formula, expression, &values)?;
    if !value.is_finite() {
        return Err(arithmetic_error(&formula, "the result is not finite"));
    }
    Ok(value)
}

pub fn evaluate_damage_profile(
    profile: &FormulaProfile,
    variables: &[(&str, f64)],
) -> Result<FormulaDamageRange, FormulaEvaluationError> {
    Ok(FormulaDamageRange {
        minimum: evaluate_profile_property(profile, "minimum", variables)?,
        maximum: evaluate_profile_property(profile, "maximum", variables)?,
    })
}

fn validate_variables(expression: &Expression<SkillAtom>) -> Result<(), String> {
    match expression {
        Expression::Atom(atom) => validate_atom_variables(atom),
        Expression::Negate(value) => validate_variables(value),
        Expression::Binary { left, right, .. } => {
            validate_variables(left)?;
            validate_variables(right)
        }
    }
}

fn validate_atom_variables(atom: &SkillAtom) -> Result<(), String> {
    match atom {
        SkillAtom::Variable(name) if !VARIABLES.contains(&name.as_str()) => {
            Err(format!("unknown variable {name:?}"))
        }
        SkillAtom::Function { arguments, .. } => arguments.iter().try_for_each(validate_variables),
        SkillAtom::Literal(_) | SkillAtom::Variable(_) => Ok(()),
    }
}

fn evaluate_expression(
    formula: &str,
    expression: &Expression<SkillAtom>,
    variables: &BTreeMap<&str, f64>,
) -> Result<f64, FormulaEvaluationError> {
    let value = match expression {
        Expression::Atom(atom) => evaluate_atom(formula, atom, variables)?,
        Expression::Negate(value) => -evaluate_expression(formula, value, variables)?,
        Expression::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate_expression(formula, left, variables)?;
            let right = evaluate_expression(formula, right, variables)?;
            evaluate_binary(formula, *operator, left, right)?
        }
    };
    if !value.is_finite() {
        return Err(arithmetic_error(
            formula,
            "an intermediate result is not finite",
        ));
    }
    Ok(value)
}

fn evaluate_atom(
    formula: &str,
    atom: &SkillAtom,
    variables: &BTreeMap<&str, f64>,
) -> Result<f64, FormulaEvaluationError> {
    match atom {
        SkillAtom::Literal(value) => Ok(*value),
        SkillAtom::Variable(name) => variables.get(name.as_str()).copied().ok_or_else(|| {
            FormulaEvaluationError::MissingVariable {
                formula: formula.to_owned(),
                variable: name.clone(),
            }
        }),
        SkillAtom::Function {
            function,
            arguments,
        } => {
            let values = arguments
                .iter()
                .map(|argument| evaluate_expression(formula, argument, variables))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(evaluate_function(*function, &values))
        }
    }
}

fn evaluate_binary(
    formula: &str,
    operator: BinaryOperator,
    left: f64,
    right: f64,
) -> Result<f64, FormulaEvaluationError> {
    match operator {
        BinaryOperator::Add => Ok(left + right),
        BinaryOperator::Subtract => Ok(left - right),
        BinaryOperator::Multiply => Ok(left * right),
        BinaryOperator::Divide if right == 0.0 => {
            Err(arithmetic_error(formula, "division by zero"))
        }
        BinaryOperator::Divide => Ok(left / right),
        BinaryOperator::Exponentiate => Ok(left.powf(right)),
    }
}

fn evaluate_function(
    function: Function,
    arguments: &[f64],
) -> f64 {
    match function {
        Function::Floor => arguments[0].floor(),
        Function::Truncate => arguments[0].trunc(),
        Function::Minimum => arguments[0].min(arguments[1]),
        Function::Maximum => arguments[0].max(arguments[1]),
    }
}

fn arithmetic_error(
    formula: &str,
    message: &str,
) -> FormulaEvaluationError {
    FormulaEvaluationError::Arithmetic {
        formula: formula.to_owned(),
        message: message.to_owned(),
    }
}

fn parse_formula(source: &str) -> Result<Expression<SkillAtom>, String> {
    if source.trim().is_empty() || !source.is_ascii() {
        return Err("formulas must contain non-empty ASCII text".to_owned());
    }
    crate::formula_parser::parse(source, parse_atom)
}

fn parse_atom(parser: &mut Parser<'_, SkillAtom>) -> Result<SkillAtom, String> {
    parser.skip_whitespace();
    if parser
        .peek()
        .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        let (start, source) = parser.number()?;
        let value = source
            .parse()
            .map_err(|_| format!("invalid number at byte {start}"))?;
        return Ok(SkillAtom::Literal(value));
    }
    if parser.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
        return parse_identifier(parser);
    }
    parser.error("expected a number, variable, function, or parenthesized expression")
}

fn parse_identifier(parser: &mut Parser<'_, SkillAtom>) -> Result<SkillAtom, String> {
    let name = parser.identifier()?;
    if !parser.consume(b'(') {
        return Ok(SkillAtom::Variable(name));
    }
    let function = match name.as_str() {
        "floor" => Function::Floor,
        "trunc" => Function::Truncate,
        "min" => Function::Minimum,
        "max" => Function::Maximum,
        _ => return parser.error(&format!("unknown function {name:?}")),
    };
    let mut arguments = vec![parser.parse_expression()?];
    while parser.consume(b',') {
        arguments.push(parser.parse_expression()?);
    }
    parser.expect(b')')?;
    let expected = match function {
        Function::Floor | Function::Truncate => 1,
        Function::Minimum | Function::Maximum => 2,
    };
    if arguments.len() != expected {
        return parser.error(&format!(
            "function {name:?} requires {expected} argument(s)"
        ));
    }
    Ok(SkillAtom::Function {
        function,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::FormulaCatalog;
    use super::FormulaConfigError;
    use super::FormulaEvaluationError;
    use super::evaluate_damage_profile;
    use super::evaluate_profile_property;

    #[test]
    fn loads_and_evaluates_profile_properties() {
        let formulas = load(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.standard]
minimum = "(PrimaryStat * 0.9 * Mastery + SecondaryStat) * WeaponAttack / 100"
maximum = "(PrimaryStat + SecondaryStat) * WeaponAttack / 100"
swing_modifier = 4.4

[weapon_profiles.unarmed]
attack = "min(floor((2 * CharacterLevel + 31) / 3), 31)"
minimum = "1"
maximum = "1"

[weapons.sword]
profile = "standard"

[weapons.bare_hands]
profile = "unarmed"
"#,
        );
        let sword = formulas.weapon_profile("sword").expect("sword profile");
        let bare_hands = formulas
            .weapon_profile("bare_hands")
            .expect("bare-hands profile");

        assert_eq!(
            evaluate_profile_property(
                sword,
                "maximum",
                &[
                    ("PrimaryStat", 48.0),
                    ("SecondaryStat", 5.0),
                    ("WeaponAttack", 20.0),
                ],
            )
            .expect("weapon damage"),
            10.6,
        );
        assert_eq!(
            evaluate_profile_property(sword, "swing_modifier", &[]).expect("numeric modifier"),
            4.4,
        );
        assert_eq!(
            evaluate_profile_property(bare_hands, "attack", &[("CharacterLevel", 200.0)],)
                .expect("capped attack"),
            31.0,
        );
    }

    #[test]
    fn skill_formulas_apply_shared_precedence_and_right_associative_exponents() {
        let formulas = load(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.math]
precedence = "2 + SkillLevel * 2 ^ 3 ^ 2 / 256 - 1"
signed = "-2 ^ 2 + 5"

[skills."1"]
profile = "math"
"#,
        );
        let math = formulas.skill_profile(1).expect("math profile");

        assert_eq!(
            evaluate_profile_property(math, "precedence", &[("SkillLevel", 1.0)])
                .expect("precedence formula"),
            3.0,
        );
        assert_eq!(
            evaluate_profile_property(math, "signed", &[]).expect("signed formula"),
            1.0,
        );
    }

    #[test]
    fn rejects_unknown_variables_before_the_server_starts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(
            &path,
            r#"
source_url = "https://example.test"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.bad]
value = "Typo + 1"
"#,
        )
        .expect("write formulas");

        let error = FormulaCatalog::load(&path)
            .err()
            .expect("unknown variable must fail")
            .to_string();

        assert!(error.contains("unknown variable"));
    }

    #[test]
    fn reports_missing_runtime_variables_and_zero_divisors() {
        let formulas = load(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.ratio]
value = "Accuracy / Avoidability"

[skills."1"]
profile = "ratio"
"#,
        );
        let ratio = formulas.skill_profile(1).expect("ratio profile");

        assert_eq!(
            evaluate_profile_property(ratio, "value", &[("Accuracy", 10.0)]),
            Err(FormulaEvaluationError::MissingVariable {
                formula: "skill_profiles.ratio.value".to_owned(),
                variable: "Avoidability".to_owned(),
            }),
        );
        assert!(matches!(
            evaluate_profile_property(ratio, "value", &[("Accuracy", 10.0), ("Avoidability", 0.0)],),
            Err(FormulaEvaluationError::Arithmetic { .. })
        ));
    }

    #[test]
    fn maps_each_entity_category_to_a_reusable_profile() {
        let formulas = load(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.lucky]
minimum = "Luck * 2.5 * WeaponAttack / 100"
maximum = "Luck * 5 * WeaponAttack / 100"

[skills."4001334"]
profile = "lucky"

[summon_profiles.ship]
durability = "CharacterLevel * 200"

[summon."5221006"]
profile = "ship"

[recovery_profiles.natural]
hp = 10
mp = 3

[recovery.base]
profile = "natural"
"#,
        );

        let skill = formulas.skill_profile(4_001_334).expect("skill profile");
        let range = evaluate_damage_profile(skill, &[("Luck", 20.0), ("WeaponAttack", 10.0)])
            .expect("profile damage");
        let summon = formulas.summon_profile(5_221_006).expect("summon profile");
        let recovery = formulas.recovery_profile("base").expect("recovery profile");

        assert_eq!(skill.name(), "lucky");
        assert_eq!(range.minimum, 5.0);
        assert_eq!(range.maximum, 10.0);
        assert_eq!(
            evaluate_profile_property(summon, "durability", &[("CharacterLevel", 120.0)],)
                .expect("summon durability"),
            24_000.0,
        );
        assert_eq!(
            evaluate_profile_property(recovery, "mp", &[]).expect("MP recovery"),
            3.0,
        );
        assert!(formulas.skill_profile(4_001_335).is_none());
    }

    #[test]
    fn rejects_undocumented_summons_selector_alias() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(
            &path,
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[summon_profiles.ship]
durability = "CharacterLevel * 200"

[summons."5221006"]
profile = "ship"
"#,
        )
        .expect("write formulas");

        assert!(matches!(
            FormulaCatalog::load(&path),
            Err(FormulaConfigError::Parse { .. })
        ));
    }

    #[test]
    fn rejects_selected_recovery_profiles_without_both_amounts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(
            &path,
            r#"
source_url = "https://example.test"

[weapon_profiles.unarmed]
attack = 1
minimum = 1
maximum = 1

[weapons.bare_hands]
profile = "unarmed"

[recovery_profiles.incomplete]
hp = 10

[recovery.base]
profile = "incomplete"
"#,
        )
        .expect("write formulas");

        let error = FormulaCatalog::load(&path)
            .err()
            .expect("incomplete recovery must fail")
            .to_string();

        assert!(error.contains("recovery_profiles.incomplete.mp"));
    }

    #[test]
    fn rejects_unknown_profile_references_before_the_server_starts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(
            &path,
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skills."4001334"]
profile = "missing"
"#,
        )
        .expect("write formulas");

        let error = FormulaCatalog::load(&path)
            .err()
            .expect("unknown profile must fail")
            .to_string();

        assert!(error.contains("unknown skill_profiles profile"));
    }

    #[test]
    fn rejects_empty_profiles_before_the_server_starts() {
        let error = load_error(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.empty]
"#,
        );

        assert!(error.contains("must define at least one formula"));
    }

    #[test]
    fn rejects_noncanonical_numeric_selector_keys() {
        let error = load_error(
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "1"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.attack]
minimum = "1"
maximum = "1"

[skills."04001334"]
profile = "attack"
"#,
        );

        assert!(error.contains("canonical decimal ID 4001334"));
    }

    fn load(source: &str) -> FormulaCatalog {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(&path, source).expect("write formulas");
        FormulaCatalog::load(&path).expect("valid formulas")
    }

    fn load_error(source: &str) -> String {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("formulas.toml");
        fs::write(&path, source).expect("write formulas");
        FormulaCatalog::load(&path)
            .err()
            .expect("invalid formulas")
            .to_string()
    }
}
