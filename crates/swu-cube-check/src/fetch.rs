//! What the checker reads from the site itself: a group's decks, and the searches an allocation
//! plan needs, so no agent copies tool output into a file. [`Source`] is the site (or a test's
//! stand-in); everything here is pure over it.

use std::collections::BTreeMap;

use serde_json::{Value, json};

const COLORS: [&str; 4] = ["command", "vigilance", "aggression", "cunning"];
const ALIGNMENTS: [&str; 2] = ["heroism", "villainy"];

pub trait Source {
    /// Ids of the decks in `group`, in id order.
    fn list_group(&self, group: &str) -> Result<Vec<i64>, String>;
    /// `get_deck`'s document for deck `id`, its `lines_digest` already verified.
    fn deck(&self, id: i64) -> Result<Value, String>;
    /// Every row of `search_cards` for `q` over the owned cards, all pages.
    fn search(&self, q: &str) -> Result<Vec<Value>, String>;
}

/// The decks of a group as pack documents; an empty group is an error, never a vacuous `ok`.
pub fn group_decks(src: &dyn Source, group: &str) -> Result<Vec<Value>, String> {
    let ids = src.list_group(group)?;
    if ids.is_empty() {
        return Err(format!("no decks in group '{group}'"));
    }
    ids.into_iter().map(|id| src.deck(id)).collect()
}

