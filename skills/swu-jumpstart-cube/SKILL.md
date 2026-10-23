---
name: swu-jumpstart-cube
description: Build a 20-pack Star Wars: Unlimited Jumpstart cube from the user's own collection through the dumpstar /mcp tools, saving each pack as a user-defined deck and checking it with the check_cube tool. Use when asked to build, extend or audit a SWU jumpstart cube.
---

# SWU Jumpstart cube

All the judgement is here. The dumpstar server enforces none of these rules: a pack is a
user-defined deck to it (free-text format label, a group), and it checks only that you own the
copies and that no copy is in two decks. The `check_cube` tool (below) is what tells you the rules hold.

## The cube rules (Ryan's document)

### 1. Format

- **Cube:** 20 half-decks, 10 Heroism and 10 Villainy.
- **Pack:** 18 cards.
- **Singleton:** only Leaders, and non-base Rares and Legendaries, are singleton: each appears in
  at most one of the 20 packs. A card is its title and subtitle, so different printings of the same
  title and subtitle (set reprint, foil, Hyperspace or Showcase) are one card, and the same title
  with a different subtitle is a different card (a Leader and a unit often share a title).
  Bases are always exempt. Commons, Uncommons and Specials may repeat; the collection's copies are
  the only limit, and the deck reservations already enforce that. `check_cube` in `cube` mode fails
  `cube-singleton`, naming the card as "Title — Subtitle" and its packs; `pack` mode does not. A
  card object with no rarity or no `subtitle` member makes `cube` refuse rather than guess.
- **Match:** two random packs make one 30-card draw deck.

### 2. The 18-card pack

1. **1 Leader.** It must be Heroism or Villainy: a **Neutral** leader (the data says `"alignment": "Neutral"`, such as
   Fennec Shand) does not lead a pack, and `check_cube` rejects one. A leader with an alignment but **no color aspect** (Anakin Skywalker, Tempted by the Dark Side, LOF-18: the official record lists Heroism only) cannot lead a pack either, since a pack is defined by its leader's color; the checker rejects it with that message, so pick another leader. It sets the alignment (Heroism/Villainy, the aspect names; `game.alignment` and the summary use them too) and the color aspect.
Bases come in three kinds in the data: 25-HP with an epic action, 28-HP with an ability but
no epic action (`collection_summary` calls these `ability`; never valid in a pack: `base-hp` rejects
them), and 30-HP vanilla.

2. **1 Epic Action Base (25 HP)**, strictly the Leader's color aspect: any 25-HP base of that aspect with an epic action.
3. **1 Vanilla Base (30 HP)**, strictly the Leader's color aspect: any 30-HP base of that aspect without an epic action.
4. **15 draw cards**, none off the Leader's alignment or color aspect (no off-aspect penalties).
   A card of the Leader's color with `"alignment": "Neutral"` (no Heroism/Villainy aspect: Make an
   Opening, Vanquish) carries no aspect penalty and is legal in that leader's pack; so is a card with
   no aspect at all. A Neutral *leader* is not legal. A card of another color,
   or of the other alignment, is not.

### 3. Pre-game setup (why the aspects must match)

A player combines Pack A and Pack B: the Leader comes from A (alignment), the Base from B (color
aspect, which removes the 3-aspect penalty), and B's two bases give the choice of tempo (25 HP
epic action) or health (30 HP vanilla).

### 4. The 15-card skeleton

**Arena and type split**

- 8 ground units.
- 3-4 space units (non-negotiable minimum, so the combined deck can contest space).
- 3-4 events or upgrades (unconditional removal, direct damage, stat buffs).

**Resource curve**

| Cost | Quantity | Purpose |
| --- | --- | --- |
| 0-1 | 0 to 1 | Optional. The checker counts cost 0 with cost 1 (one shared slot of at most 1 card). It does not enforce "premium upgrades or events only"; that is advice. |
| 2 | 4 to 5 | Critical. Ensures a turn-1 play. |
| 3 | 4 to 5 | Critical. Maintains tempo on turn 2. |
| 4 | 2 to 3 | Mid-game stabilizers and Sentinels. |
| 5 | 1 to 2 | Heavy hitters before the Leader flips. |
| 6+ | 1 | The Bomb: one massive unit. The checker's `curve` rule fails a 6+ card that is not a unit ("the 6+ slot must be a unit; <name> is an <type>"). |

