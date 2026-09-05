# Milestones for classic MapleStory parity

## Target and completion standard

This roadmap targets the player-visible behavior and content of GMS v83, before
Big Bang, implemented through Oozems's own client and server. GMS v83 is the
working baseline from the initial parity assessment.

Parity includes character progression, combat, items, world interactions,
multiplayer, and the client behavior needed to use those systems. Loading an
asset, parsing a definition, or recognizing a script name establishes only that
the data is accessible. Completion requires verified behavior.

Personal configuration may change rates, starting resources, or convenience
rules. Keep a reference configuration for parity checks and record each
intentional difference. An excluded feature narrows the parity claim; it does
not count as implemented.

Compatibility with the original client protocol, public-server hosting, payment
processing, and custom features from other private servers are outside this
roadmap. Shared play still requires character ownership checks and correct
handling of concurrent actions.

## Starting point

The initial source assessment found a useful foundation:

- WZ content loading, map rendering, character composition, audio, and native
  interface artwork.
- Authoritative movement, mob simulation, basic attacks, and selected skill
  effects.
- Inventory, item use, drops, EXP, AP, SP, quests, and SQLite persistence.
- Transaction handling for player changes and related simulation effects.

The main boundaries to remove are:

- Job classification and ancestry exist, but the assessed gameplay path does not
  perform job advancement. See the
  [job model](crates/oozems-server/src/jobs.rs).
- The [attack resolver](crates/oozems-server/src/mobs.rs) selects one target and
  resolves one damage result per attack.
- The [protocol](crates/oozems-proto/proto/oozems.proto) defines four equipment
  slots and does not model individual rolled or scrolled equipment stats.
- The [quest script model](crates/oozems-server/src/quest_scripts.rs) supports
  restricted conditions and resource actions. It needs additional world-event
  and progression capabilities.
- The [browser entry point](crates/oozems-client/src/lib.rs) uses a fixed player
  identity. The assessed map protocol does not replicate remote characters.

These are starting observations, not permanent status reports.

## Milestone sequence

All milestones are open until their exit criteria have recorded evidence.
Existing implementations can satisfy individual criteria after verification.
Dependencies below are completion gates; research and content preparation can
start before the supporting runtime is complete.

| ID  | Outcome                                         | Depends on | Status |
| --- | ----------------------------------------------- | ---------- | ------ |
| M1  | Complete durable item and equipment model       | None       | Open   |
| M2  | World-event and job-advancement primitives      | None       | Open   |
| M3  | One Explorer branch playable through second job | M1, M2     | Open   |
| M4  | Complete class and combat rules                 | M3         | Open   |
| M5  | Distinct players sharing a visible world        | M1, M3     | Open   |
| M6  | Party play and a working player economy         | M4, M5     | Open   |
| M7  | One complete party quest and boss encounter     | M2, M6     | Open   |
| M8  | Remaining classic social and item systems       | M6, M7     | Open   |
| M9  | Complete progression and world content          | M4, M7, M8 | Open   |
| M10 | Complete client behavior and presentation       | M8, M9     | Open   |
| M11 | Verified parity and reliable personal operation | M10        | Open   |

M3 is the first useful solo progression release. M6 is the first complete
small-group gameplay loop. M11 is the parity release. An earlier project
version, including version 0.1, may ship before parity is complete.

## M1: Complete the durable item and equipment model

**Outcome:** Items retain their identity and properties through every supported
inventory operation and server restart.

### Deliverables

- Separate item definitions from owned item instances. Give non-stackable
  equipment stable identities and persisted stats, upgrade slots, upgrade
  history or counters required by the rules, flags, and expiration.
- Implement all baseline equipment slots, slot conflicts, stat requirements, and
  contributions to derived character stats. Include cosmetic overlays and their
  composition rules where the baseline requires them.
- Implement inventory categories, capacities, slot ordering, stack rules,
  ammunition quantities, and supported expansion behavior.
