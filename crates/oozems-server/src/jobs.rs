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
    fn formula_family_taxonomy_boundaries_are_pinned() {
        for (job_id, family) in [
            (299, StatFormulaFamily::Standard),
            (300, StatFormulaFamily::Ranged),
            (499, StatFormulaFamily::Ranged),
            (500, StatFormulaFamily::Standard),
            (509, StatFormulaFamily::Standard),
            (510, StatFormulaFamily::Brawler),
            (519, StatFormulaFamily::Brawler),
            (520, StatFormulaFamily::Gunslinger),
            (529, StatFormulaFamily::Gunslinger),
            (530, StatFormulaFamily::Standard),
        ] {
            assert_eq!(stat_formula_family(job_id), family, "job {job_id}");
        }
    }

    #[test]
    fn magical_attack_taxonomy_boundaries_are_pinned() {
        assert_eq!(skill_attack_type(199), SkillAttackType::Physical);
        assert_eq!(skill_attack_type(200), SkillAttackType::Magical);
        assert_eq!(skill_attack_type(299), SkillAttackType::Magical);
        assert_eq!(skill_attack_type(300), SkillAttackType::Physical);
    }
}