### 5. Command zone

Exactly **5 packs per color** across the 20, each with an epic base and a vanilla base of that
color. There are no cards named "Vanilla <color> Base": the vanilla base is any 30-HP base of the
Leader's aspect without an epic action, and the epic base is any 25-HP base of that aspect with one
(`epic_action` on the card object). Ryan's preferred epic bases are the preference, not a requirement:

| Color | Preferred epic (25 HP) |
| --- | --- |
| Command (Green) | Energy Conversion Lab |
| Vigilance (Blue) | Security Complex |
| Aggression (Red) | Tarkintown |
| Cunning (Yellow) | Jedha City |

One rule decides: an epic base is a 25-HP base whose card object has a non-null `epic_action`. A
base is not epic by name; Mystic Monastery and similar bases whose `epic_action` is null are
rejected as the epic base. A collection may not own enough epic bases for 5 per
color (read the owned epic-base counts from `collection_summary` at run time): report the shortfall to the user and build the packs
that are possible; never fail silently.

**Leaders: the universality rule.** Prefer Common Leaders with universal, unconditional abilities
(damage, shielding, resource readying). Avoid Rare, parasitic Leaders whose abilities need
specific traits (Hera Syndulla, Grand Inquisitor) unless the other half-deck will support them.
Top-tier universal examples: Sabine Wren (SOR-14), Boba Fett (SOR-15), Darth Vader, Dark Lord of the Sith (SOR-10). Leia Organa, Alliance General (SOR-9) is not one: her ability needs Rebel units.

### 6. Archetype blueprints

For the 15 draw cards, favor universal "goodstuff" and additive mechanics (Shields, Sentinels,
Smuggle, Raid) that improve whatever pack they meet.

1. **Rebel Vanguard** (Heroism / Command): go-wide aggro with tribal buffs. 2-cost Rebels, General Dodonna, Medal Ceremony, You're In Command.
2. **Imperial Armor** (Villainy / Command): resource acceleration into heavy bombs. Superlaser Technician/Resupply, Overwhelming Barrage, AT-ST/Relentless.
3. **Defensive Fortress** (Any / Vigilance): hard control and attrition. Regional Governor/Sentinels, Protective Shields, Takedown/Vanquish.
4. **Underworld Smugglers** (Any / Cunning): board manipulation, tempo exhaust, Smuggle. Crafty Smuggler, Waylay, Asteroid Sanctuary, Shoot First.
5. **Aggressive Assault** (Any / Aggression): direct damage and raid. Sabine Wren/Disruptor Sniper, Open Fire, Keep Fighting.
6. **Starfighter Fleet** (Any / flex aspect): space skew. At least 6 space units (A-Wings/Interceptors), Wing Leader, Tactical Advantage.

The archetypes and the leader guidance are advice. `check_cube` does not test them.

## Never save tool output to files

`check_cube` reads the site itself. **Never copy a `get_deck` or `search_cards` result into a file
or into the checker's arguments** (retyping 15 KB per pack is how a walk once failed): name deck
ids or a group and `check_cube` fetches the decks and checks each deck's `lines_digest`. It uses
the dumpstar sign-in Claude Code already holds, so there is nothing to sign in to beyond the one
`/mcp` sign-in for `dumpstar`. If `check_cube` returns an error naming a sign-in, reconnect
`dumpstar` in `/mcp` (or make any dumpstar tool call to refresh the token) and call it again; do
not fall back to copying output around.

## Method

**Group name.** Ask the user which group the packs go in; default `swu-jumpstart-cube`. A partial
or test build names its own group so it never lands in the real cube's group. Use that one group
for every `create_deck` and `list_decks` below.

To rename a pack or move it to another group, call `update_deck` (`{id, name?, group?, format?,
description?, notes?}`; absent keys stay, null clears). Never delete and recreate a deck: that loses
its id.

