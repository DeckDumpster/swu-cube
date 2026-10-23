#!/usr/bin/env bash
# Release smoke: real check_cube calls through the built binary's MCP stdio server against a real
# dumpstar site, signed in as users are (Claude Code's dumpstar sign-in, else `swu-cube-check login`).
# No v* tag is cut until this passes on the candidate commit.
#
#   scripts/release-smoke.sh <site>
#
# Environment:
#   SWU_CUBE_PACK_DECK  id of a known-good saved pack on <site> (required)
#   SWU_CUBE_GROUP      a group of saved decks there (default swu-jumpstart-cube)
#   SWU_CUBE_BIN        the swu-cube-check binary (default target/release/swu-cube-check)
#
# Prints every call's output; exits non-zero if any assertion failed.
set -u

site=${1:?usage: release-smoke.sh <site>}
deck=${SWU_CUBE_PACK_DECK:?set SWU_CUBE_PACK_DECK to the id of a known-good saved pack}
group=${SWU_CUBE_GROUP:-swu-jumpstart-cube}
root=$(cd "$(dirname "$0")/.." && pwd)
bin=${SWU_CUBE_BIN:-$root/target/release/swu-cube-check}
skill=$root/skills/swu-jumpstart-cube/SKILL.md
[ -x "$bin" ] || { echo "no binary at $bin (cargo build --release -p swu-cube-check, or set SWU_CUBE_BIN)" >&2; exit 2; }

failed=0
check() { # check <description> <condition status>
  if [ "$2" -eq 0 ]; then echo "PASS $1"; else echo "FAILED ASSERTION: $1"; failed=1; fi
}

doc=$("$bin" show --deck "$deck" --site "$site") || { echo "cannot fetch deck $deck from $site" >&2; exit 2; }
leader=$(jq -r '[.cards | to_entries[] | select(.value.type == "Leader") | .key][0] // empty' <<<"$doc")
draw=$(jq -r '. as $d | [$d.lines[].card_id | select(. as $i | ($d.cards[$i].type) as $t | ($t != "Leader" and $t != "Base"))][0] // empty' <<<"$doc")
[ -n "$leader" ] && [ -n "$draw" ] || { echo "deck $deck has no leader and draw card" >&2; exit 2; }

# 16 distinct draw-card ids, real where the deck has them, repeated otherwise: the count is what is refused.
sixteen=$(jq -c --arg d "$draw" '[range(16) | $d]' <<<null)

reqs=$(mktemp)
trap 'rm -f "$reqs"' EXIT
n=2
call() { # call <arguments json>
  jq -cn --argjson id "$n" --argjson a "$1" '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:"check_cube",arguments:$a}}' >>"$reqs"
  n=$((n + 1))
}
{
  echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"release-smoke","version":"0"}}}'
  echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
} >"$reqs"
call "$(jq -cn --argjson d "$deck" '{mode:"pack",deck_ids:[$d]}')"                                  # 2
call "$(jq -cn --arg c "$draw" '{mode:"allocate",plan:{packs:[{name:"smoke",picks:[$c]}]}}')"          # 3
call "$(jq -cn --arg c "$leader" '{mode:"allocate",plan:{packs:[{name:"smoke",picks:[$c]}]}}')"       # 4
call "$(jq -cn --argjson p "$sixteen" '{mode:"allocate",plan:{packs:[{name:"smoke",picks:$p}]}}')"    # 5
call "$(jq -cn --arg g "$group" '{mode:"copies",group:$g}')"                                          # 6
call "$(jq -cn --arg g "$group" '{mode:"cube",group:$g}')"                                            # 7

replies=$("$bin" mcp --site "$site" <"$reqs")
reply() { jq -c "select(.id == $1)" <<<"$replies"; }
text() { reply "$1" | jq -r '.result.content[0].text // .error.message // "(no reply)"'; }
iserr() { reply "$1" | jq -r '.result.isError // true'; }

echo "== tools/list"; reply 1 | jq .
for n in 2 3 4 5 6 7; do echo "== call $n"; text "$n"; done
echo

tools=$(reply 1)
# tools/list matches every mode and argument the skill documents.
modes=$(jq -r '.result.tools[] | select(.name == "check_cube") | .inputSchema.properties.mode.enum[]' <<<"$tools")
props=$(jq -r '.result.tools[] | select(.name == "check_cube") | .inputSchema.properties | keys[]' <<<"$tools")
bad=""
for m in $(grep -o '`mode: [a-z]*`' "$skill" | sed 's/`mode: \(.*\)`/\1/' | sort -u); do
  grep -qx "$m" <<<"$modes" || bad="$bad mode:$m"
done
for p in mode deck_ids group plan verbose fill site; do
  grep -qx "$p" <<<"$props" || bad="$bad arg:$p"
done
check "tools/list offers every mode and argument the skill documents (missing:${bad:- none})" "$([ -z "$bad" ]; echo $?)"

check "pack on deck $deck returns ok" "$([ "$(text 2)" = ok ] && [ "$(iserr 2)" = false ]; echo $?)"
check "allocate with a legal draw pick and no queries is not an error" "$([ "$(iserr 3)" = false ]; echo $?)"
leader_text=$(text 4)
check "allocate refuses a leader pick, naming it ($leader)" "$([ "$(iserr 4)" = true ] && grep -q "$leader" <<<"$leader_text" && grep -qi leader <<<"$leader_text"; echo $?)"
check "allocate refuses 16 picks" "$([ "$(iserr 5)" = true ] && grep -q '16 picks' <<<"$(text 5)"; echo $?)"
check "copies over group '$group' is a verdict, not an error" "$([ "$(iserr 6)" = false ]; echo $?)"

# Every FAIL line names something: no empty lists, no empty slots.
empty=$(for i in 2 3 4 5 6 7; do text "$i"; done | grep -E '^(deck [0-9]+: )?FAIL ' | grep -E '\[\]|\{\}|\(\)|: *$|: *,|, *$|""|'"''" || true)
check "every FAIL line names something (empty: ${empty:-none})" "$([ -z "$empty" ]; echo $?)"

[ "$failed" -eq 0 ] && echo "release-smoke: all assertions passed" || echo "release-smoke: FAILED"
exit "$failed"
