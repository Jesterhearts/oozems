use std::borrow::Cow;

use oozems_proto::v1::GameGui;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestTrackerEntry;

use crate::game::buffs::TrackedBuffs;
use crate::game_gui::QuestJournalTab;

pub(crate) const QUESTS_PER_PAGE: usize = 13;

pub(crate) fn entries<'a>(
    tab: QuestJournalTab,
    gui: &'a GameGui,
    player: &'a PlayerState,
    active_buffs: &'a TrackedBuffs,
) -> Cow<'a, [QuestTrackerEntry]> {
    match tab {
        QuestJournalTab::Available => Cow::Borrowed(
            gui.quest_journal
                .as_ref()
                .map_or(&[], |journal| journal.available_quests.as_slice()),
        ),
        QuestJournalTab::InProgress => Cow::Owned(crate::quest_tracker::active_entries(
            gui,
            player,
            active_buffs,
        )),
        QuestJournalTab::Completed => Cow::Borrowed(
            gui.quest_journal
                .as_ref()
                .map_or(&[], |journal| journal.completed_quests.as_slice()),
        ),
    }
}

pub(crate) fn entry_count(
    tab: QuestJournalTab,
    gui: &GameGui,
) -> usize {
    match tab {
        QuestJournalTab::Available => gui
            .quest_journal
            .as_ref()
            .map_or(0, |journal| journal.available_quests.len()),
        QuestJournalTab::InProgress => gui.quest_tracker.len(),
        QuestJournalTab::Completed => gui
            .quest_journal
            .as_ref()
            .map_or(0, |journal| journal.completed_quests.len()),
    }
}

pub(crate) fn page_count(entry_count: usize) -> usize {
    entry_count.max(1).div_ceil(QUESTS_PER_PAGE)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::QuestJournal;
    use oozems_proto::v1::QuestTrackerEntry;

    use super::entry_count;
    use super::page_count;
    use crate::game_gui::QuestJournalTab;

    #[test]
    fn journal_sections_and_pages_follow_the_projected_entries() {
        let gui = GameGui {
            quest_tracker: vec![QuestTrackerEntry::default(); 2],
            quest_journal: Some(QuestJournal {
                available_quests: vec![QuestTrackerEntry::default(); 14],
                completed_quests: vec![QuestTrackerEntry::default(); 3],
            }),
            ..GameGui::default()
        };

        assert_eq!(entry_count(QuestJournalTab::Available, &gui), 14);
        assert_eq!(entry_count(QuestJournalTab::InProgress, &gui), 2);
        assert_eq!(entry_count(QuestJournalTab::Completed, &gui), 3);
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(13), 1);
        assert_eq!(page_count(14), 2);
    }
}