Mind the rate limit: `/mcp` allows about 120 tool calls a minute per sign-in. Use a fixed handful
of searches per pack (step 3) and choose from the results yourself; never search once per card.

1. **`collection_summary`.** Learn what is owned and what is still available. A cube needs about
   20 Leaders, 40 bases (5 epic and 5 vanilla per color) and 300 draw cards. Leaders and non-base Rares and Legendaries are singleton,
   so count **distinct** ones (by title and subtitle); Commons, Uncommons and bases may repeat, so copies count for them.
   Size the Leaders from `leaders.by_aspect_alignment`, not `leaders.by_aspect`: `by_aspect` is a raw
   count of leaders having each aspect, so it counts a two-color leader under both colors and
   overstates how many can lead a pack of one color.
2. **Allocate leaders and base pairs.** Pick 20 Leaders: 10 Heroism, 10 Villainy, 5 of each color
   (for example Command 3+2, Vigilance 2+3, Aggression 3+2, Cunning 2+3). Apply the universality
   rule. Each pack takes an epic and a vanilla base of its Leader's color; if the collection lacks them, say so and build what is possible.
3. **Plan copies across the whole cube.** Whenever two or more packs are planned together (a
   full cube or any multi-pack build), plan copies before saving any pack. Heroism-only, Villainy-only and no-aspect cards (Longbeam Cruiser) fit packs of any color, so an
   early pack's picks can use up copies a later color depends on, and the per-pack check cannot
   see it. Call `check_cube` with `mode: allocate` and
   `group: <group>` and/or `plan` (saved packs, plus any planned ones as `plan: {packs: [{name, queries?, deck?}]}`,
   each pack naming the `queries` the checker runs and/or a saved `deck` id): it runs the pool searches itself and prints one line per pack
   (picks of slots, eligible pool, skeleton ok/unmet/not checked, contested reserved, short, lost) and `FAIL` lines
   where a pack cannot be filled. `verbose: true` adds the `reserve <card>: <packs>` lines, naming
   the packs that hold each contested card; `fill: true` also chooses cards for open slots. Do not pick a
   card another pack holds the reservation for. `set_deck_lines` refuses a pick no copy is left for;
   `check_cube` in `copies` mode (step 6) is the proof.
