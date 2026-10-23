//! `swu-cube-check allocate`: plan copy allocation across the whole cube before any pack is built.
//!
//! Heroism-only, Villainy-only and no-aspect cards fit packs of any color, so one pack's picks can
//! use up copies another color's packs depend on. The pack checker sees one pack at a time; this
//! assigns every pack's slots at once (max-flow), so a plan either reserves each copy to a pack or
//! names the card and the packs competing for it.
//!
//! Input: `{"owned": {"<card id>": copies}, "packs": [{"name": "Pack 1", "slots": 15,
//! "eligible": ["<card id>", ...]}]}`. A pack takes at most one copy of a card.
//!
//! `picks` are a pack's draw cards only (at most 15), never the leader or bases: a pick row typed
//! Leader or Base, or more than 15 picks in a 15-slot pack, is refused.
//!
//! Eligibility says what a pack could take; it never reserves. A pack may also carry `"picks"`
//! (its planned cards) and `"candidates"` (cards it may still choose from), card ids or rows. Only
//! a card in one of those, owned in fewer copies than packs hold it there, is contested and
//! reserved; a pack with neither wants nothing reserved. Packs that both want a scarce card are
//! resolved in pack order.
//!
//! An eligible entry may also be a `search_cards` row (`id`, `type`, `cost`, `game.arenas`). A pack
//! with any such row, and 15 slots, is a draw-card pack: its pool must be able to satisfy the pack
//! skeleton (8 ground units, 3-4 space units, 3-4 events/upgrades, and the cost curve), or the
//! shortfall is named. The plan invents no picks: without `fill`, a pack with empty picks wants
//! nothing reserved, but its slots are still placed from its pool, so shared pools that run dry
//! report a shortfall naming the competing packs. With `fill` each such pack is given 15 cards that
//! meet the skeleton; they count as filled (never as picks) and are reserved and contested like
//! picks. Bare card ids in such a pack carry no type, so they are never chosen for it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;

use crate::{CURVE, DRAW_CARDS, Failure, GROUND_UNITS, fail};

pub struct Plan {
    /// card id -> the packs it is reserved to, in pack order.
    /// Only contested cards appear: the plan picks a pack's draw list only under `fill`.
    pub reservations: BTreeMap<String, Vec<String>>,
    /// card id -> (copies owned, packs wanting it), for every contested card.
    pub contested: BTreeMap<String, (usize, usize)>,
    pub failures: Vec<Failure>,
    /// Names of packs that are saved decks (`"saved": true`): they hold their copies outright.
    pub saved: BTreeSet<String>,
    /// One entry per pack, by pack name.
    pub summary: Vec<PackSummary>,
}

pub struct PackSummary {
    pub name: String,
    /// Slots the pack has.
    pub slots: usize,
    /// Cards the pack's author named in `picks`.
    pub picks: usize,
    /// Cards `--fill` chose for the pack (0 without it); never counted as picks.
    pub filled: usize,
    /// Cards the pack is eligible for that have an owned copy.
    pub pool: usize,
    /// Whether the pack skeleton was checked: `Some(true)` met, `Some(false)` unmet, `None` not
    /// checked (not a 15-slot pack, or only bare card ids that carry no type).
    pub skeleton: Option<bool>,
    /// Contested cards reserved to this pack.
    pub reserved: usize,
    /// Slots the plan could not fill.
    pub short: usize,
    /// Contested cards the pack could use that are reserved to other packs: (card, those packs).
    pub lost: Vec<(String, Vec<String>)>,
}

/// Cards named per pack in the default summary; `verbose` has the full reserve lines.
const LOST_SHOWN: usize = 2;

/// The text `allocate` prints for `plan` and its exit code: 0 when every pick holds, 1 when a
/// rule fails.
pub fn render(plan: &Plan, verbose: bool) -> (i32, String) {
    let mut out = String::new();
    macro_rules! say {
        ($($a:tt)*) => {{
            out.push_str(&format!($($a)*));
            out.push('\n');
        }};
    }
    if plan.reservations.is_empty() && plan.failures.is_empty() {
        let bare = plan
            .summary
            .iter()
            .filter(|s| s.picks == 0 && s.filled == 0)
            .count();
        if bare == 0 {
            say!("no contested cards");
        } else {
            say!(
                "no contested cards among picks ({bare} pack(s) have no picks; contention among their pools is not measured unless --fill)"
            );
        }
    }
    for s in &plan.summary {
        let lost: Vec<String> = s
            .lost
            .iter()
            .take(LOST_SHOWN)
            .map(|(c, to)| {
                if !to.is_empty() && to.iter().all(|t| plan.saved.contains(t)) {
                    format!("{c} held by {}", to.join("+"))
                } else {
                    format!("{c} to {}", to.join("+"))
                }
            })
            .collect();
        let more = s.lost.len().saturating_sub(LOST_SHOWN);
        say!(
            "{}: {} of {} picks{}, eligible pool of {}, skeleton {}, contested reserved {}, short {}, lost {}{}",
            s.name,
            s.picks,
            s.slots,
            if s.filled > 0 {
                format!(", {} filled", s.filled)
            } else {
                String::new()
            },
            s.pool,
            match s.skeleton {
                Some(true) => "ok",
                Some(false) => "unmet",
                None => "not checked",
            },
            s.reserved,
            s.short,
            s.lost.len(),
            if lost.is_empty() {
                String::new()
            } else {
                format!(
                    " ({}{})",
                    lost.join(", "),
                    if more > 0 {
                        format!(", +{more} more")
                    } else {
                        String::new()
                    }
                )
            }
        );
    }
    if verbose {
        for (card, packs) in &plan.reservations {
            let (copies, wanting) = plan.contested[card];
            say!(
                "reserve {card}: {} (contested: {copies} cop{}, {wanting} packs)",
                packs.join(", "),
                if copies == 1 { "y" } else { "ies" }
            );
        }
    }
    for f in &plan.failures {
        say!("FAIL {f}");
    }
    if plan.failures.is_empty() {
        say!("ok");
    }
    (
        i32::from(!plan.failures.is_empty()),
        out.trim_end().to_string(),
    )
}