- Implement equipment generation and scrolling outcomes, including failure,
  destruction, and item restrictions. Model random inputs explicitly.
- Migrate existing saved characters and items without discarding their state.
  Update protocol projections, equipment rendering, and item tooltips.

### Exit criteria

- [ ] Two copies of one equipment type can have different stats and remain
      distinct after equip, unequip, drop, pickup, save, and reload.
- [ ] Every baseline slot renders correctly and contributes the expected stats.
- [ ] Invalid equipment combinations and requirements leave state unchanged.
- [ ] Scroll outcomes consume the correct resources and preserve invariants.
- [ ] Full inventories, expired items, interrupted operations, and migration
      fixtures produce the expected saved state without loss or duplication.

## M2: Add world-event and job-advancement primitives

**Outcome:** Authored interactions can change progression and world state
through explicit, validated action plans.

### Deliverables

- Extend the typed interaction model with dialogue choices, conditional
  branches, job changes, skill grants, map transfers, spawning, and timers.
- Add event inputs for NPC interaction, portal use, map entry and exit, mob
  death, reactor transitions, and relevant item use or placement.
- Define event state, cancellation, repeated delivery, and persistence rules.
  Keep time and random inputs explicit and bound program execution.
- Separate map definitions from running map instances. Represent which instance
  owns players, mobs, drops, reactors, timers, and event state.
- Implement advancement prerequisites and rewards as a validated progression
  transform shared by authored interactions.

### Exit criteria

- [ ] A character performs one real first-job advancement through the client,
      with correct prerequisites, rewards, and persistence.
- [ ] A scripted portal and a map-entry event each reproduce a selected baseline
      interaction, including their rejection conditions.
- [ ] A timed interaction and an item-triggered reactor handle success,
      cancellation, and repeated inputs correctly.
- [ ] Two instances of the same map keep their mutable state separate.
- [ ] Missing required script behavior is reported explicitly and cannot
      silently grant progression or rewards.

## M3: Complete one Explorer branch through second job

**Outcome:** A fresh character can follow a coherent solo progression route
without database edits or developer commands.

### Deliverables

- Complete Maple Island, travel to Victoria Island, first advancement, and
  second advancement.
- Implement every skill available through that branch's second job. Extend
  combat for the target counts, hit counts, ranges, resource costs, movement,
  and effects required by those skills.
- Supply the route's quests, shops, travel, equipment, consumables, drops, and
  ammunition behavior where applicable.
- Match the reference configuration for EXP, stat growth, AP and SP allocation,
  attack cadence, damage, accuracy, recovery, death, and revival on this route.
- Provide the corresponding controls, shortcuts, animations, and feedback.

### Exit criteria

- [ ] A new character completes the route and both advancements through normal
      gameplay, using the reference configuration.
- [ ] Every skill through the selected second job has a verified scenario.
- [ ] The route supplies its required quest items and progression resources.
- [ ] Combat checks cover multiple targets or hits where relevant, misses,
      insufficient resources, invalid targets, death, and interruption.
- [ ] Reconnecting at each progression boundary preserves the expected state.

## M4: Complete class and combat rules

**Outcome:** The progression rules and full skill set for every baseline class
work through the production gameplay pipeline.

Use controlled scenarios for prerequisites whose world content is not yet
implemented. M9 verifies that all advancement paths and skill acquisitions are
reachable through normal gameplay.

### Deliverables

- Implement all class-start and advancement transitions, AP and SP rules, growth
  rules, prerequisites, mastery limits, and skill or mastery-book use. Record
  the world-content dependencies for every skill acquisition.
- Implement weapon-specific attacks and skill-specific formulas, hit counts,
  target selection, timing, costs, cooldowns, and attack restrictions.
- Complete passive, reactive, sustained, and active effects, including stacking,
  replacement, cancellation, expiration, death, and map-change behavior.
