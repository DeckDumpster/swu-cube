//! The fetch modes against a fixture site: `cube --group` reads every deck itself and gives the
//! verdict the file path gives, `allocate --group` runs the pack searches, and an expired sign-in
//! is an error that names `login`, never a silent fallback.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

mod common;

use common::{claude_store, far_future, serve, signed_in};
use serde_json::{Value, json};
use swu_cube_check::fixture::cube;

fn run(cfg: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .env("SWU_CUBE_CHECK_CONFIG", cfg)
        .env(
            "SWU_CUBE_CHECK_CLAUDE_CREDENTIALS",
            cfg.with_file_name("no-claude.json"),
        )
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn cube_group_fetches_every_deck_and_gives_the_file_path_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!((code, out.trim()), (0, "ok"));
    let decks = f
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("GET /api/agent/decks/"))
        .count();
    assert_eq!(decks, 20);

    let mut broken = cube();
    broken.pop();
    let f = serve(broken, true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 1);
    assert!(out.contains("FAIL cube-size:"), "{out}");
}

#[test]
fn pack_deck_fetches_one_deck() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, _) = run(&cfg, &["pack", "--deck", "3"]);
    assert_eq!((code, out.trim()), (0, "ok"));
}

#[test]
fn allocate_group_runs_the_pack_searches_itself() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, err) = run(&cfg, &["allocate", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Pack 1: 15 of 15 picks"), "{out}");
    // 20 packs x 4 pool searches, and no file read.
    let searches = f
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/api/agent/search"))
        .count();
    assert_eq!(searches, 20 * 4);
}

#[test]
fn allocate_plan_runs_its_queries() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let plan = dir.path().join("plan.json");
    std::fs::write(
        &plan,
        json!({"packs": [{"name": "A", "queries": ["t:unit"]}]}).to_string(),
    )
    .unwrap();
    let (code, out, err) = run(&cfg, &["allocate", "--plan", plan.to_str().unwrap()]);
    assert!(code == 0 || code == 1, "{out}{err}");
    assert!(
        f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("q=t%3Aunit"))
    );
}

#[test]
fn an_expired_sign_in_names_login_and_never_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), false);
    let cfg = signed_in(dir.path(), &f, "old", 1);
    let (code, out, err) = run(&cfg, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!(code, 2);
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("swu-cube-check login"), "{err}");
    assert!(!f.seen.lock().unwrap().iter().any(|r| r.contains("/api/")));
}

#[test]
fn a_site_that_rejects_the_token_names_login() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-stale", far_future());
    let (code, _, err) = run(&cfg, &["pack", "--deck", "1"]);
    assert_eq!(code, 2);
    assert!(err.contains("swu-cube-check login"), "{err}");
}

#[test]
fn no_cache_is_not_signed_in() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run(&dir.path().join("none.json"), &["pack", "--deck", "1"]);
    assert_eq!(code, 2);
    assert!(err.contains("swu-cube-check login"), "{err}");
}

#[test]
fn a_refresh_is_silent_cached_0600_and_prints_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "old", 1);
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, out, err) = run(&cfg, &["pack", "--deck", "1"]);
    assert_eq!(code, 0, "{err}");
    assert!(!format!("{out}{err}").contains("tok-fresh") && !err.contains("refresh-2"));
    assert_eq!(
        std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(
        (
            saved["refresh_token"].as_str(),
            saved["access_token"].as_str()
        ),
        (Some("refresh-2"), Some("tok-fresh"))
    );
    let (code, out, _) = run(&cfg, &["login", "--status"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("ryan@example.com")
            && out.contains("tenant concierge")
            && !out.contains("tok-"),
        "{out}"
    );
}

fn run_with_claude(
    cfg: &std::path::Path,
    claude: &std::path::Path,
    args: &[&str],
) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .env("SWU_CUBE_CHECK_CONFIG", cfg)
        .env("SWU_CUBE_CHECK_CLAUDE_CREDENTIALS", claude)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn claude_codes_mcp_sign_in_is_used_without_login_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let ms = (far_future() + 3600) * 1000;
    let claude = claude_store(dir.path(), &f, ms);
    let before = std::fs::read_to_string(&claude).unwrap();
    let cfg = dir.path().join("none.json");
    let (code, out, err) =
        run_with_claude(&cfg, &claude, &["login", "--status", "--site", &f.base]);
    assert_eq!(
        (code, out.trim()),
        (
            0,
            "Claude Code MCP sign-in (dumpstar) \u{2192} tenant concierge"
        ),
        "{err}"
    );
    let (code, out, _) = run_with_claude(
        &cfg,
        &claude,
        &["cube", "--group", "swu-jumpstart-cube", "--site", &f.base],
    );
    assert_eq!((code, out.trim()), (0, "ok"));
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), before);
    assert!(!cfg.exists());
}

#[test]
fn an_expired_claude_sign_in_says_reconnect_and_falls_through_to_login() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let claude = claude_store(dir.path(), &f, 1);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let (code, out, err) =
        run_with_claude(&cfg, &claude, &["cube", "--group", "swu-jumpstart-cube"]);
    assert_eq!((code, out.trim()), (0, "ok"), "{err}");
    assert!(err.contains("/mcp") && !err.contains("tok-"), "{err}");
}

#[test]
fn a_401_on_claude_codes_token_names_the_mcp_call_that_refreshes_it() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let claude = claude_store(dir.path(), &f, (far_future() + 3600) * 1000);
    let text = std::fs::read_to_string(&claude)
        .unwrap()
        .replace("tok-fresh", "tok-stale");
    std::fs::write(&claude, text).unwrap();
    let cfg = dir.path().join("none.json");
    let (code, _, err) = run_with_claude(
        &cfg,
        &claude,
        &["cube", "--group", "swu-jumpstart-cube", "--site", &f.base],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("MCP call") && err.contains("expired"), "{err}");
}

#[test]
fn copies_planned_fetches_the_owned_counts_it_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let f = serve(cube(), true);
    let cfg = signed_in(dir.path(), &f, "tok-fresh", far_future());
    let mut pack = cube()[0].clone();
    for c in pack["cards"].as_object_mut().unwrap().values_mut() {
        c.as_object_mut().unwrap().remove("owned_count");
    }
    let file = dir.path().join("planned.json");
    std::fs::write(&file, pack.to_string()).unwrap();
    let (code, out, err) = run(&cfg, &["copies", "--planned", file.to_str().unwrap()]);
    assert_eq!((code, out.trim()), (0, "ok"), "{err}");
    assert!(
        f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("/api/agent/search"))
    );
}
