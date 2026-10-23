#!/bin/sh
# Usage: scripts/check-version-bump.sh <base-ref>
# Fails when the plugin version is not bumped for a change to the plugin, or when Cargo's
# workspace version differs from .claude-plugin/plugin.json's. Claude Code updates an installed
# plugin only on a version change, so a change to crates/, skills/, bin/ or .mcp.json that keeps
# the base's version never reaches users. SWU_CUBE_ROOT overrides the repo root (the tests use it).
set -eu

die() {
    printf 'check-version-bump: %s\n' "$*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: check-version-bump.sh <base-ref>"
base=$1
root=${SWU_CUBE_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}

plugin_version() { sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1; }

plugin=$(plugin_version < "$root/.claude-plugin/plugin.json")
cargo=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$root/Cargo.toml" | head -n 1)
[ -n "$plugin" ] || die "no version in .claude-plugin/plugin.json"
[ -n "$cargo" ] || die "no version in Cargo.toml"
[ "$plugin" = "$cargo" ] || die "Cargo.toml version $cargo != .claude-plugin/plugin.json version $plugin; make them equal"

changed=$(git -C "$root" diff --name-only "$base" -- crates skills bin .mcp.json)
[ -n "$changed" ] || exit 0

basever=$(git -C "$root" show "$base:.claude-plugin/plugin.json" | plugin_version)
if [ "$plugin" = "$basever" ]; then
    printf '%s\n' "$changed" >&2
    die "the plugin changed but .claude-plugin/plugin.json is still at $basever; bump the version (and Cargo.toml's) so installed plugins update"
fi