- Add mobility, summons, transformations, elemental interactions, damage over
  time, healing, dispelling, and other baseline skill mechanics.
- Define recipient selection and effects for party skills. Verify their
  transforms here and their complete multiplayer use in M6.
- Implement ordinary mob movement and attack families, status effects,
  resistances, and immunities needed for baseline combat.

### Exit criteria

- [ ] Every required class advancement passes a production-pipeline scenario
      with its real prerequisites, rewards, and rejection conditions.
- [ ] Every required skill has verified behavior at relevant level boundaries
      and under its distinct activation, target, and resource conditions.
- [ ] Formula fixtures cover weapon and job families, rounding, caps, critical
      hits, defenses, and elemental interactions.
- [ ] Skills cannot succeed without applying their required effect, consuming
      their required resources, or enforcing their timing rules.
- [ ] Sustained effects and summons follow the baseline lifecycle across death,
      cancellation, map changes, and reconnects.

## M5: Support distinct players in a shared visible world

**Outcome:** Multiple clients control their own characters and observe a
consistent shared map.

### Deliverables

- Replace the fixed browser identity with authenticated player ownership and
  character creation, selection, and deletion according to the baseline.
- Define session replacement, disconnect, reconnect, and stale-request rules.
  Enforce ownership on every character operation.
- Implement the baseline's world and channel selection and transfer behavior.
  These may run in one process for personal use.
- Replicate remote appearance, position, movement, attacks, skill effects,
  damage, death, and map entry and exit.
- Provide ordered world updates, recovery from missed updates, and client
  interpolation. Match visibility to map and instance membership.
- Implement basic map chat and shared visibility of mobs, reactors, and drops.

### Exit criteria

- [ ] Two independent clients select different characters, meet, move, chat,
      fight, change equipment, and observe each other's actions.
- [ ] A client cannot read or mutate another player's private character state by
      substituting an identifier.
- [ ] Replaced sessions and delayed requests cannot apply stale actions.
- [ ] Disconnects, missed updates, and map transfers converge to authoritative
      state without duplicate characters or stale visible entities.
- [ ] Players in different instances cannot observe or affect each other.
- [ ] Channel transfers preserve character state and follow the baseline's
      restrictions on combat, interactions, and instances.

## M6: Complete party play and the player economy

**Outcome:** Players can progress together and exchange resources safely.

### Deliverables

- Implement party invitations, membership, leadership, departure, disbanding,
  party chat, and party-member state.
- Implement party skill recipients, damage contribution, EXP sharing, quest
  credit, drop ownership, and pickup rules from the baseline.
- Add direct trading, storage, and complete ordinary shop operations, including
  quantities, pricing, selling, and ammunition recharge where applicable.
- Populate and validate the economy needed for the selected progression routes:
  meso drops, item drops, supply costs, and equipment or book availability.
- Extend transaction handling to all participants in a transfer. Define
  reservation, confirmation, cancellation, retry, and recovery behavior.

### Exit criteria

- [ ] A party completes a shared hunt with correct credit, EXP, buffs, and loot.
- [ ] Leadership changes, departures, deaths, and map changes preserve the
      expected membership and reward rules.
- [ ] Trades and storage transfers preserve item identities and total resources.
- [ ] Concurrent confirmation, full inventories, disconnects, and failed saves
      cannot duplicate items, grant free purchases, or lose committed resources.
- [ ] A group can sustain the selected progression routes using normal rewards
      and shops under the reference configuration.

## M7: Complete one party quest and one boss encounter

**Outcome:** The instance and event systems support complete cooperative
encounters, including their failure paths.

### Deliverables

- Implement the party quest and boss selected with their admission rules,
  prerequisites, stages, objectives, portals, reactors, and rewards.
- Add encounter timers, checkpoints, completion, failure, reset, and cleanup.
- Implement the selected boss's attack patterns, skills, phases, body parts,
  shared health relationships, and participant rules as applicable.
