//! The binary the skill runs: exit 0 and "ok" on a passing document, exit 1 and a FAIL line on a
//! failing one, exit 2 on input it cannot read.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;
use swu_cube_check::fixture::{cube, pack};

fn run(mode: &str, input: &str) -> (i32, String) {
    run_args(&[mode], input)
}

fn run_args(args: &[&str], input: &str) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn check_pack_passes_and_fails_by_exit_status() {
    let (code, out) = run("pack", &pack(1, "Heroism", "Command").to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut bad = pack(1, "Heroism", "Command");
    bad["cards"]["C1-3"]["cost"] = json!(1);
    let (code, out) = run("pack", &bad.to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL curve:"), "{out}");
}

#[test]
fn check_cube_passes_and_fails_by_exit_status() {
    let (code, out) = run("cube", &json!(cube()).to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut short = cube();
    short.pop();
    let (code, out) = run("cube", &json!(short).to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL cube-size:"), "{out}");
}

#[test]
fn a_leader_in_two_packs_fails_the_cube_and_names_both() {
    let mut c = cube();
    let dup = c[0]["cards"]["L1"].clone();
    c[1]["cards"]["L2"] = dup;
    let (code, out) = run("cube", &json!(c).to_string());
    assert_eq!(code, 1);
    assert!(
        out.contains("FAIL cube-singleton:") && out.contains("Pack 1, Pack 2"),
        "{out}"
    );
}

#[test]
fn two_packs_sharing_a_single_owned_card_fail_the_cube_and_name_both() {
    let mut c = cube();
    c[0]["cards"]["C1-0"]["owned_count"] = json!(1);
    // Pack 2 takes the same card id as pack 1's C1-0, in place of its own C2-0.
    c[1]["cards"]["C1-0"] = c[0]["cards"]["C1-0"].clone();
    c[1]["lines"][3]["card_id"] = json!("C1-0");
    let (code, out) = run("cube", &json!(c).to_string());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("FAIL cube-copies:")
            && out.contains("'C1-0'")
            && out.contains("Pack 1, Pack 2"),
        "{out}"
    );
}

#[test]
fn cube_takes_one_file_per_pack_and_a_lone_pack_passes_pack_mode() {
    let dir = std::env::temp_dir().join(format!("swu-cube-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = cube()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let f = dir.join(format!("pack{i}.json"));
            std::fs::write(&f, p.to_string()).unwrap();
            f.to_str().unwrap().to_string()
        })
        .collect();
    let mut args = vec!["cube"];
    args.extend(files.iter().map(String::as_str));
    let (code, out) = run_args(&args, "");
    assert_eq!((code, out.trim()), (0, "ok"));
    let (code, out) = run_args(&["pack", &files[0]], "");
    assert_eq!((code, out.trim()), (0, "ok"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn input_that_is_not_json_exits_2() {
    assert_eq!(run("pack", "nope").0, 2);
}

#[test]
fn a_planned_document_without_reservations_is_not_failed_on_reserved() {
    let mut planned = pack(1, "Heroism", "Command");
    for l in planned["lines"].as_array_mut().unwrap() {
        l["reserved"] = json!(0);
    }
    let (code, out) = run("pack", &planned.to_string());
    assert_eq!(code, 1);
    assert!(out.contains("FAIL reserved:"), "{out}");
    let (code, out) = run_args(&["pack", "--planned"], &planned.to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let (code, out) = run_args(&["--planned", "cube"], &json!([planned]).to_string());
    assert!(!out.contains("FAIL reserved:"), "{out}");
    assert_eq!(code, 1, "cube-size still fails: {out}");
}

#[test]
fn a_digest_that_does_not_match_the_lines_is_refused_before_any_rule() {
    let mut saved = pack(1, "Heroism", "Command");
    saved["lines_digest"] =
        json!(swu_cube_check::lines_digest(&saved["lines"], &saved["cards"]).unwrap());
    let (code, out) = run("pack", &saved.to_string());
    assert_eq!((code, out.trim()), (0, "ok"));
    let mut altered = saved.clone();
    altered["lines"][0]["qty"] = json!(2);
    let (code, out) = run("pack", &altered.to_string());
    assert_eq!((code, out.trim()), (2, ""));
    altered = saved.clone();
    altered["cards"]["L1"]["game"]["aspects"] = json!(["Heroism", "Cunning"]);
    let (code, out) = run("pack", &altered.to_string());
    assert_eq!((code, out.trim()), (2, ""));
}

#[test]
fn allocate_reserves_shared_cards_across_packs_and_names_a_shortfall() {
    let mut doc = json!({
        "owned": {"Longbeam Cruiser": 1, "Command Unit": 1},
        "packs": [
            {"name": "Pack 1", "slots": 1, "eligible": ["Longbeam Cruiser", "Command Unit"], "picks": ["Longbeam Cruiser", "Command Unit"]},
            {"name": "Pack 2", "slots": 1, "eligible": ["Longbeam Cruiser"], "picks": ["Longbeam Cruiser"]},
        ]
    });
    let (code, out) = run_args(&["allocate", "--verbose"], &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("reserve Longbeam Cruiser: Pack 2 (contested: 1 copy, 2 packs)"),
        "{out}"
    );
    assert!(!out.contains("reserve Command Unit"), "{out}");
    let one = json!({"owned": {"A": 1}, "packs": [{"name": "P", "slots": 1, "eligible": ["A"], "picks": ["A"]}]});
    let (code, out) = run("allocate", &one.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing over-subscribed"), "{out}");
    doc["owned"]["Command Unit"] = json!(0);
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 1);
    assert!(
        out.contains("FAIL allocation:") && out.contains("Longbeam Cruiser"),
        "{out}"
    );
}

#[test]
fn verbose_allocate_lists_a_card_shared_by_two_packs_that_has_a_copy_for_each() {
    let doc = json!({
        "owned": {"Shared Card": 2},
        "packs": [
            {"name": "Pack 1", "slots": 1, "eligible": ["Shared Card"], "picks": ["Shared Card"]},
            {"name": "Pack 2", "slots": 1, "eligible": ["Shared Card"], "picks": ["Shared Card"]},
        ]
    });
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing over-subscribed"), "{out}");
    assert!(!out.contains("shared Shared Card"), "{out}");
    let (code, out) = run_args(&["allocate", "--verbose"], &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("shared Shared Card: Pack 1, Pack 2 (2 copies owned"),
        "{out}"
    );
}

#[test]
fn a_twenty_pack_allocation_prints_one_short_line_per_pack() {
    let mut owned = serde_json::Map::new();
    let mut rows = Vec::new();
    for i in 0..60 {
        let id = format!("X-{i:03}");
        owned.insert(id.clone(), json!(2));
        let rarity = if i % 2 == 0 { "Rare" } else { "Common" };
        rows.push(
            json!({"id": id, "name": format!("Card {i}"), "subtitle": null,
            "rarity": rarity, "type": "Unit", "cost": 2, "game": {"arenas": ["Ground"]}}),
        );
    }
    let packs: Vec<_> = (0..20)
        .map(|n| json!({"name": format!("Aggression H{n:02}"), "slots": 3, "eligible": rows, "candidates": rows}))
        .collect();
    let doc = json!({"owned": owned, "packs": packs});
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(out.len() < 4096, "{} bytes", out.len());
    for n in 0..20 {
        assert_eq!(
            out.matches(&format!("Aggression H{n:02}:")).count(),
            1,
            "{out}"
        );
    }
    assert!(!out.contains("reserve "), "{out}");
}

#[test]
fn help_prints_the_usage_for_every_subcommand_and_exits_zero() {
    for args in [
        &["--help"][..],
        &["-h"],
        &["pack", "--help"],
        &["cube", "-h"],
        &["allocate", "--help"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("usage: swu-cube-check"), "{args:?}: {text}");
        if args[0] != "--help" && args[0] != "-h" {
            assert!(text.contains(args[0]), "{args:?}: {text}");
        }
    }
}

#[test]
fn copies_reports_only_the_shared_card_over_two_packs() {
    let mut c = cube();
    for p in c.iter_mut().take(2) {
        for card in p["cards"].as_object_mut().unwrap().values_mut() {
            card["owned_count"] = json!(5);
        }
    }
    c[0]["cards"]["C1-0"]["owned_count"] = json!(1);
    c[1]["cards"]["C1-0"] = c[0]["cards"]["C1-0"].clone();
    c[1]["lines"][3]["card_id"] = json!("C1-0");
    let dir = std::env::temp_dir().join(format!("copies-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = (0..2)
        .map(|i| {
            let f = dir.join(format!("p{i}.json"));
            std::fs::write(&f, c[i].to_string()).unwrap();
            f.to_string_lossy().into_owned()
        })
        .collect();
    let (code, out) = run_args(&["copies", &files[0], &files[1]], "");
    assert_eq!(code, 1, "{out}");
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.starts_with("FAIL cube-copies:"), "{out}");
    let (code, out) = run_args(&["copies", &files[0]], "");
    assert_eq!((code, out.trim()), (0, "ok"));
}

fn skeleton_pool(owned: &mut serde_json::Map<String, serde_json::Value>) -> Vec<serde_json::Value> {
    swu_cube_check::fixture::SKELETON
        .iter()
        .enumerate()
        .map(|(i, (kind, arena, cost))| {
            let id = format!("S-{i:02}");
            owned.insert(id.clone(), json!(1));
            json!({"id": id, "type": kind, "cost": cost,
                "game": {"arenas": if arena.is_empty() { vec![] } else { vec![*arena] }}})
        })
        .collect()
}

#[test]
fn the_pack_line_says_what_each_number_counts_for_an_uncontested_pack() {
    let mut owned = serde_json::Map::new();
    let rows = skeleton_pool(&mut owned);
    let ids: Vec<_> = rows.iter().map(|r| r["id"].clone()).collect();
    let doc = json!({"owned": owned, "packs": [{"name": "Plan 1", "slots": 15, "eligible": rows, "picks": ids}]});
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Plan 1: 15 of 15 picks, eligible pool of 15, skeleton ok, contested reserved 0, short 0, lost 0\n"),
        "{out}"
    );
}

#[test]
fn empty_picks_are_not_invented_and_a_filled_pack_says_filled() {
    let mut owned = serde_json::Map::new();
    let rows = skeleton_pool(&mut owned);
    let doc = json!({"owned": owned, "packs": [{"name": "Plan 1", "slots": 15, "eligible": rows, "picks": []}]});
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Plan 1: 0 of 15 picks, eligible pool of 15, skeleton ok"),
        "{out}"
    );
    assert!(!out.contains("filled") || out.contains("--fill"), "{out}");
    let (code, out) = run_args(&["allocate", "--fill"], &doc.to_string());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Plan 1: 0 of 15 picks, 15 filled, eligible pool of 15"),
        "{out}"
    );
}

#[test]
fn twenty_empty_packs_over_a_small_shared_pool_report_contention() {
    let mut owned = serde_json::Map::new();
    let rows = skeleton_pool(&mut owned);
    let packs: Vec<_> = (0..20)
        .map(
            |i| json!({"name": format!("Pack {i:02}"), "slots": 15, "eligible": rows, "picks": []}),
        )
        .collect();
    let doc = json!({"owned": owned, "packs": packs});
    let (code, out) = run("allocate", &doc.to_string());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("FAIL allocation:") && out.contains("competing packs"),
        "{out}"
    );
    assert!(!out.contains("nothing over-subscribed"), "{out}");
}

#[test]
fn the_pack_line_names_an_unmet_skeleton_and_a_short_pack() {
    let mut owned = serde_json::Map::new();
    let mut rows = skeleton_pool(&mut owned);
    rows.pop();
    let doc = json!({"owned": owned, "packs": [
        {"name": "Plan 1", "slots": 15, "eligible": rows, "candidates": rows},
        {"name": "Plan 2", "slots": 2, "eligible": ["A"], "picks": ["A"]},
    ]});
    let mut with_a = doc.clone();
    with_a["owned"]["A"] = json!(1);
    let (code, out) = run("allocate", &with_a.to_string());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(
            "Plan 1: 0 of 15 picks, eligible pool of 14, skeleton unmet, contested reserved 0, short 15, lost 0"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "Plan 2: 1 of 2 picks, eligible pool of 1, skeleton not checked, contested reserved 0, short 1, lost 0"
        ),
        "{out}"
    );
}

#[test]
fn copies_refuses_when_it_cannot_get_owned_counts() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("planned.json");
    std::fs::write(&f, cube()[0].to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_swu-cube-check"))
        .args(["copies", "--planned", f.to_str().unwrap()])
        .env("SWU_CUBE_CHECK_CONFIG", dir.path().join("none.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("owned_count") && err.contains("login"),
        "{err}"
    );
}