pub fn allocate_value<'a>(doc: &'a Value, fill: bool) -> Result<Plan, String> {
    let owned = doc
        .get("owned")
        .and_then(Value::as_object)
        .ok_or("allocate: `owned` must be an object of card id -> copies")?;
    let packs = doc
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("allocate: `packs` must be an array")?;
    let mut copies: BTreeMap<&str, usize> = BTreeMap::new();
    for (id, n) in owned {
        let n = n
            .as_u64()
            .ok_or(format!("allocate: copies of {id} is not a number"))?;
        copies.insert(id, n as usize);
    }
    // Packs are taken in name order, so the result does not depend on their order in the input.
    let mut named: Vec<(String, &Value)> = packs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(format!("Pack {}", i + 1));
            (name, p)
        })
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(w) = named.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(format!("allocate: two packs are named {}", w[0].0));
    }

    // The cube's singleton set (Leaders; non-base Rares and Legendaries) is one card per identity,
    // wherever it is owned more than once: it is held by one pack at most.
    let mut identities: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for (_, p) in &named {
        for c in p
            .get("eligible")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = c
                .get("id")
                .or_else(|| c.get("card_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if is_singleton_row(c) {
                let name = c["name"].as_str().unwrap_or(id);
                let key = match c["subtitle"].as_str() {
                    Some(s) if !s.is_empty() => format!("{name} — {s}"),
                    _ => name.to_string(),
                };
                identities.entry(key).or_default().insert(id);
            }
        }
    }
    let mut canon: BTreeMap<&str, &str> = BTreeMap::new();
    for ids in identities.values() {
        let first = *ids.iter().next().unwrap();
        let owned_any = ids.iter().any(|i| copies.get(i).copied().unwrap_or(0) > 0);
        for &i in ids {
            canon.insert(i, first);
            copies.remove(i);
        }
        copies.insert(first, usize::from(owned_any));
    }
    let canon_id = |id: &'a str| canon.get(id).copied().unwrap_or(id);
    let copies_owned = copies.clone();
    let packs = &named;
    let mut names = Vec::new();
    let mut slots = Vec::new();
    let mut eligible: Vec<Vec<&str>> = Vec::new();
    let mut skel: Vec<Option<Vec<(&str, Meta)>>> = Vec::new();
    let mut wants: Vec<BTreeSet<&str>> = Vec::new();
    let mut pick_n: Vec<usize> = Vec::new();
    let mut pick_set: Vec<BTreeSet<&str>> = Vec::new();
    let mut saved: Vec<bool> = Vec::new();
    let mut pool_n: Vec<usize> = Vec::new();
    for (i, (name, p)) in packs.iter().enumerate() {
        names.push(name.clone());
        slots.push(
            p.get("slots")
                .and_then(Value::as_u64)
                .ok_or("allocate: a pack lacks `slots`")? as usize,
        );
        let e = p
            .get("eligible")
            .and_then(Value::as_array)
            .ok_or("allocate: a pack lacks `eligible`")?;
        let mut want = BTreeSet::new();
        for key in ["picks", "candidates"] {
            for c in p.get(key).and_then(Value::as_array).into_iter().flatten() {
                let id = c
                    .as_str()
                    .or_else(|| c.get("id").or_else(|| c.get("card_id"))?.as_str())
                    .ok_or(format!(
                        "allocate: `{key}` entries are card ids or card rows with an `id`"
                    ))?;
                want.insert(canon_id(id));
            }
        }
        let picked: BTreeSet<&str> = p
            .get("picks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().or_else(|| c.get("id")?.as_str()))
            .map(canon_id)
            .collect();
        let name = p.get("name").and_then(Value::as_str).unwrap_or("a pack");
        let non_draw = |c: &Value| matches!(c["type"].as_str(), Some("Leader" | "Base"));
        let bad = p
            .get("picks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(e.iter().filter(|r| {
                r.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| picked.contains(canon_id(id)))
            }))
            .find(|c| non_draw(c));
        if let Some(c) = bad {
            let id = c["id"].as_str().unwrap_or("?");
            return Err(format!(
                "allocate: {name} picks {id}, a {}; picks are the {DRAW_CARDS} draw cards only, never the leader or bases",
                c["type"].as_str().unwrap_or("Leader").to_lowercase()
            ));
        }
        if slots[i] == DRAW_CARDS as usize && picked.len() > slots[i] {
            return Err(format!(
                "allocate: {name} has {} picks for {} slots; picks are the {DRAW_CARDS} draw cards only, never the leader or bases",
                picked.len(),
                slots[i]
            ));
        }
        pick_n.push(picked.len());
        pick_set.push(picked);
        saved.push(p.get("saved").and_then(Value::as_bool).unwrap_or(false));
        wants.push(want);
        let mut ids = BTreeSet::new();
        let mut rows: BTreeMap<&str, Option<Meta>> = BTreeMap::new();
        for c in e {
            if let Some(id) = c.as_str() {
                let id = canon_id(id);
                ids.insert(id);
                rows.entry(id).or_insert(None);
                continue;
            }
            let id = c
                .get("id")
                .or_else(|| c.get("card_id"))
                .and_then(Value::as_str)
                .ok_or("allocate: eligible entries are card ids or card rows with an `id`")?;
            let id = canon_id(id);
            ids.insert(id);
            rows.insert(id, Meta::from_row(c));
        }
        let skeleton = slots[i] == DRAW_CARDS as usize && e.iter().any(Value::is_object);
        skel.push(skeleton.then(|| {
            rows.iter()
                .filter_map(|(id, m)| m.map(|m| (*id, m)))
                .filter(|(id, _)| copies.get(id).copied().unwrap_or(0) > 0)
                .collect()
        }));
        let usable: Vec<&str> = ids
            .into_iter()
            .filter(|c| copies.get(c).copied().unwrap_or(0) > 0)
            .collect();
        pool_n.push(usable.len());
        eligible.push(if skeleton && fill { Vec::new() } else { usable });
    }

    let mut failures = Vec::new();
    let mut unsolved_packs: BTreeSet<usize> = BTreeSet::new();
    let mut fixed: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    let skeleton_packs: Vec<usize> = (0..packs.len()).filter(|&p| skel[p].is_some()).collect();
    let mut filled = vec![0usize; packs.len()];
    if !fill {
        // Without --fill the plan invents no cards: a skeleton pack is only checked to be possible
        // from its own pool, and its slots are placed like any pack's, to show shared pools run dry.
        for &p in &skeleton_packs {
            let pool = skel[p].as_deref().unwrap_or(&[]);
            let alone = pool.iter().map(|(id, _)| (*id, 1)).collect();
            let weight = pool.iter().map(|(id, _)| (*id, 0.0)).collect();
            if solve_pack(pool, &alone, &weight).is_empty() {
                unsolved_packs.insert(p);
                failures.push(fail(
                    "allocation",
                    skeleton_shortfall(&names[p], pool, false),
                ));
                eligible[p].clear();
            }
        }
    } else if !skeleton_packs.is_empty() {
        let pools: Vec<&[(&str, Meta)]> =
            skel.iter().map(|s| s.as_deref().unwrap_or(&[])).collect();
        let (picks, unsolved) = plan_skeletons(&skeleton_packs, &pools, &copies);
        for (p, cards) in picks {
            for c in cards {
                *copies.get_mut(c).unwrap() -= 1;
                fixed.entry(c).or_default().insert(p);
                wants[p].insert(c);
                filled[p] += 1;
            }
        }
        for p in unsolved {
            unsolved_packs.insert(p);
            failures.push(fail(
                "allocation",
                skeleton_shortfall(&names[p], pools[p], skeleton_packs.len() > 1),
            ));
        }
    }

    // A card is contested when more packs pick, consider or were filled with it than there are
    // copies; eligibility alone never counts.
    let mut demand: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (p, want) in wants.iter().enumerate() {
        for &c in want {
            if copies_owned.contains_key(c) {
                demand.entry(c).or_default().push(p);
            }
        }
    }
    demand.retain(|c, ps| ps.len() > copies_owned[c]);
    let contested: BTreeMap<String, (usize, usize)> = demand
        .iter()
        .map(|(c, ps)| (c.to_string(), (copies_owned[c], ps.len())))
        .collect();

    // held[p] = cards assigned to pack p; users[c] = packs holding c.
    let mut held: Vec<BTreeSet<&str>> = vec![BTreeSet::new(); packs.len()];
    let mut users: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    // Scarcest pool first; ties by name (the pack index is its name's rank).
    let mut scarcest: Vec<usize> = (0..packs.len()).collect();
    scarcest.sort_by_key(|&p| (eligible[p].len(), p));
    for p in scarcest {
        while held[p].len() < slots[p] {
            if !augment(p, &eligible, &copies, &mut held, &mut users) {
                break;
            }
        }
    }

    for p in 0..packs.len() {
        if unsolved_packs.contains(&p) || (fill && skel[p].is_some()) {
            continue;
        }
        let short = slots[p] - held[p].len();
        if short == 0 {
            continue;
        }
        // Everything the stuck pack could displace: walk cards it wants and the packs holding them.
        let mut seen_packs = BTreeSet::from([p]);
        let mut contested: BTreeSet<&str> = BTreeSet::new();
        let mut queue = VecDeque::from([p]);
        while let Some(q) = queue.pop_front() {
            for &c in &eligible[q] {
                if held[q].contains(c) {
                    continue;
                }
                contested.insert(c);
                for &r in users.get(c).into_iter().flatten() {
                    if seen_packs.insert(r) {
                        queue.push_back(r);
                    }
                }
            }
        }
        let cards: Vec<String> = contested
            .iter()
            .map(|c| format!("{c} ({} owned)", copies[c]))
            .collect();
        let competing: Vec<&str> = seen_packs
            .iter()
            .filter(|&&q| q != p)
            .map(|&q| names[q].as_str())
            .collect();
        failures.push(fail(
            "allocation",
            format!(
                "{} is {short} card(s) short of {} even with every copy reassigned; the cards it can use are all spoken for: {}; competing packs: {}",
                names[p],
                slots[p],
                cards.join(", "),
                competing.join(", ")
            ),
        ));
    }
    for (c, ps) in fixed {
        users.entry(c).or_default().extend(ps);
    }
    // Reserve contested copies only. A copy the plan left unheld goes to the wanting pack with the
    // fewest reservations so far, then by name, so no pack is favoured by where it sits.
    let mut reservations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut count = vec![0usize; packs.len()];
    let mut reserved_to: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for c in demand.keys() {
        let mut ps = users.remove(c).unwrap_or_default();
        ps.retain(|p| demand[c].contains(p));
        // A saved deck keeps its copy: it is already reserved on the site and cannot be taken back.
        let firm: BTreeSet<usize> = demand[c]
            .iter()
            .copied()
            .filter(|&p| saved[p])
            .take(copies_owned[c])
            .collect();
        if !firm.is_empty() {
            ps = firm;
        }
        for &p in &ps {
            count[p] += 1;
        }
        reserved_to.insert(c, ps);
    }
    for (c, wanting) in &demand {
        let ps = reserved_to.get_mut(c).unwrap();
        while ps.len() < copies_owned[c] {
            let Some(&p) = wanting
                .iter()
                .filter(|p| !ps.contains(p))
                .min_by_key(|&&p| (count[p], p))
            else {
                break;
            };
            count[p] += 1;
            ps.insert(p);
        }
    }
    let mut summary: Vec<PackSummary> = (0..packs.len())
        .map(|p| PackSummary {
            name: names[p].clone(),
            slots: slots[p],
            picks: pick_n[p],
            filled: if unsolved_packs.contains(&p) {
                0
            } else {
                filled[p]
            },
            pool: pool_n[p],
            skeleton: skel[p].as_ref().map(|_| !unsolved_packs.contains(&p)),
            reserved: count[p],
            short: if unsolved_packs.contains(&p) {
                slots[p]
            } else if fill && skel[p].is_some() {
                0
            } else {
                slots[p] - held[p].len()
            },
            lost: Vec::new(),
        })
        .collect();
    for (c, wanting) in &demand {
        let holders = &reserved_to[c];
        for &p in wanting.iter().filter(|p| !holders.contains(p)) {
            summary[p].lost.push((
                c.to_string(),
                holders.iter().map(|&q| names[q].clone()).collect(),
            ));
        }
    }
    // A pack that loses a card it named in `picks` to a saved deck cannot be saved as written: the
    // saved deck's copy is already reserved on the site. A loss to an unsaved pack is the plan's own
    // assignment, which the pack is told to follow.
    for (p, s) in summary.iter().enumerate() {
        if saved[p] {
            continue;
        }
        for (c, to) in s
            .lost
            .iter()
            .filter(|(c, _)| pick_set[p].contains(c.as_str()))
        {
            let held: Vec<&str> = to
                .iter()
                .filter(|t| names.iter().position(|n| n == *t).is_some_and(|q| saved[q]))
                .map(String::as_str)
                .collect();
            if !held.is_empty() {
                failures.push(fail(
                    "allocation",
                    format!(
                        "{} picks {c}, which is unavailable — held by {}",
                        s.name,
                        held.join(", ")
                    ),
                ));
            }
        }
    }
    for (c, ps) in &reserved_to {
        reservations.insert(
            c.to_string(),
            ps.iter().map(|&p| names[p].clone()).collect(),
        );
    }
    Ok(Plan {
        reservations,
        contested,
        failures,
        saved: (0..packs.len())
            .filter(|&p| saved[p])
            .map(|p| names[p].clone())
            .collect(),
        summary,
    })
}