- Define reconnect, party changes, leader loss, player death, and restart
  handling. Follow the baseline or document an intentional difference.
- Provide stage instructions, timers, boss health, and other encounter UI.

### Exit criteria

- [ ] A group completes the party quest and boss through normal admission.
- [ ] Incorrect puzzle inputs, timeout, party failure, and premature exit
      produce the expected result without granting completion rewards.
- [ ] Concurrent instances remain isolated and clean up their entities and
      timers after completion or abandonment.
- [ ] Reward claims remain correct under retries, reconnects, and interruption.
- [ ] Encounter traces match the recorded reference sequence and phase rules.

## M8: Complete remaining classic social and item systems

**Outcome:** The remaining baseline activities work alongside progression and
combat.

### Deliverables

- Complete buddies, whispers, guilds, alliances, fame interactions, and their
  membership, visibility, and persistence rules.
- Implement player shops, hired merchants, delivery, and other exchange systems
  required by the baseline.
- Implement pets, pet skills, feeding, mounts, chairs, appearance services,
  crafting, item upgrades beyond M1, and collection rewards as applicable.
- Complete required Cash Shop catalog behavior, previews, storage, packages,
  gifting, expiration, and item effects using locally provisioned currency.
- Implement required marriage, minigames, seasonal activities, and remaining
  special items identified.
- Assign any newly discovered baseline behavior to this milestone or its owning
  earlier milestone; keep it visible in the coverage inventory.

### Exit criteria

- [ ] Every required activity and special item has a recorded scenario.
- [ ] Social changes persist and propagate to the correct recipients.
- [ ] Shop, merchant, delivery, and gift transactions preserve resources under
      concurrent use, retries, expiration, and disconnects.
- [ ] Pets, mounts, cosmetics, and consumable effects follow their complete
      lifecycle through map changes, death, expiration, and reload.
- [ ] All baseline entries have an implementation owner; no residual category is
      hidden behind a generic label such as "miscellaneous content".

## M9: Complete progression and world content

**Outcome:** All required regions and progression content work through normal
gameplay.

### Deliverables

- Complete the required quest chains, NPC programs, scripted portals, reactors,
  shops, transportation, field hazards, and map-specific rules.
- Complete the remaining bosses, party quests, expeditions, and regional
  encounters using the verified runtime and the social and item systems from M8.
- Audit every required drop source, quantity, probability, quest restriction,
  shop price, travel fare, and progression reward.
- Verify script replacements against behavior evidence. Replace placeholders and
  name-only programs where additional behavior is required.
- Add automated checks for unreachable objectives, missing item sources, broken
  destination links, and progression dependencies that cannot be met.

### Exit criteria

- [ ] Every required world-content entry in the inventory is
      behavior-verified.
- [ ] Each region has a recorded gameplay route through its entry, progression,
      travel, and encounter boundaries.
- [ ] Every baseline class can complete its advancement and skill-acquisition
      paths through normal gameplay.
- [ ] Required quest chains and advancement resources are obtainable without
      developer intervention.
- [ ] No required quest is skipped or completed through an unverified fallback.
- [ ] Drop and economy parameters have documented evidence or are explicitly
      recorded as unresolved differences, which prevent this gate from closing.

## M10: Complete client behavior and presentation

**Outcome:** The browser presents the implemented game coherently and matches
the reference behavior at the supported display and input settings.

Client work belongs in every earlier milestone. This milestone verifies the
complete experience and closes remaining presentation differences.

### Deliverables

- Complete character, inventory, skills, quests, party, social, trade, shop,
  storage, map-navigation, encounter, and Cash Shop interfaces.
- Complete key bindings, consumable shortcuts, drag-and-drop operations,
  tooltips, selection, scrolling, and modal focus behavior.
