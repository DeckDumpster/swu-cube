//! The rule checkers of the `swu-jumpstart-cube` skill: [`check_pack`] and [`check_cube`].
//!
//! They read the deck JSON the `/mcp` tools return and judge it against Ryan's cube rules
//! (`skills/swu-jumpstart-cube/SKILL.md`). The dumpstar server holds none of these rules.
//!
//! # Input
//!
//! A *pack document* is `get_deck`'s result as-is: it carries the card objects its lines name, as
//! `search_cards`/`get_card` return them, under `cards` keyed by card id:
//!
//! ```json
//! {"deck": {"name": "Pack 1", "group": "swu-jumpstart-cube"},
//!  "lines": [{"card_id": "SOR-5", "qty": 1, "zone": "main", "reserved": 1}],
//!  "cards": {"SOR-5": {"name": "...", "type": "Leader", "cost": 5,
//!                       "game": {"aspects": ["Vigilance", "Heroism"], "arenas": ["Ground"], "hp": 6}}}}
//! ```
//!
//! A *cube document* is an array of pack documents, or `{"packs": [...], "cards": {...}}` where
//! `cards` supplies any card a pack's own `cards` lacks.

pub mod allocate;
pub mod fetch;
pub mod mcp;
pub mod site;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

pub const COLORS: [&str; 4] = ["Command", "Vigilance", "Aggression", "Cunning"];
pub const ALIGNMENTS: [&str; 2] = ["Heroism", "Villainy"];
/// Ryan's preferred 25-HP epic-action base of each color; a preference, not a requirement.
pub const EPIC_BASES: [(&str, &str); 4] = [
    ("Command", "Energy Conversion Lab"),
    ("Vigilance", "Security Complex"),
    ("Aggression", "Tarkintown"),
    ("Cunning", "Jedha City"),
];

pub const PACK_SIZE: u32 = 18;
pub const DRAW_CARDS: u32 = 15;
pub const CUBE_PACKS: usize = 20;
pub const PACKS_PER_COLOR: usize = 5;
pub const GROUND_UNITS: u32 = 8;
/// Cost 1, 2, 3, 4, 5, 6+: the inclusive quantity range of each.
pub const CURVE: [(u32, u32); 6] = [(0, 1), (4, 5), (4, 5), (2, 3), (1, 2), (1, 1)];

/// One broken rule: a short `rule` slug, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub rule: &'static str,
    pub message: String,
    /// A warning: printed as `WARN`, and does not fail the check.
    pub warn: bool,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

fn fail(rule: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        rule,
        message: message.into(),
        warn: false,
    }
}

fn warn(rule: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        warn: true,
        ..fail(rule, message)
    }
}

/// A mechanic some cards consume and others produce, found by rules-text patterns (lowercase
/// substrings). A card's `game.mechanics` (`{"consumes": [...], "produces": [...]}`, mechanic
/// names), when the catalog carries it, takes the place of the patterns for that card.
struct Mechanic {
    name: &'static str,
    consumes: &'static [&'static str],
    produces: &'static [&'static str],
}

const MECHANICS: [Mechanic; 1] = [Mechanic {
    name: "force",
    consumes: &["use the force"],
    produces: &["the force is with you"],
}];