4. **Fill each pack to the skeleton.** Four `search_cards` per pack cover the Leader's whole pool,
   because section 2 allows four sets and no one query returns them all: color and alignment, color
   only, alignment only, and no aspect at all. None returns Leaders or bases, which are chosen
   separately. All pass `avail>=1` in the query (when `query_help` lists `avail`; otherwise
   pass `owned_only: true` and keep results whose `available_count` is at least 1), `limit: 100`.
   `<other colors>` is each color but the Leader's (`-a:<other color>` once per color), and `<other
   alignment>` is the opposite of the Leader's:
   - color+alignment: `a:<color> a:<alignment> -a:<each other color> -t:leader -t:base avail>=1`
   - color only: `a:<color> -a:heroism -a:villainy -a:<each other color> -t:leader -t:base avail>=1`
   - alignment only: `a:<alignment> -a:<every color> -t:leader -t:base avail>=1`
   - no aspect: `-a:heroism -a:villainy -a:<every color> -t:leader -t:base avail>=1`

   `<every color>` is all four colors (`-a:command -a:vigilance -a:aggression -a:cunning`). Each
   `-a:<other color>` drops the cards that carry a second color (a Vigilance + Cunning unit, a
   three-aspect IG-88): they fail the pack's aspect rule. For Vigilance and Heroism the color+alignment
   query is `a:vigilance a:heroism -a:command -a:aggression -a:cunning -t:leader -t:base avail>=1`.
   The alignment-only set is cards such as Alliance X-Wing and Wing Leader (Heroism, no color); a
   Villainy leader's pool has AT-ST there. The checker accepts exactly these four sets.

   One more for the color's bases: `t:base a:<color> -t:leader avail>=1`. Then pick from the combined results the 15
   cards that meet section 4, the 25-HP and 30-HP bases, and the Leader. A contested card reserved to another pack in step 3 is not yours to pick. Prefer
   cards of the Leader's own alignment and color; same-color cards with no alignment, cards of the Leader's
   alignment with no color, and cards with no aspects are also allowed.
5. **Save the pack by card ids, then check the saved deck.** A planned pack is named by card
   ids only: the Leader, the two bases and the 15 draw ids, each with a qty of 1. Write no card
   objects and no files. `create_deck` (`format`: `SWU Jumpstart Cube`, `group`: the group name
   (the same for all 20 packs), `name`: `Pack <n>`), then `set_deck_lines` with the 18
   `{card_id, qty: 1}` lines. A refusal says which lines are short and changes nothing: swap those
   cards and retry. Then call `check_cube` with `mode: pack` and `deck_ids: [<id>]`: the checker fetches the deck and its
   card data itself. Fix every `FAIL` line with `set_deck_lines` on the same deck (it replaces the
   lines; never delete and recreate) and call `check_cube` on it again.
6. **Check the whole group.** Call `check_cube` with `mode: cube` and `group: <group>`: it lists the
   group's decks, fetches each, and checks them (no `get_deck` calls, no files). `cube` sums each card's `qty` over the packs and fails `cube-copies`, naming
   the card and the packs, when the sum exceeds the `owned_count` the site reports. `reserved` must equal `qty` on every line. The cube-level check applies to a
   complete 20-pack cube only; a partial or test build checks each saved deck with
   `mode: pack` and `deck_ids` instead (`cube-size`, `cube-alignment` and `cube-bases` would fail),
   and checks copies across them with `mode: copies` and the group, which sums
   each card's `qty` against `owned_count` and prints only `cube-copies` failures (or `ok`).
   `get_deck` carries a `lines_digest` and the checker recomputes it from the lines and the card objects under `cards`. If it
   refuses with "this is not the saved deck: digest mismatch", the deck changed
   while it was read: call it again. Never edit anything (or the digest) to make it pass.

## The checker

`check_cube` is a tool of this plugin's `swu-cube-check` MCP server. Its arguments: `mode`
(`pack`, `cube`, `copies` or `allocate`), the decks as `deck_ids` (integers) or as a `group` name (check_cube takes either)
(`allocate` takes `group` and/or `plan`, plus `verbose` and `fill`, never `deck_ids`), and
optionally `site`. It prints `ok`, or one `FAIL <rule>: <what is wrong>` line per broken rule; an
input it cannot fetch or read (no sign-in, an unknown deck, an empty group) comes back as a tool
error, never as `ok`. With several `deck_ids` in `pack` mode each line is prefixed `deck <id>:`.

The deck it checks is `get_deck`'s result, which only the checker handles: it fetches it and you
never write one. It carries the card object of every line under `cards`, keyed by card id (the
`<SET>-<number>` form), each with the shape of a `search_cards` row.

Card fields the checker reads and the digest covers (generated from `DIGEST_CARD_FIELDS` in `crates/swu-cube-check/src/lib.rs`; a test asserts this line matches it): `/name`, `/subtitle`, `/type`, `/rarity`, `/cost`, `/game/hp`, `/game/epic_action`, `/game/aspects`, `/game/arenas`. 

The `reserved` rule is meaningful only on the saved deck, where the server reports reservations.
`pack` never checks copies across packs: only `cube` or `copies` over the saved decks (or
`set_deck_lines` refusing) has run that sum. `copies` needs each card's `owned_count`, which a saved
deck carries.

The rules reported are: `shape`, `leader`, `base-hp`, `base-aspect`, `base-name`,
`draw-aspect`, `arena`, `curve`, `reserved`, `unknown-card`, and for the cube `cube-size`,
`cube-alignment`, `cube-bases`, `cube-singleton`, `cube-copies`. `draw-aspect` rejects a draw card with any color other than the Leader's or any
alignment other than the Leader's. A card with no alignment (of the Leader's color) or with no
aspects at all passes.

Finish by telling the user which packs were built, which Leaders you chose and why, and the
`check_cube` cube output.