/// Whether the cube's singleton rule binds this search_cards row: every Leader, and every non-base
/// Rare or Legendary (the rule `swu-cube-check cube` enforces as cube-singleton).
fn is_singleton_row(c: &Value) -> bool {
    match c["type"].as_str() {
        Some("Leader") => true,
        Some("Base") | None => false,
        Some(_) => matches!(c["rarity"].as_str(), Some("Rare" | "Legendary")),
    }
}

/// What the skeleton needs to know of a card: its arena/type class and its cost bucket.
#[derive(Clone, Copy)]
struct Meta {
    /// 0 ground unit, 1 space unit, 2 event or upgrade.
    class: usize,
    /// Cost 1..6+ as 0..5.
    bucket: usize,
}

impl Meta {
    fn from_row(v: &Value) -> Option<Meta> {
        let arenas: Vec<&str> = v["game"]["arenas"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let class = match v["type"].as_str()? {
            "Unit" if arenas.contains(&"Space") => 1,
            "Unit" if arenas.contains(&"Ground") => 0,
            "Unit" | "Leader" | "Base" => return None,
            _ => 2,
        };
        let cost = u32::try_from(v["cost"].as_u64()?).ok()?;
        Some(Meta {
            class,
            bucket: (cost.clamp(1, 6) - 1) as usize,
        })
    }
}

const CLASS_NAMES: [&str; 3] = ["ground units", "space units", "events or upgrades"];
const CLASS_PROFILES: [[usize; 3]; 2] =
    [[GROUND_UNITS as usize, 3, 4], [GROUND_UNITS as usize, 4, 3]];

/// Every cost-curve profile summing to a draw pack.
fn bucket_profiles() -> Vec<[usize; 6]> {
    fn go(i: usize, left: usize, cur: &mut [usize; 6], out: &mut Vec<[usize; 6]>) {
        if i == 6 {
            if left == 0 {
                out.push(*cur);
            }
            return;
        }
        for n in CURVE[i].0 as usize..=CURVE[i].1 as usize {
            if n <= left {
                cur[i] = n;
                go(i + 1, left - n, cur, out);
            }
        }
    }
    let mut out = Vec::new();
    go(0, DRAW_CARDS as usize, &mut [0; 6], &mut out);
    out
}

/// Max flow over source -> class -> bucket -> sink; returns the cards per (class, bucket) cell
/// when every target is met.
fn cell_flow(
    cells: &[[usize; 6]; 3],
    classes: [usize; 3],
    buckets: [usize; 6],
) -> Option<[[usize; 6]; 3]> {
    const N: usize = 11;
    let mut cap = [[0i64; N]; N];
    for c in 0..3 {
        cap[0][1 + c] = classes[c] as i64;
        for b in 0..6 {
            cap[1 + c][4 + b] = cells[c][b] as i64;
        }
    }
    for b in 0..6 {
        cap[4 + b][10] = buckets[b] as i64;
    }
    let want: usize = classes.iter().sum();
    let mut flow = 0;
    loop {
        let mut prev = [usize::MAX; N];
        prev[0] = 0;
        let mut queue = VecDeque::from([0]);
        while let Some(u) = queue.pop_front() {
            for v in 0..N {
                if prev[v] == usize::MAX && cap[u][v] > 0 {
                    prev[v] = u;
                    queue.push_back(v);
                }
            }
        }
        if prev[10] == usize::MAX {
            break;
        }
        let (mut v, mut room) = (10, i64::MAX);
        while v != 0 {
            room = room.min(cap[prev[v]][v]);
            v = prev[v];
        }
        v = 10;
        while v != 0 {
            cap[prev[v]][v] -= room;
            cap[v][prev[v]] += room;
            v = prev[v];
        }
        flow += room as usize;
    }
    if flow != want {
        return None;
    }
    let mut out = [[0; 6]; 3];
    for c in 0..3 {
        for b in 0..6 {
            out[c][b] = cap[4 + b][1 + c] as usize;
        }
    }
    Some(out)
}

/// Up to `CANDIDATES` distinct 15-card selections meeting the skeleton from `pool`, taking only
/// cards with a copy left, the ones using the least contested cards (`weight`: lower is freer) first.
fn solve_pack<'a>(
    pool: &[(&'a str, Meta)],
    copies: &BTreeMap<&str, usize>,
    weight: &BTreeMap<&str, f64>,
) -> Vec<Vec<&'a str>> {
    let mut cells: Vec<Vec<Vec<&'a str>>> = vec![vec![Vec::new(); 6]; 3];
    for (id, m) in pool {
        if copies.get(id).copied().unwrap_or(0) > 0 {
            cells[m.class][m.bucket].push(id);
        }
    }
    for row in &mut cells {
        for cell in row {
            cell.sort_by(|a, b| weight[a].total_cmp(&weight[b]).then(a.cmp(b)));
        }
    }
    let mut sizes = [[0; 6]; 3];
    for c in 0..3 {
        for b in 0..6 {
            sizes[c][b] = cells[c][b].len();
        }
    }
    // The 6+ slot is a unit: an event or upgrade never fills it.
    sizes[2][5] = 0;
    let mut found: Vec<(f64, Vec<&'a str>)> = Vec::new();
    for classes in CLASS_PROFILES {
        for buckets in bucket_profiles() {
            let Some(take) = cell_flow(&sizes, classes, buckets) else {
                continue;
            };
            let mut picked = Vec::new();
            let mut cost = 0.0;
            for c in 0..3 {
                for b in 0..6 {
                    for id in &cells[c][b][..take[c][b]] {
                        cost += weight[id];
                        picked.push(*id);
                    }
                }
            }
            picked.sort();
            if !found.iter().any(|(_, f)| *f == picked) {
                found.push((cost, picked));
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.truncate(CANDIDATES);
    found.into_iter().map(|(_, p)| p).collect()
}

const CANDIDATES: usize = 8;
const SEARCH_NODES: usize = 4000;

struct Search<'a, 'b> {
    pools: &'b [&'b [(&'a str, Meta)]],
    weight: &'b BTreeMap<&'a str, f64>,
    nodes: usize,
}

impl<'a> Search<'a, '_> {
    /// Depth-first over each pack's candidate selections, within a node budget.
    fn place(
        &mut self,
        order: &[usize],
        left: &mut BTreeMap<&'a str, usize>,
        picks: &mut BTreeMap<usize, Vec<&'a str>>,
    ) -> bool {
        let Some((&p, rest)) = order.split_first() else {
            return true;
        };
        for cards in solve_pack(self.pools[p], left, self.weight) {
            if self.nodes == 0 {
                return false;
            }
            self.nodes -= 1;
            for c in &cards {
                *left.get_mut(c).unwrap() -= 1;
            }
            picks.insert(p, cards.clone());
            if self.place(rest, left, picks) {
                return true;
            }
            picks.remove(&p);
            for c in &cards {
                *left.get_mut(c).unwrap() += 1;
            }
        }
        false
    }
}

/// Assigns every skeleton pack its 15 cards. Packs are placed tightest pool first, each taking its
/// least contested selection; when that strands a pack the search backs up and tries the packs'
/// other selections. Returns the picks and the packs no search could place.
fn plan_skeletons<'a>(
    order_of: &[usize],
    pools: &[&[(&'a str, Meta)]],
    copies: &BTreeMap<&'a str, usize>,
) -> (BTreeMap<usize, Vec<&'a str>>, Vec<usize>) {
    // Contested cards are those many skeleton packs want relative to the copies owned.
    let mut weight: BTreeMap<&str, f64> = BTreeMap::new();
    for &p in order_of {
        for (id, _) in pools[p] {
            *weight.entry(id).or_default() += 1.0;
        }
    }
    for (id, w) in weight.iter_mut() {
        *w /= copies[id] as f64;
    }
    let mut tight = order_of.to_vec();
    tight.sort_by_key(|&p| pools[p].len());

    let mut search = Search {
        pools,
        weight: &weight,
        nodes: SEARCH_NODES,
    };
    let mut picks = BTreeMap::new();
    if search.place(&tight, &mut copies.clone(), &mut picks) {
        return (picks, Vec::new());
    }
    // No complete placement: report the packs a plain greedy pass cannot place.
    let mut left = copies.clone();
    let mut picks = BTreeMap::new();
    let mut unsolved = Vec::new();
    for &p in &tight {
        match solve_pack(pools[p], &left, &weight).into_iter().next() {
            Some(cards) => {
                for c in &cards {
                    *left.get_mut(c).unwrap() -= 1;
                }
                picks.insert(p, cards);
            }
            None => unsolved.push(p),
        }
    }
    unsolved.sort();
    (picks, unsolved)
}

/// Why a pack gets no skeleton: first against its own pool, then against the other packs.
fn skeleton_shortfall(name: &str, pool: &[(&str, Meta)], shared: bool) -> String {
    let mut by_class = [0; 3];
    let mut by_bucket = [0; 6];
    for (_, m) in pool {
        by_class[m.class] += 1;
        by_bucket[m.bucket] += 1;
    }
    let weight = pool.iter().map(|(id, _)| (*id, 0.0)).collect();
    let alone = pool.iter().map(|(id, _)| (*id, 1)).collect();
    let pool_counts = format!(
        "{} {}, {} {}, {} {}; by cost 1..6+: {:?}",
        by_class[0],
        CLASS_NAMES[0],
        by_class[1],
        CLASS_NAMES[1],
        by_class[2],
        CLASS_NAMES[2],
        by_bucket
    );
    if solve_pack(pool, &alone, &weight).is_empty() {
        let mut gaps = Vec::new();
        if by_class[0] < GROUND_UNITS as usize {
            gaps.push(format!("needs {GROUND_UNITS} {}", CLASS_NAMES[0]));
        }
        if by_class[1] < 3 {
            gaps.push(format!("needs 3-4 {}", CLASS_NAMES[1]));
        }
        if by_class[2] < 3 {
            gaps.push(format!("needs 3-4 {}", CLASS_NAMES[2]));
        }
        for (i, (lo, _)) in CURVE.iter().enumerate() {
            if by_bucket[i] < *lo as usize {
                gaps.push(format!("needs at least {lo} at cost {}", i + 1));
            }
        }
        return format!(
            "{name} cannot meet the pack skeleton from the cards it is eligible for ({pool_counts}); {}",
            if gaps.is_empty() {
                "no combination meets the curve and arena counts together".to_string()
            } else {
                gaps.join(", ")
            }
        );
    }
    format!(
        "{name} can meet the pack skeleton alone ({pool_counts}) but not with the copies{} left once the other packs are placed",
        if shared { " each" } else { "" }
    )
}

/// Give pack `start` one more card, moving other packs' picks to alternatives where needed.
fn augment<'a>(
    start: usize,
    eligible: &[Vec<&'a str>],
    copies: &BTreeMap<&str, usize>,
    held: &mut [BTreeSet<&'a str>],
    users: &mut BTreeMap<&'a str, BTreeSet<usize>>,
) -> bool {
    // BFS over packs; edge p -> q when p wants a card (not held) that q holds and p cannot get free.
    let mut prev: BTreeMap<usize, (usize, &'a str)> = BTreeMap::new(); // pack -> (from pack, card taken from it)
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(p) = queue.pop_front() {
        for &c in &eligible[p] {
            if held[p].contains(c) {
                continue;
            }
            let used = users.get(c).map_or(0, BTreeSet::len);
            if used < copies[c] {
                // Free copy: p takes it; each pack on the path back hands over what its
                // predecessor wanted and takes the next link in turn.
                let (mut cur, mut card) = (p, c);
                loop {
                    held[cur].insert(card);
                    users.entry(card).or_default().insert(cur);
                    match prev.get(&cur) {
                        Some(&(from, taken)) => {
                            held[cur].remove(taken);
                            users.get_mut(taken).unwrap().remove(&cur);
                            cur = from;
                            card = taken;
                        }
                        None => return true,
                    }
                }
            }
            for &q in users.get(c).into_iter().flatten() {
                if seen.insert(q) {
                    prev.insert(q, (p, c));
                    queue.push_back(q);
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Pack-by-pack, first eligible card with a copy left: what the skill did before it planned.
    fn greedy(doc: &Value) -> Vec<String> {
        let mut left: BTreeMap<String, u64> = doc["owned"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
            .collect();
        let mut starved = Vec::new();
        for p in doc["packs"].as_array().unwrap() {
            let mut need = p["slots"].as_u64().unwrap();
            for c in p["eligible"].as_array().unwrap() {
                let n = left.get_mut(c.as_str().unwrap()).unwrap();
                if need > 0 && *n > 0 {
                    *n -= 1;
                    need -= 1;
                }
            }
            if need > 0 {
                starved.push(p["name"].as_str().unwrap().to_string());
            }
        }
        starved
    }

    /// Longbeam Cruiser has no aspect, so every pack may take it; the Vigilance pack has little else.
    fn starving() -> Value {
        json!({
            "owned": {"Longbeam Cruiser": 1, "Vigilance Unit": 1, "Command Unit": 1},
            "packs": [
                {"name": "Pack 1 (Command)", "slots": 1, "eligible": ["Longbeam Cruiser", "Command Unit"]},
                {"name": "Pack 2 (Vigilance)", "slots": 1, "eligible": ["Longbeam Cruiser"]},
            ]
        })
    }

    #[test]
    fn a_greedy_build_starves_the_last_pack_and_the_plan_does_not() {
        let doc = starving();
        assert_eq!(greedy(&doc), vec!["Pack 2 (Vigilance)"]);
        let doc = with_picks(
            doc,
            [&["Longbeam Cruiser", "Command Unit"], &["Longbeam Cruiser"]],
        );
        let plan = allocate_value(&doc, false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert_eq!(
            plan.reservations["Longbeam Cruiser"],
            vec!["Pack 2 (Vigilance)"]
        );
        assert!(!plan.reservations.contains_key("Command Unit"));
    }

    #[test]
    fn a_shortfall_names_the_card_and_the_competing_packs() {
        let mut doc = starving();
        doc["owned"]["Command Unit"] = json!(0);
        let plan = allocate_value(&doc, false).unwrap();
        assert_eq!(plan.failures.len(), 1);
        let m = &plan.failures[0].message;
        assert!(
            m.contains("Longbeam Cruiser (1 owned)") && m.contains("Pack 1 (Command)"),
            "{m}"
        );
    }

    #[test]
    fn a_pack_takes_one_copy_of_a_card() {
        let doc =
            json!({"owned": {"A": 2}, "packs": [{"name": "P", "slots": 2, "eligible": ["A"]}]});
        assert_eq!(allocate_value(&doc, false).unwrap().failures.len(), 1);
    }

    /// An eligible pool whose card-id order breaks the skeleton: ten cost-2 events sort first, so
    /// taking 15 in id order gives 10 events and no space units. Each of `packs` packs can use it all.
    fn skeleton_doc(packs: usize) -> Value {
        use crate::fixture::SKELETON;
        let mut owned = serde_json::Map::new();
        let mut rows = Vec::new();
        let mut row = |id: String, kind: &str, arena: &str, cost: u32| {
            owned.insert(id.clone(), json!(1));
            rows.push(json!({"id": id, "type": kind, "cost": cost,
                "game": {"arenas": if arena.is_empty() { vec![] } else { vec![arena] }}}));
        };
        for i in 0..10 {
            row(format!("A-{i:03}"), "Event", "", 2);
        }
        for n in 0..packs {
            for (i, (kind, arena, cost)) in SKELETON.iter().enumerate() {
                row(format!("Z{n}-{i:02}"), kind, arena, *cost);
            }
        }
        let pack = |n: usize| {
            json!({"name": format!("Pack {}", n + 1), "slots": 15, "eligible": rows,
                "candidates": rows.iter().map(|r| r["id"].clone()).collect::<Vec<_>>()})
        };
        json!({"owned": owned, "packs": (0..packs).map(pack).collect::<Vec<_>>()})
    }

    #[test]
    fn picks_that_include_a_leader_or_too_many_cards_are_refused() {
        let mut doc = skeleton_doc(1);
        let leader = json!({"id": "L-1", "type": "Leader", "cost": 3, "game": {"arenas": ["Ground"]}});
        doc["packs"][0]["picks"] = json!([leader]);
        let err = allocate_value(&doc, false).err().unwrap();
        assert!(err.contains("L-1") && err.contains("leader"), "{err}");
        let ids: Vec<String> = (0..16).map(|i| format!("Z0-{i:02}")).collect();
        doc["packs"][0]["picks"] = json!(ids);
        let err = allocate_value(&doc, false).err().unwrap();
        assert!(err.contains("16 picks for 15 slots"), "{err}");
    }

    #[test]
    fn one_pack_reserves_nothing() {
        let doc = skeleton_doc(1);
        let plan = allocate_value(&doc, false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert!(plan.reservations.is_empty(), "{:?}", plan.reservations);
    }

    #[test]
    fn two_packs_wanting_one_copy_assign_it_to_exactly_one() {
        let doc = skeleton_doc(2);
        let plan = allocate_value(&doc, false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        // Every card is wanted by both packs and owned once: all are contested, each to one pack.
        assert_eq!(plan.reservations.len(), 10 + 2 * 15);
        assert!(plan.reservations.values().all(|ps| ps.len() == 1));
        assert_eq!(plan.contested["A-000"], (1, 2));
    }

    #[test]
    fn a_pool_that_cannot_form_the_skeleton_is_a_named_shortfall() {
        let mut doc = skeleton_doc(1);
        let eligible = doc["packs"][0]["eligible"].as_array_mut().unwrap();
        eligible.retain(|r| r["game"]["arenas"][0] != "Space");
        let plan = allocate_value(&doc, false).unwrap();
        assert_eq!(plan.failures.len(), 1);
        let m = &plan.failures[0].message;
        assert!(m.contains("Pack 1") && m.contains("space"), "{m}");
    }

    fn with_picks(mut doc: Value, picks: [&[&str]; 2]) -> Value {
        for (i, p) in picks.iter().enumerate() {
            doc["packs"][i]["picks"] = json!(p);
        }
        doc
    }

    #[test]
    fn a_card_eligible_for_two_packs_but_picked_by_neither_is_reserved_to_neither() {
        let doc = with_picks(starving(), [&["Command Unit"], &[]]);
        let plan = allocate_value(&doc, false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert!(plan.reservations.is_empty(), "{:?}", plan.reservations);
        assert!(plan.contested.is_empty());
    }

    #[test]
    fn a_card_picked_by_both_with_one_copy_goes_to_one_and_the_other_is_named() {
        let mut doc = starving();
        doc["owned"]["Command Unit"] = json!(0);
        let doc = with_picks(doc, [&["Longbeam Cruiser"], &["Longbeam Cruiser"]]);
        let plan = allocate_value(&doc, false).unwrap();
        assert_eq!(plan.reservations["Longbeam Cruiser"].len(), 1);
        assert_eq!(plan.failures.len(), 1);
        assert!(
            plan.failures[0]
                .message
                .contains("Longbeam Cruiser (1 owned)")
        );
    }

    #[test]
    fn a_card_only_one_pack_wants_is_left_to_the_skill() {
        let mut doc = skeleton_doc(2);
        let own: Vec<Value> = doc["packs"][0]["eligible"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| !r["id"].as_str().unwrap().starts_with("Z1"))
            .cloned()
            .collect();
        doc["packs"][0]["candidates"] =
            json!(own.iter().map(|r| r["id"].clone()).collect::<Vec<_>>());
        doc["packs"][0]["eligible"] = json!(own);
        let plan = allocate_value(&doc, false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert!(!plan.reservations.contains_key("Z1-00"));
        assert_eq!(plan.reservations["A-000"].len(), 1);
    }

    /// Two packs of one slot each, both able to use either Rare; the Rare is owned twice.
    fn rares(order: [&str; 2]) -> Value {
        let row = |id: &str, rarity: &str| {
            json!({"id": id, "name": id, "subtitle": null, "rarity": rarity, "type": "Unit",
                "cost": 2, "game": {"arenas": ["Ground"]}})
        };
        let packs: Vec<Value> = order
            .iter()
            .map(|n| json!({"name": n, "slots": 1, "eligible": [row("R-1", "Rare"), row("C-1", "Common")], "picks": ["R-1"]}))
            .collect();
        json!({"owned": {"R-1": 2, "C-1": 2}, "packs": packs})
    }

    #[test]
    fn a_rare_owned_twice_wanted_by_two_packs_is_reserved_to_one() {
        let plan = allocate_value(&rares(["A", "B"]), false).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert_eq!(plan.reservations["R-1"].len(), 1);
        assert!(!plan.reservations.contains_key("C-1"));
    }

    #[test]
    fn reversing_the_packs_gives_the_same_reservations() {
        let a = allocate_value(&rares(["A", "B"]), false).unwrap();
        let b = allocate_value(&rares(["B", "A"]), false).unwrap();
        assert_eq!(a.reservations, b.reservations);
        let lost = |p: &Plan| {
            p.summary
                .iter()
                .map(|s| (s.name.clone(), s.reserved, s.short, s.lost.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(lost(&a), lost(&b));
    }

    #[test]
    fn a_skeleton_plan_does_not_depend_on_pack_order() {
        let mut doc = skeleton_doc(3);
        let mut rev = doc.clone();
        for d in [&mut doc, &mut rev] {
            for p in d["packs"].as_array_mut().unwrap() {
                p["candidates"] = p["eligible"].clone();
            }
        }
        rev["packs"].as_array_mut().unwrap().reverse();
        let (a, b) = (
            allocate_value(&doc, false).unwrap(),
            allocate_value(&rev, false).unwrap(),
        );
        assert_eq!(a.reservations, b.reservations);
    }

    #[test]
    fn a_pick_held_by_a_saved_pack_fails_the_plan_naming_the_pack_and_card() {
        let doc = json!({"owned": {"X": 1, "Y": 1}, "packs": [
            {"name": "Hold 1", "saved": true, "slots": 1, "eligible": ["X"], "picks": ["X"]},
            {"name": "Hold 2", "slots": 1, "eligible": ["X", "Y"], "picks": ["X"]},
        ]});
        let plan = allocate_value(&doc, false).unwrap();
        assert_eq!(plan.reservations["X"], vec!["Hold 1"]);
        assert_eq!(plan.failures.len(), 1, "{:?}", plan.failures);
        let m = &plan.failures[0].message;
        assert!(
            m.contains("Hold 2") && m.contains("X") && m.contains("held by Hold 1"),
            "{m}"
        );
    }
}