/// The part of a leader's rules text its undeployed side prints: before `Deployed:`, without the
/// epic action that deploys it.
fn front_side(text: &str) -> String {
    let front = text.split("Deployed:").next().unwrap_or("");
    front
        .lines()
        .filter(|l| !l.trim_start().starts_with("Epic Action"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A card the checkers care about, read from a `search_cards`/`get_card` card object.
#[derive(Debug, Clone)]
struct Card {
    name: String,
    /// `None` when the card object has no `subtitle` key; a null subtitle is `Some("")`.
    subtitle: Option<String>,
    rarity: Option<String>,
    kind: String,
    cost: Option<u32>,
    hp: Option<i64>,
    /// `Some` when the card object carries a non-null `epic_action`; a null is no epic action.
    epic_action: Option<bool>,
    aspects: BTreeSet<String>,
    arenas: BTreeSet<String>,
    /// The card object's `owned_count`, when the file carries one.
    owned: Option<u32>,
    rules_text: String,
    /// `game.mechanics`, when the catalog carries it: (consumed, produced) mechanic names.
    mechanics: Option<(BTreeSet<String>, BTreeSet<String>)>,
    /// `game.traits`, when the catalog carries it.
    traits: BTreeSet<String>,
}

impl Card {
    /// Whether the card consumes (`produce` false) or produces mechanic `m`. A leader is read on
    /// its undeployed side only; a base's consumption is not counted.
    fn touches(&self, m: &Mechanic, produce: bool) -> bool {
        if let Some((consumes, produces)) = &self.mechanics {
            return if produce { produces } else { consumes }.contains(m.name);
        }
        if self.kind == "Base" && !produce {
            return false;
        }
        let text = if self.kind == "Leader" {
            front_side(&self.rules_text)
        } else {
            self.rules_text.clone()
        }
        .to_lowercase();
        if produce { m.produces } else { m.consumes }
            .iter()
            .any(|p| text.contains(p))
    }

    fn from_value(v: &Value) -> Card {
        let strings = |p: &Value| -> BTreeSet<String> {
            p.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        };
        let game = &v["game"];
        Card {
            name: v["name"].as_str().unwrap_or("?").to_string(),
            subtitle: v
                .get("subtitle")
                .map(|s| s.as_str().unwrap_or("").to_string()),
            rarity: v["rarity"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(String::from),
            kind: v["type"].as_str().unwrap_or("").to_string(),
            cost: v["cost"].as_u64().and_then(|c| u32::try_from(c).ok()),
            hp: game["hp"].as_i64(),
            epic_action: game
                .get("epic_action")
                .filter(|e| !e.is_null())
                .map(|e| e.as_str().is_some_and(|t| !t.is_empty())),
            aspects: strings(&game["aspects"]),
            arenas: strings(&game["arenas"]),
            owned: v["owned_count"]
                .as_u64()
                .and_then(|o| u32::try_from(o).ok()),
            rules_text: game["rules_text"].as_str().unwrap_or("").to_string(),
            mechanics: game.get("mechanics").filter(|m| m.is_object()).map(|m| {
                (strings(&m["consumes"]), strings(&m["produces"]))
            }),
            traits: strings(&game["traits"]),
        }
    }

    /// Whether this base has an epic action: the catalog's `epic_action`, and nothing else.
    fn is_epic(&self) -> bool {
        self.epic_action == Some(true)
    }

    fn colors(&self) -> Vec<&str> {
        COLORS
            .into_iter()
            .filter(|c| self.aspects.contains(*c))
            .collect()
    }

    fn alignments(&self) -> Vec<&str> {
        ALIGNMENTS
            .into_iter()
            .filter(|a| self.aspects.contains(*a))
            .collect()
    }
}

struct Line {
    card_id: String,
    qty: u32,
    sideboard: bool,
    reserved: Option<u32>,
}

/// A pack with its cards resolved.
pub struct Pack {
    name: String,
    lines: Vec<Line>,
    cards: BTreeMap<String, Card>,
}

impl Pack {
    /// Reads a pack document; `fallback` supplies cards the pack's own `cards` lacks.
    pub fn from_value(v: &Value, fallback: &Value) -> Result<Pack, String> {
        let lines = v
            .get("lines")
            .and_then(Value::as_array)
            .ok_or("a pack document needs a 'lines' array (get_deck's result)")?;
        let mut cards = BTreeMap::new();
        for source in [fallback, &v["cards"]] {
            if let Some(m) = source.as_object() {
                for (id, c) in m {
                    cards.insert(id.clone(), Card::from_value(c));
                }
            }
        }
        Ok(Pack {
            name: v["deck"]["name"]
                .as_str()
                .unwrap_or("(unnamed)")
                .to_string(),
            lines: lines
                .iter()
                .map(|l| Line {
                    card_id: l["card_id"].as_str().unwrap_or("").to_string(),
                    qty: l["qty"]
                        .as_u64()
                        .and_then(|q| u32::try_from(q).ok())
                        .unwrap_or(0),
                    sideboard: l["zone"].as_str() == Some("sideboard"),
                    reserved: l["reserved"].as_u64().and_then(|q| u32::try_from(q).ok()),
                })
                .collect(),
            cards,
        })
    }

    /// The leader's alignment and color, when the pack has exactly one valid leader.
    fn leader(&self) -> Option<(String, String)> {
        let leaders: Vec<&Card> = self.of_kind("Leader").map(|(c, _)| c).collect();
        let [l] = leaders.as_slice() else { return None };
        match (l.alignments().as_slice(), l.colors().as_slice()) {
            ([a], [c]) => Some((a.to_string(), c.to_string())),
            _ => None,
        }
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = (&'a Card, u32)> + 'a {
        self.lines.iter().filter_map(move |l| {
            self.cards
                .get(&l.card_id)
                .filter(|c| c.kind == kind)
                .map(|c| (c, l.qty))
        })
    }
}

/// The hard checks of one pack. An empty result passes.
pub fn check_pack(pack: &Pack) -> Vec<Failure> {
    let mut out = Vec::new();

    for l in &pack.lines {
        if !pack.cards.contains_key(&l.card_id) {
            out.push(fail(
                "unknown-card",
                format!("card '{}' has no card object to check against", l.card_id),
            ));
        }
        if l.sideboard {
            out.push(fail(
                "shape",
                format!("'{}' is in the sideboard; a pack has none", l.card_id),
            ));
        }
        if let Some(r) = l.reserved
            && r < l.qty
        {
            out.push(fail(
                "reserved",
                format!("'{}' is reserved {r} of {} copies", l.card_id, l.qty),
            ));
        }
    }

    let total: u32 = pack.lines.iter().map(|l| l.qty).sum();
    if total != PACK_SIZE {
        out.push(fail("shape", format!("{total} cards, not {PACK_SIZE}")));
    }
    let count = |kind: &str| pack.of_kind(kind).map(|(_, q)| q).sum::<u32>();
    if count("Leader") != 1 {
        out.push(fail("shape", format!("{} leaders, not 1", count("Leader"))));
    }
    if count("Base") != 2 {
        out.push(fail("shape", format!("{} bases, not 2", count("Base"))));
    }
    let draw: Vec<(&Card, u32)> = ["Unit", "Event", "Upgrade"]
        .into_iter()
        .flat_map(|k| pack.of_kind(k))
        .collect();
    let draw_count: u32 = draw.iter().map(|(_, q)| q).sum();
    if draw_count != DRAW_CARDS {
        out.push(fail(
            "shape",
            format!("{draw_count} draw cards, not {DRAW_CARDS}"),
        ));
    }
    for l in &pack.lines {
        if let Some(c) = pack.cards.get(&l.card_id)
            && !["Leader", "Base", "Unit", "Event", "Upgrade"].contains(&c.kind.as_str())
        {
            out.push(fail(
                "shape",
                format!("'{}' is a {}, not a draw card", c.name, c.kind),
            ));
        }
    }

    let leader = pack.of_kind("Leader").next().map(|(c, _)| c);
    if count("Leader") > 1 {
        out.push(fail(
            "leader",
            format!("the pack has {} leaders, not 1", count("Leader")),
        ));
    } else if let Some(l) = leader
        && pack.leader().is_none()
    {
        if l.alignments().is_empty() {
            out.push(fail(
                "leader",
                format!(
                    "leader '{}' is Neutral (no Heroism or Villainy); a Neutral leader does not lead a pack",
                    l.name
                ),
            ));
        } else if l.colors().is_empty() {
            out.push(fail(
                "leader",
                format!(
                    "leader '{}' has no color aspect ({:?}); a pack is defined by its leader's color, so a colorless leader cannot lead a pack",
                    l.name, l.aspects
                ),
            ));
        } else {
            out.push(fail(
                "leader",
                format!(
                    "leader '{}' must have exactly one alignment and one color, has {:?}",
                    l.name, l.aspects
                ),
            ));
        }
    }

    let bases: Vec<&Card> = pack.of_kind("Base").map(|(c, _)| c).collect();
    let mut hps: Vec<i64> = bases.iter().filter_map(|b| b.hp).collect();
    hps.sort_unstable();
    if bases.len() == 2 && hps != [25, 30] {
        if hps == [25, 28] {
            let force_reason = pack
                .lines
                .iter()
                .filter_map(|l| pack.cards.get(&l.card_id))
                .any(|c| {
                    c.kind != "Base"
                        && (c.traits.contains("Force")
                            || c.touches(&MECHANICS[0], false))
                });
            if !force_reason {
                out.push(fail(
                    "base-hp",
                    "a 28-HP ability base stands in for the 30-HP base only when the pack has a Force unit or a Force consumer, and this pack has neither",
                ));
            }
        } else {
            out.push(fail(
                "base-hp",
                format!("bases have HP {hps:?}, not one 25 and one 30 (or one 25 and one 28-HP Force base)"),
            ));
        }
    }

    let base_color = pack.leader().map(|(_, c)| c).or_else(|| {
        let colors: Vec<String> = pack
            .of_kind("Leader")
            .filter_map(|(l, _)| match l.colors().as_slice() {
                [c] => Some(c.to_string()),
                _ => None,
            })
            .collect();
        colors.first().cloned()
    });
    if pack.leader().is_none()
        && let Some(color) = &base_color
    {
        for b in &bases {
            if b.colors() != [color.as_str()] {
                out.push(fail(
                    "base-aspect",
                    format!(
                        "base '{}' is {:?}, the leader's color is {color}",
                        b.name,
                        b.colors()
                    ),
                ));
            }
        }
    }

    if let Some((alignment, color)) = pack.leader() {
        for b in &bases {
            if b.colors() != [color.as_str()] {
                out.push(fail(
                    "base-aspect",
                    format!(
                        "base '{}' is {:?}, the leader's color is {color}",
                        b.name,
                        b.colors()
                    ),
                ));
            }
            if b.hp == Some(25) && !b.is_epic() {
                out.push(fail(
                    "base-name",
                    format!(
                        "the 25-HP {color} base must be an epic base, but '{}' has no epic action",
                        b.name
                    ),
                ));
            }
            if b.hp == Some(30) && b.epic_action == Some(true) {
                out.push(fail(
                    "base-name",
                    format!(
                        "the 30-HP {color} base must be vanilla, but '{}' has an epic action",
                        b.name
                    ),
                ));
            }
        }
        for (c, _) in &draw {
            let off_color = c.colors().iter().any(|x| *x != color);
            let off_align = c.alignments().iter().any(|x| *x != alignment);
            if off_color || off_align {
                out.push(fail(
                    "draw-aspect",
                    format!(
                        "'{}' has aspects {:?}, the leader is {alignment} {color}",
                        c.name, c.aspects
                    ),
                ));
            }
        }
    }

    let (mut ground, mut space, mut spells) = (0, 0, 0);
    for (c, q) in &draw {
        match c.kind.as_str() {
            "Unit" if c.arenas.contains("Space") => space += q,
            "Unit" if c.arenas.contains("Ground") => ground += q,
            "Unit" => out.push(fail(
                "arena",
                format!(
                    "'{}' is a unit with no arena, so it is neither ground nor space",
                    c.name
                ),
            )),
            _ => spells += q,
        }
    }
    if ground != GROUND_UNITS {
        out.push(fail(
            "arena",
            format!("{ground} ground units, not {GROUND_UNITS}"),
        ));
    }
    if !(3..=4).contains(&space) {
        out.push(fail("arena", format!("{space} space units, not 3 or 4")));
    }
    if !(3..=4).contains(&spells) {
        out.push(fail(
            "arena",
            format!("{spells} events and upgrades, not 3 or 4"),
        ));
    }

    let mut curve = [0u32; 6];
    for (c, q) in &draw {
        match c.cost {
            Some(cost) => curve[(cost.clamp(1, 6) - 1) as usize] += q,
            None => out.push(fail("curve", format!("'{}' has no cost", c.name))),
        }
    }
    for (i, (lo, hi)) in CURVE.iter().enumerate() {
        let label = if i == 5 {
            "6+".to_string()
        } else {
            (i + 1).to_string()
        };
        if !(*lo..=*hi).contains(&curve[i]) {
            out.push(fail(
                "curve",
                format!("{} cards at cost {label}, want {lo} to {hi}", curve[i]),
            ));
        }
    }
    let big: Vec<&Card> = draw
        .iter()
        .filter(|(c, _)| c.cost.is_some_and(|n| n >= 6))
        .map(|(c, _)| *c)
        .collect();
    if !big.is_empty()
        && !big.iter().any(|c| c.kind == "Unit")
        && let Some(c) = big.first()
    {
        out.push(fail(
            "curve",
            format!(
                "the 6+ slot must be a unit; {} is an {}",
                c.name,
                c.kind.to_lowercase()
            ),
        ));
    }
    out.extend(mechanic_failures(pack));
    out
}

/// A consumer with nothing in its own pack to produce what it spends. A match is two random packs,
/// so only this pack's own cards (and the leader's front side) are dependable producers. The
/// leader's undeployed side is a failure; a draw card is a warning.
fn mechanic_failures(pack: &Pack) -> Vec<Failure> {
    let mut out = Vec::new();
    let cards: Vec<&Card> = pack
        .lines
        .iter()
        .filter_map(|l| pack.cards.get(&l.card_id))
        .collect();
    for m in &MECHANICS {
        if cards.iter().any(|c| c.touches(m, true)) {
            continue;
        }
        for c in &cards {
            if !c.touches(m, false) {
                continue;
            }
            if c.kind == "Leader" {
                out.push(fail(
                    "leader-mechanic",
                    format!(
                        "leader '{}' spends {} on its undeployed side, and nothing in the pack produces it",
                        c.name, m.name
                    ),
                ));
            } else if c.kind != "Base" {
                out.push(warn(
                    "pack-mechanic",
                    format!(
                        "'{}' spends {}, and nothing in the pack produces it",
                        c.name, m.name
                    ),
                ));
            }
        }
    }
    out
}

/// The planned cards (`search_cards`/`get_card` rows) that spend a mechanic nothing in `cards`
/// produces, as (card name, mechanic). `cards` is a pack's picks plus its leader, when known.
pub fn unproduced_consumers(cards: &[&Value]) -> Vec<(String, &'static str)> {
    let cards: Vec<Card> = cards.iter().map(|v| Card::from_value(v)).collect();
    let mut out = Vec::new();
    for m in &MECHANICS {
        if cards.iter().any(|c| c.touches(m, true)) {
            continue;
        }
        out.extend(
            cards
                .iter()
                .filter(|c| c.touches(m, false))
                .map(|c| (c.name.clone(), m.name)),
        );
    }
    out
}

/// Cube-wide: a mechanic consumed somewhere and produced nowhere fails; for each pack that
/// consumes one it does not produce itself, the share of the other packs that do.
fn cube_mechanic_findings(packs: &[Pack]) -> Vec<Failure> {
    let mut out = Vec::new();
    let in_pack = |p: &Pack, m: &Mechanic, produce: bool| -> u32 {
        p.lines
            .iter()
            .filter_map(|l| p.cards.get(&l.card_id).map(|c| (c, l.qty)))
            .filter(|(c, _)| c.kind != "Base" || produce)
            .filter(|(c, _)| c.touches(m, produce))
            .map(|(_, q)| q)
            .sum()
    };
    for m in &MECHANICS {
        let consumers: u32 = packs.iter().map(|p| in_pack(p, m, false)).sum();
        let producers: u32 = packs.iter().map(|p| in_pack(p, m, true)).sum();
        if consumers > 0 && producers == 0 {
            out.push(fail(
                "cube-mechanic",
                format!("{} consumed by {consumers} cards, produced by 0", m.name),
            ));
        }
        let producing: Vec<bool> = packs.iter().map(|p| in_pack(p, m, true) > 0).collect();
        for (i, p) in packs.iter().enumerate() {
            if in_pack(p, m, false) == 0 || producing[i] {
                continue;
            }
            let others = packs.len() - 1;
            let covered = producing.iter().filter(|b| **b).count();
            out.push(warn(
                "mechanic-report",
                format!(
                    "pack '{}' spends {} and makes none; {covered} of {others} other packs produce it ({}%)",
                    p.name,
                    m.name,
                    if others == 0 { 0 } else { covered * 100 / others }
                ),
            ));
        }
    }
    out
}

/// Reads a pack document and checks it.
/// Drops every `reserved` member: a planned (pre-save) document has no reservations to check,
/// so the `reserved` rule is meaningful only on the post-save `get_deck` document.
pub fn strip_reservations(doc: &mut Value) {
    match doc {
        Value::Array(items) => items.iter_mut().for_each(strip_reservations),
        Value::Object(o) => {
            if let Some(Value::Array(lines)) = o.get_mut("lines") {
                for l in lines {
                    if let Some(m) = l.as_object_mut() {
                        m.remove("reserved");
                    }
                }
            }
            if let Some(packs) = o.get_mut("packs") {
                strip_reservations(packs);
            }
        }
        _ => {}
    }
}

/// The card fields the digest covers, per line: the ones the checker reads (name, type, cost, HP,
/// epic action, aspects, arenas), subtitle and rarity. Each is a JSON pointer into the card object; arrays
/// are sorted. This list is the single source for the server (which computes the digest) and the
/// checker (which recomputes it), so they cannot drift.
pub const DIGEST_CARD_FIELDS: [&str; 9] = [
    "/name",
    "/subtitle",
    "/type",
    "/rarity",
    "/cost",
    "/game/hp",
    "/game/epic_action",
    "/game/aspects",
    "/game/arenas",
];

/// The digest `get_deck` returns as `lines_digest`: SHA-256 (lowercase hex) over the canonical form
/// of the deck's lines: per line one `LF`-terminated row of `TAB`-separated cells, sorted as
/// rows: `card_id`, `qty`, `reserved`, then the line's card object's [`DIGEST_CARD_FIELDS`], each
/// as compact JSON (`null` when absent; arrays sorted). `zone` is not part of it. `None` when
/// `lines` is not an array of lines with a card id and a qty.
pub fn lines_digest(lines: &Value, cards: &Value) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut rows = Vec::new();
    for l in lines.as_array()? {
        let id = l["card_id"].as_str()?;
        let mut row = vec![
            id.to_string(),
            l["qty"].as_u64()?.to_string(),
            l["reserved"].as_u64().unwrap_or(0).to_string(),
        ];
        let card = cards.get(id).unwrap_or(&Value::Null);
        for ptr in DIGEST_CARD_FIELDS {
            let mut v = card.pointer(ptr).cloned().unwrap_or(Value::Null);
            if let Value::Array(a) = &mut v {
                a.sort_by_key(|x| x.to_string());
            }
            row.push(v.to_string());
        }
        rows.push(row.join("\t"));
    }
    rows.sort();
    let canon: String = rows.iter().map(|r| format!("{r}\n")).collect();
    Some(
        Sha256::digest(canon.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Refuses a document whose `lines_digest` does not match its own `lines` and `cards`: it is not
/// the saved deck `get_deck` returned. A document with no `lines_digest` (a planned one) passes.
pub fn verify_digest(doc: &Value) -> Result<(), String> {
    let Some(claimed) = doc.get("lines_digest") else {
        return Ok(());
    };
    let actual = doc
        .get("lines")
        .and_then(|l| lines_digest(l, &doc["cards"]));
    if actual.as_deref() == claimed.as_str() {
        Ok(())
    } else {
        Err("this is not the saved deck: digest mismatch; copy the get_deck result again".into())
    }
}

/// The shared test vector: the server and the checker both compute this digest of these lines.
pub const VECTOR_LINES: &str = r#"[{"card_id":"SOR-5","qty":1,"zone":"main","reserved":1},{"card_id":"C1-3","qty":2,"zone":"main","reserved":2},{"card_id":"JTL-10","qty":1,"zone":"sideboard","reserved":0}]"#;
pub const VECTOR_CARDS: &str = r#"{"SOR-5":{"name":"Leader","subtitle":"Sub","type":"Leader","rarity":"Rare","cost":5,"game":{"aspects":["Villainy","Command"],"arenas":["Ground"],"hp":6,"epic_action":null}},"C1-3":{"name":"Base","subtitle":null,"type":"Base","rarity":"Common","cost":null,"game":{"aspects":["Vigilance"],"arenas":[],"hp":25,"epic_action":"Epic Action: Draw a card."}}}"#;
pub const VECTOR_DIGEST: &str = "39b256e5fe319322da2c7e8a6cbf4eda2c5ae6471249ea1741894edb101eee3e";

/// The checker's verdict as the CLI prints it and its exit code: `ok` (0), one `FAIL` line per
/// failure (1), or the unreadable-input message (2).
pub fn render(result: Result<Vec<Failure>, String>) -> (i32, String) {
    match result {
        Err(e) => (2, e),
        Ok(failures) if failures.is_empty() => (0, "ok".into()),
        Ok(failures) => {
            let hard = failures.iter().any(|f| !f.warn);
            let lines = failures.iter().map(|f| {
                format!("{} {f}", if f.warn { "WARN" } else { "FAIL" })
            });
            let text: Vec<String> = if hard {
                lines.collect()
            } else {
                std::iter::once("ok".to_string()).chain(lines).collect()
            };
            (i32::from(hard), text.join("\n"))
        }
    }
}

pub fn check_pack_value(doc: &Value) -> Result<Vec<Failure>, String> {
    Ok(check_pack(&Pack::from_value(doc, &Value::Null)?))
}

/// The pack checks of every pack, then the cube's: 20 packs, 10 of each alignment, 5 of each
/// color's base pair.
pub fn check_cube(packs: &[Pack]) -> Vec<Failure> {
    let mut out = Vec::new();
    for p in packs {
        out.extend(check_pack(p).into_iter().map(|f| Failure {
            message: format!("pack '{}': {}", p.name, f.message),
            ..f
        }));
    }
    if packs.len() != CUBE_PACKS {
        out.push(fail(
            "cube-size",
            format!("{} packs, not {CUBE_PACKS}", packs.len()),
        ));
    }
    let mut alignments: BTreeMap<String, usize> = BTreeMap::new();
    let mut colors: BTreeMap<String, usize> = BTreeMap::new();
    for (a, c) in packs.iter().filter_map(Pack::leader) {
        *alignments.entry(a).or_default() += 1;
        *colors.entry(c).or_default() += 1;
    }
    for a in ALIGNMENTS {
        let n = alignments.get(a).copied().unwrap_or(0);
        if n != CUBE_PACKS / 2 {
            out.push(fail(
                "cube-alignment",
                format!("{n} {a} packs, not {}", CUBE_PACKS / 2),
            ));
        }
    }
    for c in COLORS {
        let n = colors.get(c).copied().unwrap_or(0);
        if n != PACKS_PER_COLOR {
            out.push(fail(
                "cube-bases",
                format!("{n} {c} base pairs, not {PACKS_PER_COLOR}"),
            ));
        }
    }
    out.extend(singleton_failures(packs));
    out.extend(copies_failures(packs));
    out.extend(cube_mechanic_findings(packs));
    out
}

/// The packs together may not use more copies of a card than the files say are owned: each
/// card's qty is summed over the packs and compared with its `owned_count` (the smallest one any
/// file reports). A card with no `owned_count` in any file is not checked.
fn copies_failures(packs: &[Pack]) -> Vec<Failure> {
    let mut used: BTreeMap<&str, (u32, Option<u32>, Vec<&str>)> = BTreeMap::new();
    for p in packs {
        for l in &p.lines {
            let e = used.entry(&l.card_id).or_default();
            e.0 += l.qty;
            if let Some(o) = p.cards.get(&l.card_id).and_then(|c| c.owned) {
                e.1 = Some(e.1.map_or(o, |m| m.min(o)));
            }
            if !e.2.contains(&p.name.as_str()) {
                e.2.push(&p.name);
            }
        }
    }
    used.into_iter()
        .filter_map(|(id, (qty, owned, in_packs))| {
            let owned = owned.filter(|o| qty > *o)?;
            Some(fail(
                "cube-copies",
                format!(
                    "'{id}' is used {qty} times across the packs ({}) but {owned} owned",
                    in_packs.join(", ")
                ),
            ))
        })
        .collect()
}

/// A card's title and subtitle: the identity the singleton rule counts, so printings of one card
/// are one and a different subtitle is a different card.
fn identity(c: &Card) -> String {
    match c.subtitle.as_deref() {
        Some(s) if !s.is_empty() => format!("{} — {s}", c.name),
        _ => c.name.clone(),
    }
}

/// Whether the singleton rule binds this card: every Leader, and every non-base Rare or Legendary.
fn is_singleton(c: &Card) -> bool {
    c.kind == "Leader"
        || (c.kind != "Base" && matches!(c.rarity.as_deref(), Some("Rare" | "Legendary")))
}

/// Refuses a cube whose cards cannot be told apart or sorted by rarity: every non-base card needs a
/// rarity, and every card a `subtitle` member (null for a card with none). The singleton rule would
/// otherwise guess, and a control that cannot check must refuse.
fn require_identity(packs: &[Pack]) -> Result<(), String> {
    for p in packs {
        for l in &p.lines {
            let Some(c) = p.cards.get(&l.card_id) else {
                continue;
            };
            if c.kind != "Base" && c.rarity.is_none() {
                return Err(format!(
                    "pack '{}': card '{}' ('{}') has no rarity; copy the card object from get_deck or get_card as-is",
                    p.name, l.card_id, c.name
                ));
            }
            if c.subtitle.is_none() {
                return Err(format!(
                    "pack '{}': card '{}' ('{}') has no subtitle member; copy the card object from get_deck or get_card as-is",
                    p.name, l.card_id, c.name
                ));
            }
        }
    }
    Ok(())
}

/// Every Leader and every non-base Rare or Legendary is singleton: one card (title and subtitle,
/// printings collapsed) appears in at most one pack. Bases and the other rarities may repeat; the
/// collection's copies are the only limit.
fn singleton_failures(packs: &[Pack]) -> Vec<Failure> {
    let mut seen: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for p in packs {
        for l in &p.lines {
            let Some(c) = p.cards.get(&l.card_id).filter(|c| is_singleton(c)) else {
                continue;
            };
            let in_packs = seen.entry(identity(c)).or_default();
            if !in_packs.contains(&p.name.as_str()) {
                in_packs.push(&p.name);
            }
        }
    }
    seen.into_iter()
        .filter(|(_, in_packs)| in_packs.len() > 1)
        .map(|(name, in_packs)| {
            fail(
                "cube-singleton",
                format!(
                    "'{name}' is in {} packs ({}); Leaders and non-base Rares and Legendaries are singleton",
                    in_packs.len(),
                    in_packs.join(", ")
                ),
            )
        })
        .collect()
}

fn packs_of(doc: &Value) -> Result<Vec<Pack>, String> {
    let (list, shared) = match doc {
        Value::Array(a) => (a, &Value::Null),
        Value::Object(_) => (
            doc["packs"]
                .as_array()
                .ok_or("a cube document is an array of packs or {\"packs\": [...]}")?,
            &doc["cards"],
        ),
        _ => return Err("a cube document is an array of packs or {\"packs\": [...]}".into()),
    };
    list.iter().map(|p| Pack::from_value(p, shared)).collect()
}

/// Reads a cube document (see the crate docs) and checks it.
pub fn check_cube_value(doc: &Value) -> Result<Vec<Failure>, String> {
    let packs = packs_of(doc)?;
    require_identity(&packs)?;
    Ok(check_cube(&packs))
}

/// Only the copy-count rule over any number of packs, for a partial or test build that the
/// 20-pack cube rules would drown in unrelated failures.
pub fn check_copies_value(doc: &Value) -> Result<Vec<Failure>, String> {
    Ok(copies_failures(&packs_of(doc)?))
}

/// Builders for the passing cube and its broken variants, used by this crate's tests.
pub mod fixture {
    use serde_json::{Value, json};

    use super::{EPIC_BASES, Pack};

    /// The 15-card skeleton of a pack: `(type, arena, cost)`. 8 ground, 3 space, 4 spells; the
    /// curve is 1, 4, 4, 3, 2, 1.
    pub const SKELETON: [(&str, &str, u32); 15] = [
        ("Unit", "Ground", 2),
        ("Unit", "Ground", 2),
        ("Unit", "Ground", 3),
        ("Unit", "Ground", 3),
        ("Unit", "Ground", 4),
        ("Unit", "Ground", 4),
        ("Unit", "Ground", 5),
        ("Unit", "Ground", 6),
        ("Unit", "Space", 2),
        ("Unit", "Space", 2),
        ("Unit", "Space", 3),
        ("Event", "", 1),
        ("Event", "", 3),
        ("Upgrade", "", 4),
        ("Event", "", 5),
    ];

    pub fn card(
        name: &str,
        kind: &str,
        cost: u32,
        arena: &str,
        hp: Option<i64>,
        aspects: &[&str],
    ) -> Value {
        json!({
            "name": name, "subtitle": null, "rarity": if kind == "Leader" { "Rare" } else { "Common" },
            "type": kind, "cost": cost,
            "game": {"aspects": aspects, "arenas": if arena.is_empty() { vec![] } else { vec![arena] }, "hp": hp},
        })
    }

    /// A passing pack document of the given alignment and color; `n` keeps card ids distinct.
    pub fn pack(n: usize, alignment: &str, color: &str) -> Value {
        let epic = EPIC_BASES.iter().find(|(c, _)| *c == color).unwrap().1;
        let mut cards = serde_json::Map::new();
        let mut lines = vec![];
        let mut put = |id: String, c: Value| {
            lines.push(json!({"card_id": id, "qty": 1, "zone": "main", "reserved": 1}));
            cards.insert(id, c);
        };
        put(
            format!("L{n}"),
            card(
                &format!("Leader {n}"),
                "Leader",
                5,
                "Ground",
                Some(6),
                &[alignment, color],
            ),
        );
        put(format!("B25-{n}"), {
            let mut b = card(epic, "Base", 0, "", Some(25), &[color]);
            b["game"]["epic_action"] = json!("Epic Action: Draw a card.");
            b
        });
        put(
            format!("B30-{n}"),
            card(
                &format!("{color} Vanilla Base"),
                "Base",
                0,
                "",
                Some(30),
                &[color],
            ),
        );
        for (i, (kind, arena, cost)) in SKELETON.iter().enumerate() {
            put(
                format!("C{n}-{i}"),
                card(
                    &format!("Card {n}-{i}"),
                    kind,
                    *cost,
                    arena,
                    None,
                    &[alignment, color],
                ),
            );
        }
        json!({"deck": {"name": format!("Pack {n}"), "group": "swu-jumpstart-cube"}, "lines": lines, "cards": cards})
    }

    /// The passing cube: 20 packs, 10 of each alignment, 5 per color.
    pub fn cube() -> Vec<Value> {
        let split = [
            ("Command", 3),
            ("Vigilance", 2),
            ("Aggression", 3),
            ("Cunning", 2),
        ];
        let mut packs = vec![];
        for (color, heroes) in split {
            for k in 0..5 {
                let a = if k < heroes { "Heroism" } else { "Villainy" };
                packs.push(pack(packs.len() + 1, a, color));
            }
        }
        packs
    }

    pub fn parsed(v: &Value) -> Pack {
        Pack::from_value(v, &Value::Null).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::fixture::{cube, pack};
    use super::*;

    #[test]
    fn the_skill_lists_exactly_the_digest_card_fields() {
        let skill = include_str!("../../../skills/swu-jumpstart-cube/SKILL.md");
        let line = skill
            .lines()
            .find(|l| l.starts_with("Card fields the checker reads"))
            .expect("SKILL.md lacks the card-field line");
        let listed: Vec<&str> = line
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|t| t.starts_with('/') && !t.contains(".rs"))
            .collect();
        assert_eq!(listed, DIGEST_CARD_FIELDS.to_vec());
    }

    fn leader_guidance() -> &'static str {
        include_str!("../../../skills/swu-jumpstart-cube/SKILL.md")
    }

    #[test]
    fn the_skill_says_why_bases_cannot_supply_a_leaders_force() {
        let skill = leader_guidance();
        let para = skill
            .lines()
            .find(|l| l.starts_with("**Force leaders.**"))
            .expect("SKILL.md lacks the Force leaders paragraph");
        for needle in [
            "never play with it",
            "partner's base is random",
            "at least two non-base Force producers",
            "prefer a leader with no Force cost",
            "draw cards, never its bases",
        ] {
            assert!(para.contains(needle), "Force leaders paragraph omits `{needle}`");
        }
    }

    #[test]
    fn the_archetype_guidance_names_why_a_parasitic_leader_is_parasitic() {
        let skill = leader_guidance();
        let line = skill
            .lines()
            .find(|l| l.starts_with("The archetypes and the leader guidance are advice"))
            .expect("SKILL.md lacks the archetype advice line");
        assert!(line.contains("Grand Inquisitor"), "archetype guidance omits the parasitic example");
        assert!(line.contains("Force leaders"), "archetype guidance does not point at the Force leaders rule");
    }

    #[test]
    fn the_shared_vector_digests_to_its_pinned_value() {
        let lines: Value = serde_json::from_str(VECTOR_LINES).unwrap();
        let cards: Value = serde_json::from_str(VECTOR_CARDS).unwrap();
        assert_eq!(lines_digest(&lines, &cards).as_deref(), Some(VECTOR_DIGEST));
    }

    fn saved_pack() -> Value {
        let mut p = pack(1, "Heroism", "Command");
        p["lines_digest"] = json!(lines_digest(&p["lines"], &p["cards"]).unwrap());
        p
    }

    #[test]
    fn a_changed_card_field_is_refused() {
        assert!(verify_digest(&saved_pack()).is_ok());
        for (ptr, v) in [
            ("/L1/game/aspects", json!(["Heroism", "Cunning"])),
            ("/L1/cost", json!(4)),
            ("/L1/game/hp", json!(7)),
            ("/L1/game/arenas", json!(["Space"])),
            ("/L1/type", json!("Unit")),
            ("/B25-1/game/epic_action", Value::Null),
        ] {
            let mut p = saved_pack();
            *p["cards"].pointer_mut(ptr).unwrap() = v;
            assert!(
                verify_digest(&p).unwrap_err().contains("digest mismatch"),
                "{ptr}"
            );
        }
        let mut p = saved_pack();
        p["cards"]["L1"]["game"]["aspects"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert!(verify_digest(&p).is_ok(), "aspect order is not content");
    }

    #[test]
    fn a_digest_is_order_independent_and_a_changed_qty_is_refused() {
        let mut p = saved_pack();
        assert!(verify_digest(&p).is_ok());
        p["lines"].as_array_mut().unwrap().reverse();
        assert!(verify_digest(&p).is_ok());
        p["lines"][0]["qty"] = json!(2);
        assert!(verify_digest(&p).unwrap_err().contains("digest mismatch"));
    }

    fn rules(doc: &Value) -> Vec<&'static str> {
        check_pack_value(doc)
            .unwrap()
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }

    /// Replaces `f(card)` in the line whose card id starts with `id`.
    fn edit(doc: &mut Value, id: &str, f: impl Fn(&mut Value)) {
        let key = doc["cards"]
            .as_object()
            .unwrap()
            .keys()
            .find(|k| k.starts_with(id))
            .unwrap()
            .clone();
        f(&mut doc["cards"][key]);
    }

    fn remove_line(doc: &mut Value, id: &str) {
        let lines = doc["lines"].as_array_mut().unwrap();
        lines.retain(|l| !l["card_id"].as_str().unwrap().starts_with(id));
    }

    #[test]
    fn a_well_formed_pack_passes() {
        assert_eq!(
            check_pack_value(&pack(1, "Heroism", "Vigilance")).unwrap(),
            vec![]
        );
    }

    #[test]
    fn pack_shape_fails_on_a_missing_card_and_a_second_leader() {
        let mut p = pack(1, "Heroism", "Command");
        remove_line(&mut p, "C1-14");
        assert!(rules(&p).contains(&"shape"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-0", |c| c["type"] = json!("Leader"));
        assert!(rules(&p).contains(&"shape"));
    }

    #[test]
    fn a_sideboard_line_fails_the_shape() {
        let mut p = pack(1, "Villainy", "Cunning");
        p["lines"][3]["zone"] = json!("sideboard");
        assert!(rules(&p).contains(&"shape"));
    }

    #[test]
    fn a_leader_needs_one_alignment_and_one_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "L1", |c| {
            c["game"]["aspects"] = json!(["Heroism", "Command", "Cunning"])
        });
        assert!(rules(&p).contains(&"leader"));
    }

    #[test]
    fn a_neutral_leader_is_rejected_as_neutral() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "L1", |c| {
            c["game"]["aspects"] = json!(["Aggression", "Cunning"]);
            c["game"]["alignment"] = json!("Neutral");
        });
        let fails = check_pack_value(&p).unwrap();
        assert!(
            fails
                .iter()
                .any(|f| f.rule == "leader"
                    && f.message.contains("Neutral leader does not lead a pack")),
            "{fails:?}"
        );
    }

    #[test]
    fn a_colorless_leader_cannot_lead_a_pack() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "L1", |c| c["game"]["aspects"] = json!(["Heroism"]));
        let fails = check_pack_value(&p).unwrap();
        assert!(
            fails.iter().any(|f| f.rule == "leader"
                && f.message.contains("has no color aspect")
                && f.message.contains("pack is defined by its leader's color")),
            "{fails:?}"
        );
    }

    #[test]
    fn two_leaders_report_the_leader_count_not_an_alignment_fault() {
        let mut p = pack(1, "Heroism", "Vigilance");
        edit(&mut p, "C1-0", |c| {
            c["type"] = json!("Leader");
            c["game"]["aspects"] = json!(["Villainy", "Command"]);
        });
        let fails = check_pack_value(&p).unwrap();
        let leader: Vec<&str> = fails
            .iter()
            .filter(|f| f.rule == "leader")
            .map(|f| f.message.as_str())
            .collect();
        assert_eq!(leader, ["the pack has 2 leaders, not 1"]);
    }

    #[test]
    fn an_off_color_base_fails_even_when_the_pack_has_two_leaders() {
        let mut p = pack(1, "Heroism", "Vigilance");
        edit(&mut p, "C1-0", |c| c["type"] = json!("Leader"));
        edit(&mut p, "B30", |c| c["game"]["aspects"] = json!(["Command"]));
        assert!(rules(&p).contains(&"base-aspect"));
    }

    #[test]
    fn bases_must_be_25_and_30_hp() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| c["game"]["hp"] = json!(27));
        assert!(rules(&p).contains(&"base-hp"));
    }

    fn force_base(p: &mut Value) {
        edit(p, "B30", |c| {
            c["game"]["hp"] = json!(28);
            c["game"]["rules_text"] =
                json!("When a friendly Force unit attacks: The Force is with you.");
        });
    }

    #[test]
    fn a_28_hp_base_without_a_force_reason_fails_naming_why() {
        let mut p = pack(1, "Heroism", "Command");
        force_base(&mut p);
        let f = check_pack_value(&p).unwrap();
        let m = f.iter().find(|f| f.rule == "base-hp").expect("base-hp");
        assert!(m.message.contains("neither"), "{}", m.message);
    }

    #[test]
    fn a_28_hp_force_base_passes_with_a_force_consumer_or_unit() {
        let mut p = pack(1, "Heroism", "Command");
        force_base(&mut p);
        edit(&mut p, "C1-0", |c| {
            c["game"]["rules_text"] = json!("Action [use the Force]: Heal 2.")
        });
        assert!(!rules(&p).contains(&"base-hp"), "{:?}", rules(&p));
        let mut p = pack(1, "Heroism", "Command");
        force_base(&mut p);
        edit(&mut p, "C1-0", |c| c["game"]["traits"] = json!(["Force"]));
        assert!(!rules(&p).contains(&"base-hp"));
    }

    #[test]
    fn a_base_must_match_the_leaders_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| c["game"]["aspects"] = json!(["Cunning"]));
        assert!(rules(&p).contains(&"base-aspect"));
    }

    #[test]
    fn the_25_hp_base_must_be_an_epic_base() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Plain Outpost");
            c["game"]["epic_action"] = json!(null);
        });
        assert!(rules(&p).contains(&"base-name"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Plain Outpost");
            c["game"].as_object_mut().unwrap().remove("epic_action");
        });
        assert!(rules(&p).contains(&"base-name"));
    }

    #[test]
    fn a_25_hp_base_without_an_epic_action_is_not_the_epic_base_even_by_name() {
        for name in ["Security Complex", "Mystic Monastery"] {
            let mut p = pack(1, "Heroism", "Command");
            edit(&mut p, "B25", |c| {
                c["name"] = json!(name);
                c["game"]["epic_action"] = json!(null);
            });
            assert!(rules(&p).contains(&"base-name"), "{name}");
        }
    }

    #[test]
    fn any_25_hp_base_with_an_epic_action_is_accepted() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Some Future Base");
            c["game"]["epic_action"] = json!("Epic Action: Draw a card.");
        });
        assert!(rules(&p).is_empty());
    }

    #[test]
    fn a_stub_that_fails_every_rule_reports_every_rule() {
        let mut p = pack(1, "Heroism", "Command");
        p["lines"].as_array_mut().unwrap().truncate(3);
        edit(&mut p, "B25", |c| c["game"]["epic_action"] = json!(null));
        let r = rules(&p);
        for want in ["shape", "base-name", "arena", "curve"] {
            assert!(r.contains(&want), "{want} missing from {r:?}");
        }
    }

    #[test]
    fn any_real_30_hp_vanilla_base_is_accepted() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| {
            c["name"] = json!("Echo Base");
            c["game"]["epic_action"] = json!(null);
        });
        assert!(rules(&p).is_empty());
        edit(&mut p, "B30", |c| {
            c["game"]["epic_action"] = json!("Epic Action: Draw a card.")
        });
        assert!(rules(&p).contains(&"base-name"));
    }

    #[test]
    fn the_base_name_message_names_the_actual_base() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Plain Outpost");
            c["game"]["epic_action"] = json!(null);
        });
        let f = check_pack_value(&p).unwrap();
        assert_eq!(
            f[0].message,
            "the 25-HP Command base must be an epic base, but 'Plain Outpost' has no epic action"
        );
    }

    #[test]
    fn a_draw_card_must_share_the_leaders_alignment_and_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Villainy", "Command"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Heroism", "Cunning"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| c["game"]["aspects"] = json!([]));
        assert!(rules(&p).is_empty(), "a neutral card is on every aspect");
    }

    #[test]
    fn an_alignment_only_card_is_accepted_and_the_other_alignment_rejected() {
        let mut p = fixture::pack(1, "Villainy", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Villainy"])
        });
        assert!(rules(&p).is_empty(), "Villainy with no color, as AT-ST");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Heroism"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
    }

    #[test]
    fn a_same_color_card_with_no_alignment_is_accepted_and_an_off_color_one_rejected() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Command"])
        });
        assert!(
            rules(&p).is_empty(),
            "no Heroism/Villainy aspect, no penalty"
        );
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Cunning"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
    }

    #[test]
    fn the_arena_split_is_8_ground_3_or_4_space_3_or_4_spells() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-8", |c| c["game"]["arenas"] = json!(["Ground"]));
        assert!(rules(&p).contains(&"arena"));
        let mut p = pack(1, "Heroism", "Command");
        for id in ["C1-11", "C1-12"] {
            edit(&mut p, id, |c| {
                c["type"] = json!("Unit");
                c["game"]["arenas"] = json!(["Space"]);
            });
        }
        assert!(rules(&p).contains(&"arena"));
    }

    #[test]
    fn a_unit_with_no_arena_is_rejected_not_counted_as_ground() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-8", |c| c["game"]["arenas"] = json!([]));
        let msgs: Vec<String> = check_pack_value(&p)
            .unwrap()
            .into_iter()
            .map(|f| f.message)
            .collect();
        assert!(msgs.iter().any(|m| m.contains("no arena")), "{msgs:?}");
    }

    #[test]
    fn the_curve_holds_each_cost_to_its_range() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-0", |c| c["cost"] = json!(1));
        edit(&mut p, "C1-1", |c| c["cost"] = json!(1));
        assert!(rules(&p).contains(&"curve"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-7", |c| c["cost"] = json!(4));
        assert!(rules(&p).contains(&"curve"), "no six-drop");
    }

    #[test]
    fn the_6_plus_slot_must_be_a_unit() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-7", |c| c["type"] = json!("Event"));
        let msgs: Vec<String> = check_pack_value(&p)
            .unwrap()
            .into_iter()
            .map(|f| f.message)
            .collect();
        assert!(
            msgs.contains(&"the 6+ slot must be a unit; Card 1-7 is an event".to_string()),
            "{msgs:?}"
        );
        assert!(!rules(&pack(1, "Heroism", "Command")).contains(&"curve"));
    }

    #[test]
    fn a_copy_not_reserved_fails() {
        let mut p = pack(1, "Heroism", "Command");
        p["lines"][4]["reserved"] = json!(0);
        assert!(rules(&p).contains(&"reserved"));
    }

    #[test]
    fn a_line_with_no_card_object_fails() {
        let mut p = pack(1, "Heroism", "Command");
        p["cards"].as_object_mut().unwrap().remove("L1");
        assert!(rules(&p).contains(&"unknown-card"));
    }

    fn cube_rules(packs: Vec<Value>) -> Vec<&'static str> {
        check_cube_value(&Value::Array(packs))
            .unwrap()
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }

    #[test]
    fn a_well_formed_cube_passes() {
        assert_eq!(cube_rules(cube()), Vec::<&str>::new());
    }

    #[test]
    fn a_cube_of_19_packs_fails() {
        let mut c = cube();
        c.pop();
        assert!(cube_rules(c).contains(&"cube-size"));
    }

    #[test]
    fn a_cube_needs_ten_of_each_alignment() {
        let mut c = cube();
        c[0] = pack(1, "Villainy", "Command");
        assert!(cube_rules(c).contains(&"cube-alignment"));
    }

    #[test]
    fn a_cube_needs_five_of_each_base_pair() {
        let mut c = cube();
        c[0] = pack(1, "Heroism", "Cunning");
        assert!(cube_rules(c).contains(&"cube-bases"));
    }

    fn set(doc: &mut Value, id: &str, name: &str, subtitle: Value, rarity: &str) {
        edit(doc, id, |c| {
            c["name"] = json!(name);
            c["subtitle"] = subtitle.clone();
            c["rarity"] = json!(rarity);
        });
    }

    fn singleton(packs: Vec<Value>) -> Vec<String> {
        check_cube_value(&Value::Array(packs))
            .unwrap()
            .into_iter()
            .filter(|f| f.rule == "cube-singleton")
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn a_leader_in_two_packs_fails_naming_both() {
        let mut c = cube();
        set(
            &mut c[0],
            "L",
            "Grand Inquisitor",
            json!("Hunting the Jedi"),
            "Rare",
        );
        set(
            &mut c[1],
            "L",
            "Grand Inquisitor",
            json!("Hunting the Jedi"),
            "Rare",
        );
        c[1]["cards"]["L2"]["rarity"] = json!("Legendary");
        let m = singleton(c);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(
            m[0].contains("'Grand Inquisitor — Hunting the Jedi'")
                && m[0].contains("Pack 1, Pack 2")
        );
    }

    #[test]
    fn a_leader_and_a_unit_sharing_a_title_are_distinct_cards() {
        let mut c = cube();
        set(
            &mut c[0],
            "L",
            "Grand Inquisitor",
            json!("Hunting the Jedi"),
            "Rare",
        );
        set(
            &mut c[1],
            "C2-5",
            "Grand Inquisitor",
            json!("Fallen Jedi"),
            "Rare",
        );
        assert_eq!(singleton(c), Vec::<String>::new());
    }

    #[test]
    fn a_rare_draw_card_in_two_packs_fails() {
        let mut c = cube();
        set(&mut c[0], "C1-5", "Rare Unit", json!(null), "Rare");
        set(&mut c[3], "C4-5", "Rare Unit", json!(null), "Rare");
        let m = singleton(c);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("'Rare Unit'") && m[0].contains("Pack 1, Pack 4"));
    }

    #[test]
    fn a_common_or_uncommon_draw_card_in_two_packs_is_ok() {
        for rarity in ["Common", "Uncommon", "Special"] {
            let mut c = cube();
            set(&mut c[0], "C1-5", "Filler", json!(null), rarity);
            set(&mut c[3], "C4-5", "Filler", json!(null), rarity);
            assert_eq!(singleton(c), Vec::<String>::new(), "{rarity}");
        }
    }

    #[test]
    fn a_legendary_draw_card_in_two_packs_fails() {
        let mut c = cube();
        set(&mut c[0], "C1-5", "Legend", json!(null), "Legendary");
        set(&mut c[3], "C4-5", "Legend", json!(null), "Legendary");
        assert_eq!(singleton(c).len(), 1);
    }

    #[test]
    fn two_printings_of_one_card_collapse_to_one() {
        let mut c = cube();
        set(&mut c[0], "L", "Twin", json!("Printing"), "Rare");
        set(&mut c[1], "L", "Twin", json!("Printing"), "Legendary");
        let ids: Vec<String> = [0, 1]
            .iter()
            .map(|i| {
                c[*i]["cards"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .find(|k| k.starts_with('L'))
                    .unwrap()
                    .clone()
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_eq!(singleton(c).len(), 1);
    }

    #[test]
    fn a_card_with_no_rarity_refuses() {
        let mut c = cube();
        edit(&mut c[0], "C1-5", |x| {
            x.as_object_mut().unwrap().remove("rarity");
        });
        let e = check_cube_value(&Value::Array(c)).unwrap_err();
        assert!(e.contains("no rarity") && e.contains("Card 1-5"), "{e}");
    }

    #[test]
    fn a_card_with_no_subtitle_member_refuses() {
        let mut c = cube();
        edit(&mut c[0], "L", |x| {
            x.as_object_mut().unwrap().remove("subtitle");
        });
        let e = check_cube_value(&Value::Array(c)).unwrap_err();
        assert!(e.contains("no subtitle"), "{e}");
    }

    #[test]
    fn a_base_repeated_across_packs_is_exempt() {
        // the fixture already repeats each color's bases in five packs
        assert!(!cube_rules(cube()).contains(&"cube-singleton"));
    }

    #[test]
    fn pack_mode_ignores_singleton() {
        let p = pack(1, "Heroism", "Command");
        assert!(
            check_pack_value(&p)
                .unwrap()
                .iter()
                .all(|f| f.rule != "cube-singleton")
        );
    }

    #[test]
    fn a_broken_pack_inside_a_cube_is_named() {
        let mut c = cube();
        edit(&mut c[4], "C5-3", |x| x["cost"] = json!(1));
        let f = check_cube_value(&Value::Array(c)).unwrap();
        assert!(f.iter().any(|f| f.message.contains("pack 'Pack 5'")));
    }

    #[test]
    fn a_cube_document_may_share_its_cards() {
        let mut packs = cube();
        let mut shared = serde_json::Map::new();
        for p in &mut packs {
            for (k, v) in p["cards"].as_object().unwrap().clone() {
                shared.insert(k, v);
            }
            p.as_object_mut().unwrap().remove("cards");
        }
        let doc = json!({"packs": packs, "cards": shared});
        assert_eq!(check_cube_value(&doc).unwrap(), vec![]);
    }

    fn force_leader(p: &mut Value) {
        edit(p, "L1", |c| {
            c["game"]["rules_text"] = json!("Action [Exhaust, use the Force]: Draw a card.\nEpic Action: Deploy.\nDeployed:\nOn Attack: Draw.")
        });
    }

    #[test]
    fn a_leader_spending_an_unproduced_resource_fails_and_its_draw_consumers_warn() {
        let mut p = pack(1, "Heroism", "Command");
        force_leader(&mut p);
        edit(&mut p, "C1-0", |c| {
            c["game"]["rules_text"] = json!("Action [use the Force]: Heal 2.")
        });
        let f = check_pack_value(&p).unwrap();
        assert!(f.iter().any(|f| f.rule == "leader-mechanic" && !f.warn), "{f:?}");
        assert!(f.iter().any(|f| f.rule == "pack-mechanic" && f.warn), "{f:?}");
        let (code, text) = render(Ok(f));
        assert_eq!(code, 1);
        assert!(text.contains("FAIL leader-mechanic") && text.contains("WARN pack-mechanic"));
    }

    #[test]
    fn a_producer_in_the_pack_covers_the_consumer_and_the_deployed_side_is_ignored() {
        let mut p = pack(1, "Heroism", "Command");
        force_leader(&mut p);
        edit(&mut p, "C1-0", |c| {
            c["game"]["rules_text"] = json!("The Force is with you.")
        });
        assert_eq!(check_pack_value(&p).unwrap(), vec![]);
        let mut q = pack(1, "Heroism", "Command");
        edit(&mut q, "L1", |c| {
            c["game"]["rules_text"] = json!("Deployed:\nOn Attack: use the Force.")
        });
        assert_eq!(check_pack_value(&q).unwrap(), vec![]);
    }

    #[test]
    fn warnings_alone_render_ok_with_exit_zero() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-0", |c| {
            c["game"]["rules_text"] = json!("use the Force: heal")
        });
        let (code, text) = render(check_pack_value(&p));
        assert_eq!(code, 0);
        assert!(text.starts_with("ok\nWARN pack-mechanic"), "{text}");
    }

    #[test]
    fn a_cube_with_consumers_and_no_producer_fails_and_reports_partner_share() {
        let mut c = cube();
        force_leader(&mut c[0]);
        edit(&mut c[1], "C2-0", |x| {
            x["game"]["rules_text"] = json!("use the Force")
        });
        let f = check_cube_value(&Value::Array(c.clone())).unwrap();
        assert!(
            f.iter().any(|f| f.rule == "cube-mechanic"
                && f.message == "force consumed by 2 cards, produced by 0"),
            "{f:?}"
        );
        edit(&mut c[5], "C6-0", |x| {
            x["game"]["rules_text"] = json!("The Force is with you.")
        });
        let f = check_cube_value(&Value::Array(c)).unwrap();
        assert!(f.iter().all(|f| f.rule != "cube-mechanic"));
        assert!(
            f.iter().any(|f| f.rule == "mechanic-report" && f.message.contains("1 of 19")),
            "{f:?}"
        );
    }

    #[test]
    fn catalog_mechanics_replace_the_text_patterns() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "L1", |c| {
            c["game"]["mechanics"] = json!({"consumes": ["force"], "produces": []})
        });
        let f = check_pack_value(&p).unwrap();
        assert!(f.iter().any(|f| f.rule == "leader-mechanic"), "{f:?}");
    }
}
