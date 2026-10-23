#!/bin/bash
# Tests for scripts/release-smoke.sh against a stub swu-cube-check: a success reply with no isError
# passes, and the over-limit plan is sent as 16 distinct ids.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*"; fails=$((fails + 1)); }

cards='{"L":{"type":"Leader","name":"Lead"},"B":{"type":"Base"}}'
lines='[]'
for i in $(seq 15); do
  cards=$(jq -c --arg k "c$i" '. + {($k): {type:"Unit"}}' <<<"$cards")
  lines=$(jq -c --arg k "c$i" '. + [{card_id:$k}]' <<<"$lines")
done
cards=$(jq -c '. + {extra:{type:"Event"}}' <<<"$cards")
jq -cn --argjson c "$cards" --argjson l "$lines" '{deck:{name:"Lead - Smoke Test"},cards:$c,lines:$l}' >"$work/doc.json"

cat >"$work/stub" <<'S'
#!/bin/bash
# show: print the fixture deck. mcp: keep the requests, answer with replies that omit isError on success.
if [ "$1" = show ]; then cat "$STUB_DIR/doc.json"; exit 0; fi
cat >"$STUB_DIR/reqs"
tools='{"name":"check_cube","inputSchema":{"properties":{"mode":{"enum":["pack","allocate","copies","cube"]},"deck_ids":{},"group":{},"plan":{},"verbose":{},"fill":{},"site":{}}}}'
echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[$tools]}}"
reply() { jq -cn --argjson id "$1" --arg t "$2" --argjson e "$3" '{jsonrpc:"2.0",id:$id,result:({content:[{type:"text",text:$t}]} + $e)}'; }
reply 2 ok '{}'
reply 3 "allocation verdict" '{}'
reply 4 "L is a Leader" '{"isError":true}'
reply 5 "16 picks: more than 15" '{"isError":true}'
reply 6 "copies verdict" '{}'
reply 7 "$(printf "FAIL pack 'A': leader-mechanic: x\nFAIL pack 'B': leader-mechanic: x\nWARN pack 'C': pack-mechanic: x\nWARN pack 'D': pack-mechanic: x\nWARN pack 'E': pack-mechanic: x\nFAIL cube-mechanic: force consumed by 7 cards, produced by 0")" '{}'
S
chmod +x "$work/stub"

export STUB_DIR=$work SWU_CUBE_BIN=$work/stub SWU_CUBE_PACK_DECK=1
out=$(bash "$root/scripts/release-smoke.sh" stub 2>&1) && rc=0 || rc=$?
[ "$rc" = 0 ] || { fail "success replies without isError did not pass (rc=$rc)"; echo "$out"; }

n=$(jq -s '[.[] | select(.id == 5) | .params.arguments.plan.packs[0].picks | unique | length] | .[0]' "$work/reqs")
t=$(jq -s '[.[] | select(.id == 5) | .params.arguments.plan.packs[0].picks | length] | .[0]' "$work/reqs")
[ "$n" = 16 ] && [ "$t" = 16 ] || fail "16-pick call carried $n distinct of $t ids"

[ "$fails" = 0 ] && echo "release-smoke tests passed"
exit "$fails"
