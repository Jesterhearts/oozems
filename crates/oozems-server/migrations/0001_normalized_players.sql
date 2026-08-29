BEGIN IMMEDIATE;

CREATE TABLE players (
    player_id TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(player_id) BETWEEN 1 AND 32
            AND player_id NOT GLOB '*[^A-Za-z0-9_-]*'
        ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    name TEXT NOT NULL
        CHECK (
            length(name) BETWEEN 3 AND 12
            AND name NOT GLOB '*[^A-Za-z0-9_]*'
        ),
    level INTEGER NOT NULL CHECK (level BETWEEN 1 AND 4294967295),
    map_id INTEGER NOT NULL CHECK (map_id BETWEEN 0 AND 4294967295),
    appearance_gender INTEGER NOT NULL CHECK (appearance_gender IN (1, 2)),
    skin_id INTEGER NOT NULL CHECK (skin_id BETWEEN 0 AND 4294967295),
    face_id INTEGER NOT NULL CHECK (face_id BETWEEN 0 AND 4294967295),
    hair_id INTEGER NOT NULL CHECK (hair_id BETWEEN 0 AND 4294967295),
    job_id INTEGER NOT NULL CHECK (job_id BETWEEN 0 AND 4294967295),
    hp INTEGER NOT NULL CHECK (hp BETWEEN 0 AND 4294967295),
    max_hp INTEGER NOT NULL CHECK (max_hp BETWEEN 1 AND 4294967295),
    mp INTEGER NOT NULL CHECK (mp BETWEEN 0 AND 4294967295),
    max_mp INTEGER NOT NULL CHECK (max_mp BETWEEN 1 AND 4294967295),
    experience INTEGER NOT NULL CHECK (experience >= 0),
    experience_required INTEGER NOT NULL CHECK (experience_required > 0),
    fame INTEGER NOT NULL CHECK (fame BETWEEN -2147483648 AND 2147483647),
    ability_points INTEGER NOT NULL CHECK (ability_points BETWEEN 0 AND 4294967295),
    strength INTEGER NOT NULL CHECK (strength BETWEEN 0 AND 4294967295),
    dexterity INTEGER NOT NULL CHECK (dexterity BETWEEN 0 AND 4294967295),
    intelligence INTEGER NOT NULL CHECK (intelligence BETWEEN 0 AND 4294967295),
    luck INTEGER NOT NULL CHECK (luck BETWEEN 0 AND 4294967295),
    inventory_capacity INTEGER NOT NULL
        CHECK (inventory_capacity BETWEEN 1 AND 4294967295),
    skill_points INTEGER NOT NULL CHECK (skill_points BETWEEN 0 AND 4294967295),
    mesos INTEGER NOT NULL CHECK (mesos >= 0),
    cash_points INTEGER NOT NULL CHECK (cash_points >= 0),
    CHECK (hp <= max_hp),
    CHECK (mp <= max_mp)
) STRICT;

CREATE TABLE inventory_stacks (
    player_id TEXT NOT NULL,
    slot_index INTEGER NOT NULL CHECK (slot_index >= 0),
    item_id INTEGER NOT NULL CHECK (item_id BETWEEN 1 AND 4294967295),
    quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 4294967295),
    expires_at_unix_ms INTEGER NOT NULL CHECK (expires_at_unix_ms >= 0),
    PRIMARY KEY (player_id, slot_index),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE equipped_items (
    player_id TEXT NOT NULL,
    equipment_slot INTEGER NOT NULL CHECK (equipment_slot IN (1, 2, 3, 4)),
    item_id INTEGER NOT NULL CHECK (item_id BETWEEN 1 AND 4294967295),
    expires_at_unix_ms INTEGER NOT NULL CHECK (expires_at_unix_ms >= 0),
    PRIMARY KEY (player_id, equipment_slot),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE learned_skills (
    player_id TEXT NOT NULL,
    skill_id INTEGER NOT NULL CHECK (skill_id BETWEEN 1 AND 4294967295),
    level INTEGER NOT NULL CHECK (level BETWEEN 0 AND 4294967295),
    master_level INTEGER NOT NULL CHECK (master_level BETWEEN 0 AND 4294967295),
    PRIMARY KEY (player_id, skill_id),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE,
    CHECK (level > 0 OR master_level > 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE key_bindings (
    player_id TEXT NOT NULL,
    code TEXT NOT NULL CHECK (length(code) > 0),
    binding_order INTEGER NOT NULL CHECK (binding_order >= 0),
    action INTEGER NOT NULL CHECK (action BETWEEN 0 AND 8),
    skill_id INTEGER NOT NULL CHECK (skill_id BETWEEN 0 AND 4294967295),
    PRIMARY KEY (player_id, code),
    UNIQUE (player_id, binding_order),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE,
    CHECK (
        (action BETWEEN 1 AND 8 AND skill_id = 0)
        OR (action = 0 AND skill_id > 0)
    )
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX key_bindings_unique_action_target
    ON key_bindings (player_id, action)
    WHERE action <> 0;

CREATE UNIQUE INDEX key_bindings_unique_skill_target
    ON key_bindings (player_id, skill_id)
    WHERE skill_id <> 0;

CREATE TABLE player_quests (
    player_id TEXT NOT NULL,
    quest_id INTEGER NOT NULL CHECK (quest_id BETWEEN 1 AND 4294967295),
    status INTEGER NOT NULL CHECK (status IN (0, 1, 2)),
    accepted_at_unix_ms INTEGER NOT NULL CHECK (accepted_at_unix_ms >= 0),
    completed_at_unix_ms INTEGER NOT NULL CHECK (completed_at_unix_ms >= 0),
    dialogue_step INTEGER NOT NULL CHECK (dialogue_step BETWEEN 0 AND 4294967295),
    completion_quiz_passed INTEGER NOT NULL
        CHECK (completion_quiz_passed IN (0, 1)),
    PRIMARY KEY (player_id, quest_id),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE,
    CHECK (status <> 0 OR dialogue_step > 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE quest_mob_progress (
    player_id TEXT NOT NULL,
    quest_id INTEGER NOT NULL,
    mob_id INTEGER NOT NULL CHECK (mob_id BETWEEN 1 AND 4294967295),
    count INTEGER NOT NULL CHECK (count BETWEEN 0 AND 4294967295),
    PRIMARY KEY (player_id, quest_id, mob_id),
    FOREIGN KEY (player_id, quest_id)
        REFERENCES player_quests (player_id, quest_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE quest_records (
    player_id TEXT NOT NULL,
    quest_id INTEGER NOT NULL CHECK (quest_id BETWEEN 1 AND 4294967295),
    PRIMARY KEY (player_id, quest_id),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE quest_record_entries (
    player_id TEXT NOT NULL,
    quest_id INTEGER NOT NULL,
    entry_index INTEGER NOT NULL CHECK (entry_index BETWEEN 0 AND 4294967295),
    value TEXT NOT NULL CHECK (length(CAST(value AS BLOB)) <= 15),
    PRIMARY KEY (player_id, quest_id, entry_index),
    FOREIGN KEY (player_id, quest_id)
        REFERENCES quest_records (player_id, quest_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE monster_book_cards (
    player_id TEXT NOT NULL,
    card_item_id INTEGER NOT NULL CHECK (card_item_id BETWEEN 1 AND 4294967295),
    count INTEGER NOT NULL CHECK (count BETWEEN 1 AND 5),
    PRIMARY KEY (player_id, card_item_id),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

PRAGMA application_id = 1330596421;
PRAGMA user_version = 1;

COMMIT;
