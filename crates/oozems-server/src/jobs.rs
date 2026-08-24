#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatFormulaFamily {
    Standard,
    Ranged,
    Brawler,
    Gunslinger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillAttackType {
    Physical,
    Magical,
}

const MAGE_JOBS: std::ops::Range<u32> = 200..300;
const RANGED_JOBS: std::ops::Range<u32> = 300..500;
const BRAWLER_JOBS: std::ops::Range<u32> = 510..520;
const GUNSLINGER_JOBS: std::ops::Range<u32> = 520..530;

pub fn stat_formula_family(job_id: u32) -> StatFormulaFamily {
    if BRAWLER_JOBS.contains(&job_id) {
        StatFormulaFamily::Brawler
    } else if GUNSLINGER_JOBS.contains(&job_id) {
        StatFormulaFamily::Gunslinger
    } else if RANGED_JOBS.contains(&job_id) {
        StatFormulaFamily::Ranged
    } else {
        StatFormulaFamily::Standard
    }
}

pub fn skill_attack_type(job_id: u32) -> SkillAttackType {
    if MAGE_JOBS.contains(&job_id) {
        SkillAttackType::Magical
    } else {
        SkillAttackType::Physical
    }
}

impl StatFormulaFamily {
    pub fn profile_name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Ranged => "ranged",
            Self::Brawler => "brawler",
            Self::Gunslinger => "gunslinger",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoritative_job_ids_select_named_formula_families() {
        assert_eq!(stat_formula_family(0), StatFormulaFamily::Standard);
        assert_eq!(stat_formula_family(212), StatFormulaFamily::Standard);
        assert_eq!(stat_formula_family(311), StatFormulaFamily::Ranged);
        assert_eq!(stat_formula_family(411), StatFormulaFamily::Ranged);
        assert_eq!(stat_formula_family(511), StatFormulaFamily::Brawler);
        assert_eq!(stat_formula_family(521), StatFormulaFamily::Gunslinger);
    }

    #[test]
    fn only_mage_family_skills_select_magical_attack() {
        assert_eq!(skill_attack_type(212), SkillAttackType::Magical);
        assert_eq!(skill_attack_type(112), SkillAttackType::Physical);
        assert_eq!(skill_attack_type(522), SkillAttackType::Physical);
    }
}