- Match movement feel, attack timing, animation ordering, sprite layers,
  projectiles, damage indicators, status displays, camera behavior, and audio.
- Validate local and remote effects together under latency and uneven frame
  timing. Present rejected actions and reconnect state clearly.
- Define supported browsers, display settings, and an agreed performance budget
  for representative busy scenes.

### Exit criteria

- [ ] Every implemented player action is reachable through the intended UI.
- [ ] Recorded reference scenes pass visual, timing, and audio checks within
      tolerances fixed before verification.
- [ ] Keyboard, pointer, window focus, and modal transitions do not lose actions
      or leave the character in an unintended state.
- [ ] Missing assets and recoverable request failures preserve the session and
      present a usable recovery path.
- [ ] Busy scenes meet the recorded frame-time and update-latency budgets on the
      supported test environment.

## M11: Verify parity and reliable personal operation

**Outcome:** The complete baseline is verified and can be installed, played,
updated, backed up, and restored without manual data repair.

### Deliverables

- Run the complete baseline scenario set and resolve remaining required behavior
  gaps. Publish the final coverage and deviation reports.
- Verify save migration, backup and restore, clean shutdown, process failure,
  database failure, and recovery of interrupted operations.
- Verify concurrent combat, rewards, item transfers, and instance transitions at
  the intended personal-server capacity.
- Document installation, matching content configuration, recovery, and local
  administration. Provide useful diagnostics for failed content and actions.
- Run the project's required formatting, build, test, and WASM checks. Record
  the server revision, archive hashes, configuration, and scenario results for
  the parity release.

### Exit criteria

- [ ] Every required entry is behavior-verified, with no unresolved required
      behavior or progression blockers.
- [ ] Every intentional difference is listed with its effect on the parity
      claim.
- [ ] Fresh-player and returning-player scenarios cover every class and region.
- [ ] Forced interruption and recovery preserve committed resources and prevent
      duplicate rewards; uncommitted work follows documented recovery rules.
- [ ] A backup restores a playable world with the expected character state.
- [ ] A recorded sustained multi-client session meets capacity targets without
      unbounded resource growth, stuck instances, or state divergence.
- [ ] The release can be reproduced from the recorded source and content inputs.

## Implementation and verification rules

Use the existing dataflow architecture as the basis for the work:

```text
input event + state + content rules + explicit time/random inputs
  -> validated action plan
  -> state transforms and persistence
  -> authoritative events and client projections
```

- Keep item definitions, owned instances, character progression, combat state,
  map instances, and presentation data distinct.
- Use free functions with explicit dependencies. Validate at boundaries and
  return typed errors or action values.
- Treat content research and content validation as deliverables alongside
  runtime implementation. WZ data alone does not supply every server rule.
- Test leaf transforms with input/output fixtures and complete workflows through
  the real client/server pipeline. Use explicit seeds or controlled random
  inputs for probabilistic behavior.
- Use browser scenarios for visual, timing, interaction, and shared-world
  behavior that unit tests cannot establish.
- Verify failure and restart behavior as each durable feature is added. M11
  verifies the composition of those guarantees.
- Before expanding a module beyond 1,000 non-test lines, split its separate
  responsibilities into cohesive modules.
- Record milestone evidence with the source revision, content hashes,
  configuration, scenario, expected result, actual result, and open defects.
  Update this document's status table only after all exit criteria pass.

## Reference use

[Cosmic](https://github.com/P0nk/Cosmic) identifies GMS v83 and vanilla gameplay
as its target. It is useful as a behavior reference to investigate and verify.

[HeavenMS's feature list](https://github.com/ronancpl/HeavenMS/blob/master/docs/feature_list.md)
provides a checklist of established server capabilities, including party quests,
social systems, travel, item behavior, and encounter handling. It also contains
custom behavior, so presence on that list does not establish a GMS requirement.

Resolve disagreements through documented baseline evidence and keep uncertain
behavior visible until it is settled.
