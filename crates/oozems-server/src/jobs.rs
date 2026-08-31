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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowthFamily {
    Beginner,
    Warrior,
    Magician,
    Bowman,
    Thief,
    Pirate,
    Aran,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponFamily {
    Sword,
    Axe,
    BluntWeapon,
    Dagger,
    Wand,
    Staff,
    Spear,
    Polearm,
    Bow,
    Crossbow,
    Claw,
    Knuckle,
    Gun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponType {
    pub family: WeaponFamily,
    pub profile_name: &'static str,
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

pub fn growth_family(job_id: u32) -> GrowthFamily {
    match job_id {
        0 | 1_000 | 2_000 => GrowthFamily::Beginner,
        100..200 | 1_100..1_200 => GrowthFamily::Warrior,
        200..300 | 1_200..1_300 => GrowthFamily::Magician,
        300..400 | 1_300..1_400 => GrowthFamily::Bowman,
        400..500 | 1_400..1_500 => GrowthFamily::Thief,
        500..600 | 1_500..1_600 => GrowthFamily::Pirate,
        2_100..2_200 => GrowthFamily::Aran,
        _ => GrowthFamily::Beginner,
    }
}

pub fn is_beginner_job(job_id: u32) -> bool {
    matches!(job_id, 0 | 1_000 | 2_000)
}

pub fn is_job_ancestor(
    ancestor_job_id: u32,
    mut job_id: u32,
) -> bool {
    loop {
        if ancestor_job_id == job_id {
            return true;
        }
        let Some(parent) = parent_job(job_id) else {
            return false;
        };
        job_id = parent;
    }
}

fn parent_job(job_id: u32) -> Option<u32> {
    match job_id {
        0 | 1_000 | 2_000 => None,
        100..1_000 if !job_id.is_multiple_of(10) => Some(job_id - 1),
        100..1_000 if !job_id.is_multiple_of(100) => Some(job_id - job_id % 100),
        100..1_000 => Some(0),
        1_000..3_000 if !job_id.is_multiple_of(10) => Some(job_id - 1),
        1_000..3_000 if !job_id.is_multiple_of(100) => Some(job_id - job_id % 100),
        1_000..3_000 => Some(job_id / 1_000 * 1_000),
        _ => None,
    }
}

#[cfg(test)]
pub fn weapon_family(item_id: u32) -> Option<WeaponFamily> {
    weapon_type(item_id).map(|weapon| weapon.family)
}

pub fn weapon_type(item_id: u32) -> Option<WeaponType> {
    match item_id / 10_000 {
        130 => weapon(WeaponFamily::Sword, "one_handed_sword"),
        131 => weapon(WeaponFamily::Axe, "one_handed_sword"),
        132 => weapon(WeaponFamily::BluntWeapon, "one_handed_sword"),
        133 | 134 => weapon(WeaponFamily::Dagger, "dagger"),
        137 => weapon(WeaponFamily::Wand, "wand_basic"),
        138 => weapon(WeaponFamily::Staff, "staff_basic"),
        140 => weapon(WeaponFamily::Sword, "two_handed_sword"),
        141 => weapon(WeaponFamily::Axe, "two_handed_sword"),
        142 => weapon(WeaponFamily::BluntWeapon, "two_handed_sword"),
        143 => weapon(WeaponFamily::Spear, "spear"),
        144 => weapon(WeaponFamily::Polearm, "polearm"),
        145 => weapon(WeaponFamily::Bow, "bow"),
        146 => weapon(WeaponFamily::Crossbow, "crossbow"),
        147 => weapon(WeaponFamily::Claw, "claw"),
        148 => weapon(WeaponFamily::Knuckle, "knuckle"),
        149 => weapon(WeaponFamily::Gun, "gun"),
        _ => None,
    }
}

const fn weapon(
    family: WeaponFamily,
    profile_name: &'static str,
) -> Option<WeaponType> {
    Some(WeaponType {
        family,
        profile_name,
    })
}

impl WeaponFamily {
    pub const COUNT: usize = 13;

    pub const fn index(self) -> usize {
        self as usize
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

    #[test]
    fn growth_taxonomy_covers_v83_job_families() {
        for (job_id, family) in [
            (0, GrowthFamily::Beginner),
            (112, GrowthFamily::Warrior),
            (232, GrowthFamily::Magician),
            (322, GrowthFamily::Bowman),
            (422, GrowthFamily::Thief),
            (522, GrowthFamily::Pirate),
            (1_112, GrowthFamily::Warrior),
            (1_212, GrowthFamily::Magician),
            (1_312, GrowthFamily::Bowman),
            (1_412, GrowthFamily::Thief),
            (1_512, GrowthFamily::Pirate),
            (2_112, GrowthFamily::Aran),
        ] {
            assert_eq!(growth_family(job_id), family, "job {job_id}");
        }
    }

    #[test]
    fn item_prefix_selects_the_weapon_family() {
        assert_eq!(weapon_family(1_302_000), Some(WeaponFamily::Sword));
        assert_eq!(weapon_family(1_442_000), Some(WeaponFamily::Polearm));
        assert_eq!(weapon_family(1_492_000), Some(WeaponFamily::Gun));
        assert_eq!(weapon_family(2_070_000), None);
        assert_eq!(
            weapon_type(1_402_000).map(|weapon| weapon.profile_name),
            Some("two_handed_sword")
        );
    }

    #[test]
    fn job_ancestry_keeps_prior_advancement_skills() {
        assert!(is_job_ancestor(100, 112));
        assert!(is_job_ancestor(110, 112));
        assert!(is_job_ancestor(1_100, 1_112));
        assert!(is_job_ancestor(2_100, 2_112));
        assert!(!is_job_ancestor(120, 112));
        assert!(!is_job_ancestor(1_100, 2_112));
    }
}
