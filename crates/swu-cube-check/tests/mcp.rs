//! `swu-cube-check mcp` against the fixture site: `check_cube` gives the verdict the CLI gives for
//! the same decks.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use common::{claude_store, far_future, serve};
use serde_json::{Value, json};
use swu_cube_check::fixture::cube;

/// Sends `requests` (one JSON-RPC message per line) to `mcp` and returns the responses.
fn session(claude: &std::path::Path, site: &str, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .arg("mcp")
        .env("DUMPSTAR_SITE", site)
        .env("SWU_CUBE_CHECK_CONFIG", claude.with_file_name("none.json"))
        .env("SWU_CUBE_CHECK_CLAUDE_CREDENTIALS", claude)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = BufReader::new(child.stdout.take().unwrap());
    let replies = out
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    replies
}

fn call(id: u64, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": "check_cube", "arguments": args}})
}

fn text(reply: &Value) -> (&str, bool) {
    (
        reply["result"]["content"][0]["text"].as_str().unwrap(),
        reply["result"]["isError"].as_bool().unwrap(),
    )
}

fn cli(claude: &std::path::Path, site: &str, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .env("DUMPSTAR_SITE", site)
        .env("SWU_CUBE_CHECK_CONFIG", claude.with_file_name("none.json"))
        .env("SWU_CUBE_CHECK_CLAUDE_CREDENTIALS", claude)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn the_handshake_lists_check_cube() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude.json");
    let r = session(
        &claude,
        "http://127.0.0.1:1",
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}),
        ],
    );
    assert_eq!(r.len(), 3, "a notification gets no reply: {r:?}");
    assert_eq!(r[0]["result"]["serverInfo"]["name"], "swu-cube-check");
    assert_eq!(r[1]["result"]["tools"][0]["name"], "check_cube");
    assert_eq!(r[2]["error"]["code"], -32601);
}

#[test]
fn check_cube_gives_the_failures_the_cli_reports() {
    let dir = tempfile::tempdir().unwrap();
    let mut broken = cube();
    broken[2]["cards"]["C3-3"]["cost"] = json!(1);
    broken.pop();
    let f = serve(broken, true);
    let ms = (far_future() + 3600) * 1000;
    let claude = claude_store(dir.path(), &f, ms);

    let r = session(
        &claude,
        &f.base,
        &[
            call(1, json!({"mode": "pack", "deck_ids": [3]})),
            call(2, json!({"mode": "pack", "deck_ids": [1]})),
            call(3, json!({"mode": "cube", "group": "swu-jumpstart-cube"})),
            call(4, json!({"mode": "cube", "deck_ids": [1, 2, 3]})),
            call(5, json!({"mode": "cube"})),
            call(6, json!({"mode": "pack", "deck_ids": [1, 3]})),
        ],
    );

    let (code, out) = cli(&claude, &f.base, &["pack", "--deck", "3"]);
    assert_eq!(code, 1);
    assert!(out.contains("FAIL curve:"), "{out}");
    assert_eq!(text(&r[0]), (out.trim_end(), false));

    let (code, out) = cli(&claude, &f.base, &["pack", "--deck", "1"]);
    assert_eq!((code, out.trim()), (0, "ok"));
    assert_eq!(text(&r[1]), ("ok", false));

    let (code, out) = cli(&claude, &f.base, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 1);
    assert!(out.contains("FAIL cube-size:"), "{out}");
    assert_eq!(text(&r[2]), (out.trim_end(), false));

    let (text4, _) = text(&r[3]);
    assert!(text4.contains("FAIL cube-size:"), "{text4}");

    let (msg, is_error) = text(&r[4]);
    assert!(is_error && msg.contains("deck_ids or by group"), "{msg}");

    let (both, _) = text(&r[5]);
    assert!(both.contains("deck 3: FAIL curve:"), "{both}");
    assert!(both.contains("deck 1: ok"), "{both}");
}

#[test]
fn no_sign_in_is_an_error_naming_login() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude.json");
    let r = session(
        &claude,
        "http://127.0.0.1:1",
        &[call(1, json!({"mode": "pack", "deck_ids": [1]}))],
    );
    let (msg, is_error) = text(&r[0]);
    assert!(is_error && msg.contains("login"), "{msg}");
}

#[test]
fn check_cube_allocate_gives_the_text_the_cli_prints() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let ms = (far_future() + 3600) * 1000;
    let claude = claude_store(dir.path(), &f, ms);

    let r = session(
        &claude,
        &f.base,
        &[
            call(
                1,
                json!({"mode": "allocate", "group": "swu-jumpstart-cube"}),
            ),
            call(
                2,
                json!({"mode": "allocate", "group": "swu-jumpstart-cube", "verbose": true, "fill": true}),
            ),
            call(
                3,
                json!({"mode": "allocate", "plan": {"packs": [{"name": "A", "queries": ["t:unit"]}]}}),
            ),
            call(4, json!({"mode": "allocate"})),
            call(
                5,
                json!({"mode": "allocate", "group": "g", "deck_ids": [1]}),
            ),
        ],
    );

    let (code, out) = cli(
        &claude,
        &f.base,
        &["allocate", "--group", "swu-jumpstart-cube"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Pack 1: 15 of 15 picks"), "{out}");
    assert_eq!(text(&r[0]), (out.trim_end(), false));

    let (code, out) = cli(
        &claude,
        &f.base,
        &[
            "allocate",
            "--group",
            "swu-jumpstart-cube",
            "--verbose",
            "--fill",
        ],
    );
    assert!(code == 0 || code == 1, "{out}");
    assert_eq!(text(&r[1]), (out.trim_end(), false));

    let (plan, is_error) = text(&r[2]);
    assert!(!is_error && plan.contains("\nA: 0 of 15 picks"), "{plan}");

    let (msg, is_error) = text(&r[3]);
    assert!(is_error && msg.contains("group and/or a plan"), "{msg}");
    let (msg, is_error) = text(&r[4]);
    assert!(is_error && msg.contains("not deck_ids"), "{msg}");
}