/// The four searches that cover a Leader's pool (the skill's step 4), from its aspects.
pub fn pool_queries(leader: &Value) -> Result<Vec<String>, String> {
    let aspects: Vec<String> = leader["game"]["aspects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str().map(str::to_lowercase))
        .collect();
    let color = aspects.iter().find(|a| COLORS.contains(&a.as_str()));
    let align = aspects.iter().find(|a| ALIGNMENTS.contains(&a.as_str()));
    let (Some(color), Some(align)) = (color, align) else {
        return Err(format!(
            "{} has no color and alignment to lead a pack",
            leader["name"].as_str().unwrap_or("the leader")
        ));
    };
    let other_colors: String = COLORS
        .iter()
        .filter(|c| *c != color)
        .map(|c| format!(" -a:{c}"))
        .collect();
    let all_colors: String = COLORS.iter().map(|c| format!(" -a:{c}")).collect();
    let other_align = ALIGNMENTS.iter().find(|a| *a != align).unwrap();
    Ok(vec![
        format!("a:{color} a:{align}{other_colors} -t:leader -t:base"),
        format!("a:{color} -a:{align} -a:{other_align}{other_colors} -t:leader -t:base"),
        format!("a:{align}{all_colors} -t:leader -t:base"),
        format!("-a:heroism -a:villainy{all_colors} -t:leader -t:base"),
    ])
}

fn id_of(row: &Value) -> Option<&str> {
    row.get("id").or_else(|| row.get("card_id"))?.as_str()
}

fn is_draw(card: &Value) -> bool {
    !matches!(card["type"].as_str(), Some("Leader" | "Base"))
}

/// One pack of an allocation from a saved deck: its draw cards are its picks and are eligible,
/// and its Leader's four pool searches fill the rest of its eligible rows.
fn deck_pack(
    src: &dyn Source,
    deck: &Value,
    owned: &mut BTreeMap<String, u64>,
) -> Result<Value, String> {
    let name = deck["deck"]["name"].as_str().unwrap_or("deck");
    let cards = deck["cards"].as_object().ok_or("deck has no cards")?;
    let leader = cards
        .values()
        .find(|c| c["type"] == "Leader")
        .ok_or(format!("{name} has no Leader"))?;
    let mut picks = Vec::new();
    let mut eligible: BTreeMap<String, Value> = BTreeMap::new();
    for l in deck["lines"].as_array().into_iter().flatten() {
        let Some(id) = l["card_id"].as_str() else {
            continue;
        };
        let card = &cards[id];
        if is_draw(card) {
            picks.push(json!(id));
            let mut row = card.clone();
            row["id"] = json!(id);
            eligible.insert(id.to_string(), row);
        }
    }
    for q in pool_queries(leader).map_err(|e| format!("{name}: {e}"))? {
        for row in src.search(&q)? {
            if let Some(id) = id_of(&row) {
                eligible.entry(id.to_string()).or_insert(row);
            }
        }
    }
    for (id, row) in &eligible {
        owned.insert(id.clone(), row["owned_count"].as_u64().unwrap_or(0));
    }
    Ok(json!({
        "name": name, "slots": 15,
        "eligible": eligible.into_values().collect::<Vec<_>>(),
        "picks": picks,
    }))
}

/// The allocation document for a group, or for a plan file whose packs name `queries` (search
/// strings the checker runs) and/or `deck` (a saved deck's id) in place of rows.
pub fn allocation(
    src: &dyn Source,
    group: Option<&str>,
    plan: Option<&Value>,
) -> Result<Value, String> {
    let mut owned: BTreeMap<String, u64> = BTreeMap::new();
    let mut packs = Vec::new();
    if let Some(g) = group {
        for d in group_decks(src, g)? {
            packs.push(deck_pack(src, &d, &mut owned)?);
        }
    }
    if let Some(plan) = plan {
        for (n, p) in plan["packs"]
            .as_array()
            .ok_or("`packs` must be an array")?
            .iter()
            .enumerate()
        {
            let mut pack = match p["deck"].as_i64() {
                Some(id) => {
                    let mut dp = deck_pack(src, &src.deck(id)?, &mut owned)?;
                    for k in ["name", "candidates"] {
                        if p.get(k).is_some() {
                            dp[k] = p[k].clone();
                        }
                    }
                    dp["saved"] = json!(true);
                    if let Some(extra) = p["picks"].as_array() {
                        let mut all = dp["picks"].as_array().cloned().unwrap_or_default();
                        all.extend(extra.iter().cloned());
                        dp["picks"] = json!(all);
                    }
                    dp
                }
                None => p.clone(),
            };
            if pack.get("name").is_none() {
                pack["name"] = json!(format!("Pack {}", n + 1));
            }
            let mut rows: Vec<Value> = pack["eligible"].as_array().cloned().unwrap_or_default();
            for q in p["queries"].as_array().into_iter().flatten() {
                let q = q.as_str().ok_or("`queries` must be strings")?;
                for row in src.search(q)? {
                    if let Some(id) = id_of(&row) {
                        let n = row["available_count"].as_u64().unwrap_or(0);
                        let n = if p["deck"].is_i64() {
                            row["owned_count"].as_u64().unwrap_or(n)
                        } else {
                            n
                        };
                        owned.entry(id.to_string()).or_insert(n);
                    }
                    rows.push(row);
                }
            }
            if let Some(o) = pack.as_object_mut() {
                o.remove("queries");
                o.remove("deck");
            }
            pack["eligible"] = json!(rows);
            if pack.get("slots").is_none() {
                pack["slots"] = json!(15);
            }
            packs.push(pack);
        }
        if let Some(extra) = plan["owned"].as_object() {
            for (id, n) in extra {
                owned.insert(
                    id.clone(),
                    n.as_u64()
                        .ok_or(format!("copies of {id} is not a number"))?,
                );
            }
        }
    }
    if packs.is_empty() {
        return Err("nothing to allocate: give --group or a --plan with packs".into());
    }
    Ok(json!({ "owned": owned, "packs": packs }))
}

/// Gives every card on a pack's lines that has no `owned_count` the count the site reports, found
/// by searching its name over the owned cards and matching the id; a card the search does not
/// return is not owned and gets 0. Returns how many cards were filled.
pub fn fill_owned(src: &dyn Source, doc: &mut Value) -> Result<usize, String> {
    let mut missing: BTreeMap<String, String> = BTreeMap::new();
    let mut note = |cards: &Value, lines: &Value| {
        for l in lines.as_array().into_iter().flatten() {
            let Some(id) = l["card_id"].as_str() else {
                continue;
            };
            let c = &cards[id];
            if c.get("owned_count").is_none_or(Value::is_null) {
                let name = c["name"].as_str().unwrap_or(id);
                missing.insert(id.to_string(), name.to_string());
            }
        }
    };
    match &*doc {
        Value::Array(packs) => packs.iter().for_each(|p| note(&p["cards"], &p["lines"])),
        d => {
            for p in d["packs"].as_array().into_iter().flatten() {
                let shared = if p["cards"].is_object() {
                    &p["cards"]
                } else {
                    &d["cards"]
                };
                note(shared, &p["lines"]);
            }
            if d.get("packs").is_none() {
                note(&d["cards"], &d["lines"]);
            }
        }
    }
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let names: std::collections::BTreeSet<&String> = missing.values().collect();
    for name in names {
        for row in src.search(&format!("\"{name}\""))? {
            if let Some(id) = id_of(&row) {
                counts.insert(id.to_string(), row["owned_count"].as_u64().unwrap_or(0));
            }
        }
    }
    let fill = |cards: &mut Value| {
        for id in missing.keys() {
            if let Some(c) = cards.get_mut(id.as_str()).filter(|c| c.is_object()) {
                c["owned_count"] = json!(counts.get(id).copied().unwrap_or(0));
            }
        }
    };
    match doc {
        Value::Array(packs) => packs.iter_mut().for_each(|p| fill(&mut p["cards"])),
        d => {
            fill(&mut d["cards"]);
            for p in d["packs"].as_array_mut().into_iter().flatten() {
                fill(&mut p["cards"]);
            }
        }
    }
    Ok(missing.len())
}
