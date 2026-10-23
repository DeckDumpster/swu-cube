# swu-cube

A Claude Code plugin for building a Star Wars: Unlimited Jumpstart cube from your own collection on [dumpstar](https://star.dumpster.cards).

It holds the `swu-jumpstart-cube` skill and the `swu-cube-check` checker, which enforces the cube rules on your machine. The dumpstar server enforces none of them; it supplies the collection and the decks through its `/mcp` tools.

Install from the DeckDumpster marketplace:

```
/plugin marketplace add DeckDumpster/plugins
/plugin install swu-cube@deckdumpster
```

## Sign in

The plugin declares two MCP servers: `dumpstar` (`https://star.dumpster.cards/mcp`) and `swu-cube-check`, which runs the checker locally. Sign in to dumpstar once with `/mcp` (select `dumpstar`, authenticate in the browser). `swu-cube-check` reuses that sign-in: it reads Claude Code's stored dumpstar token (read-only; it never writes it or uses its refresh token) and fetches the decks it checks itself. If the token has expired, make any dumpstar call in Claude Code to refresh it.

Then ask Claude to build or audit your SWU Jumpstart cube; the `swu-jumpstart-cube` skill drives the dumpstar tools and calls `check_cube` (modes `pack`, `cube` and `copies`, decks named by `deck_ids` or a `group`).

## The checker binary

`bin/swu-cube-check-launch` runs the checker. On first use it downloads the binary pinned in the launcher for your platform (`aarch64-apple-darwin` or `x86_64-unknown-linux-gnu`) from this repo's GitHub release, verifies it against the sha256 pinned in the launcher, caches it under the plugin's data directory and execs it. A checksum mismatch or any other platform is refused with a named error. 

### Releasing

Pushing a `v*` tag builds both targets and attaches them and a `SHA256SUMS` to the release; the workflow opens no pull request. To release: tag `v*`, wait for the Release workflow to go green, run `scripts/pin-release.sh <version>` (it downloads that release's `SHA256SUMS`, rewrites `VERSION` and the two `SHA256_` lines in the launcher, and refuses if a binary is missing from it), then land the resulting diff as a bead through the queue.

The same binary has a CLI, for use outside Claude Code: `swu-cube-check pack --deck ID`, `cube --group NAME`, `copies --group NAME`, `allocate --group NAME`, and `login` (`--help` prints the usage).

## Development

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
sh tests/launcher.sh
```

The digest (`lines_digest`) is implemented here, to the canonical form dumpstar documents in its `get_deck` tool description; `DIGEST_CARD_FIELDS` in `crates/swu-cube-check/src/lib.rs` lists the card fields, and a test keeps the skill's list of them in step.

### Against a real dumpstar

An opt-in test runs `check_cube` against the real `get_deck` output of a dumpstar realenv slot and verifies its digest. It does nothing unless `SWU_CUBE_SITE` is set:

```
SWU_CUBE_SITE=https://<slot-url> SWU_CUBE_GROUP=<group of saved decks> \
  cargo test -p swu-cube-check --test realenv -- --nocapture
```

`SWU_CUBE_GROUP` defaults to `swu-jumpstart-cube`. The test signs in as the checker does: Claude Code's dumpstar sign-in for that site, else `swu-cube-check login --site <url>`.
