//! `swu-cube-check mcp`: the `check_cube` tool over MCP stdio (newline-delimited JSON-RPC). It
//! fetches decks through a [`Source`] exactly as the CLI's `pack --deck` / `cube --group` do and
//! answers with the text the CLI prints.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::fetch::{self, Source};

const PROTOCOL: &str = "2025-06-18";

/// Opens the site for one call; the argument is the call's `site` override.
pub type Open = dyn Fn(Option<&str>) -> Result<Box<dyn Source>, String>;

fn tool() -> Value {
    json!({
        "name": "check_cube",
        "description": "Check saved dumpstar decks against the SWU Jumpstart cube rules. mode `pack` checks each deck as one 18-card pack; mode `cube` checks the decks together as a 20-pack cube (singleton, copies, alignment, bases); mode `copies` runs only the copy-count check over a partial build; mode `allocate` plans copies across the whole cube (give `group` and/or `plan`, not `deck_ids`; `verbose` adds the reserve lines, `fill` chooses cards for the slots a plan leaves open). Name the decks by `deck_ids` or by `group`; the checker fetches them itself and verifies each deck's lines_digest. Returns `ok`, or one `FAIL <rule>: <what is wrong>` line per broken rule.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["pack", "cube", "copies", "allocate"]},
                "deck_ids": {"type": "array", "items": {"type": "integer"}, "description": "Saved deck ids."},
                "group": {"type": "string", "description": "A deck group: every deck in it."},
                "plan": {"type": "object", "description": "allocate only: a plan whose `packs` name `queries` (searches the checker runs) and/or `deck` (a saved deck id), as `allocate --plan FILE` reads."},
                "verbose": {"type": "boolean", "description": "allocate only: add every reserve line."},
                "fill": {"type": "boolean", "description": "allocate only: choose cards for open slots."},
                "site": {"type": "string", "description": "Override the dumpstar site URL."}
            },
            "required": ["mode"],
            "additionalProperties": false
        }
    })
}

/// Runs one `check_cube` call: the verdict text and whether it is an error (input the checker
/// could not read or fetch; a rule failure is a result).
pub fn check_cube(args: &Value, src: &dyn Source) -> (String, bool) {
    let run = || -> Result<(i32, String), String> {
        let mode = args["mode"]
            .as_str()
            .ok_or("mode must be `pack`, `cube`, `copies` or `allocate`")?;
        if mode == "allocate" {
            if args.get("deck_ids").is_some_and(|v| !v.is_null()) {
                return Err("allocate takes group and/or plan, not deck_ids".into());
            }
            let group = args.get("group").and_then(Value::as_str);
            let plan = args.get("plan").filter(|v| !v.is_null());
            if group.is_none() && plan.is_none() {
                return Err("allocate needs a group and/or a plan".into());
            }
            let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
            let doc = fetch::allocation(src, group, plan)?;
            let plan = crate::allocate::allocate_value(&doc, flag("fill"))?;
            return Ok(crate::allocate::render(&plan, flag("verbose")));
        }
        let ids: Option<Vec<i64>> = match args.get("deck_ids").filter(|v| !v.is_null()) {
            Some(v) => Some(
                v.as_array()
                    .ok_or("deck_ids must be an array of integers")?
                    .iter()
                    .map(|i| i.as_i64().ok_or("deck_ids must be an array of integers"))
                    .collect::<Result<_, _>>()?,
            ),
            None => None,
        };
        let group = args.get("group").and_then(Value::as_str);
        let decks: Vec<(Option<i64>, Value)> = match (ids, group) {
            (Some(ids), None) => {
                if ids.is_empty() {
                    return Err("deck_ids is empty".into());
                }
                ids.into_iter()
                    .map(|id| Ok((Some(id), src.deck(id)?)))
                    .collect::<Result<_, String>>()?
            }
            (None, Some(g)) => fetch::group_decks(src, g)?
                .into_iter()
                .map(|d| (None, d))
                .collect(),
            _ => return Err("name the decks by deck_ids or by group, not both or neither".into()),
        };
        match mode {
            "cube" => {
                let all = Value::Array(decks.into_iter().map(|(_, d)| d).collect());
                Ok(crate::render(crate::check_cube_value(&all)))
            }
            "copies" => {
                let all = Value::Array(decks.into_iter().map(|(_, d)| d).collect());
                Ok(crate::render(crate::check_copies_value(&all)))
            }
            "pack" if decks.len() == 1 => Ok(crate::render(crate::check_pack_value(&decks[0].1))),
            "pack" => {
                let (mut code, mut lines) = (0, Vec::new());
                for (n, (id, d)) in decks.iter().enumerate() {
                    let label = id.map_or_else(
                        || {
                            d["deck"]["name"]
                                .as_str()
                                .map_or(format!("deck {}", n + 1), String::from)
                        },
                        |i| format!("deck {i}"),
                    );
                    let (c, text) = crate::render(crate::check_pack_value(d));
                    code = code.max(c);
                    lines.extend(text.lines().map(|l| format!("{label}: {l}")));
                }
                Ok((code, lines.join("\n")))
            }
            other => Err(format!(
                "mode must be `pack`, `cube`, `copies` or `allocate`, not `{other}`"
            )),
        }
    };
    match run() {
        Ok((code, text)) => (text, code == 2),
        Err(e) => (e, true),
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle(msg: &Value, open: &Open) -> Option<Value> {
    let id = msg.get("id")?;
    let params = &msg["params"];
    Some(match msg["method"].as_str() {
        Some("initialize") => reply(
            id,
            json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "swu-cube-check", "version": env!("CARGO_PKG_VERSION")},
            }),
        ),
        Some("ping") => reply(id, json!({})),
        Some("tools/list") => reply(id, json!({"tools": [tool()]})),
        Some("tools/call") if params["name"] == "check_cube" => {
            let args = &params["arguments"];
            let (text, is_error) = match open(args["site"].as_str()) {
                Ok(src) => check_cube(args, src.as_ref()),
                Err(e) => (e, true),
            };
            reply(
                id,
                json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
            )
        }
        Some("tools/call") => error(id, -32602, "unknown tool"),
        _ => error(id, -32601, "method not found"),
    })
}

/// Serves requests from `input` until it closes. `open` is called once per tool call, so a
/// sign-in Claude Code refreshed in between is picked up.
pub fn serve(input: impl BufRead, mut out: impl Write, open: &Open) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, open),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = response {
            writeln!(out, "{r}")
                .and_then(|_| out.flush())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
