//! Opt-in: `check_cube` against a dumpstar realenv slot's real `get_deck` output.
//!
//! Set `SWU_CUBE_SITE` to the slot's URL (and `SWU_CUBE_GROUP` to a group of saved decks there,
//! default `swu-jumpstart-cube`). The checker signs in as it does for a user: Claude Code's dumpstar
//! sign-in, else `swu-cube-check login`. Unset, the test does nothing.

use serde_json::json;
use swu_cube_check::fetch::Source;
use swu_cube_check::{mcp, site};

#[test]
fn check_cube_reads_the_real_get_deck_and_verifies_its_digest() {
    let Ok(base) = std::env::var("SWU_CUBE_SITE") else {
        eprintln!("SWU_CUBE_SITE is not set: skipping");
        return;
    };
    let group = std::env::var("SWU_CUBE_GROUP").unwrap_or_else(|_| "swu-jumpstart-cube".into());
    let src = site::Site::open(&site::config_path(), Some(&base)).unwrap();
    let ids = src.list_group(&group).unwrap();
    assert!(!ids.is_empty(), "no decks in group '{group}' on {base}");

    // `deck` refuses a document whose lines_digest does not match its lines and cards.
    let deck = src.deck(ids[0]).unwrap();
    assert_eq!(
        swu_cube_check::lines_digest(&deck["lines"], &deck["cards"]).as_deref(),
        deck["lines_digest"].as_str(),
        "the digest the site sent is not the canonical one"
    );

    let (text, is_error) = mcp::check_cube(&json!({"mode": "pack", "deck_ids": [ids[0]]}), &src);
    assert!(!is_error, "{text}");
    assert!(text == "ok" || text.starts_with("FAIL "), "{text}");
}
