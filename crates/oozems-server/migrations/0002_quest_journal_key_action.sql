BEGIN IMMEDIATE;

ALTER TABLE key_bindings RENAME TO key_bindings_v1;

CREATE TABLE key_bindings (
    player_id TEXT NOT NULL,
    code TEXT NOT NULL CHECK (length(code) > 0),
    binding_order INTEGER NOT NULL CHECK (binding_order >= 0),
    action INTEGER NOT NULL CHECK (action BETWEEN 0 AND 9),
    skill_id INTEGER NOT NULL CHECK (skill_id BETWEEN 0 AND 4294967295),
    PRIMARY KEY (player_id, code),
    UNIQUE (player_id, binding_order),
    FOREIGN KEY (player_id) REFERENCES players (player_id) ON DELETE CASCADE,
    CHECK (
        (action BETWEEN 1 AND 9 AND skill_id = 0)
        OR (action = 0 AND skill_id > 0)
    )
) STRICT, WITHOUT ROWID;

INSERT INTO key_bindings (
    player_id,
    code,
    binding_order,
    action,
    skill_id
)
SELECT
    player_id,
    code,
    binding_order,
    action,
    skill_id
FROM key_bindings_v1;

DROP TABLE key_bindings_v1;

CREATE UNIQUE INDEX key_bindings_unique_action_target
    ON key_bindings (player_id, action)
    WHERE action <> 0;

CREATE UNIQUE INDEX key_bindings_unique_skill_target
    ON key_bindings (player_id, skill_id)
    WHERE skill_id <> 0;

INSERT INTO key_bindings (player_id, code, binding_order, action, skill_id)
SELECT
    players.player_id,
    'KeyQ',
    COALESCE(MAX(key_bindings.binding_order), -1) + 1,
    9,
    0
FROM players
LEFT JOIN key_bindings ON key_bindings.player_id = players.player_id
WHERE NOT EXISTS (
    SELECT 1
    FROM key_bindings AS assigned
    WHERE assigned.player_id = players.player_id AND assigned.code = 'KeyQ'
)
GROUP BY players.player_id;

PRAGMA user_version = 2;

COMMIT;
