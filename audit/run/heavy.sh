#!/bin/bash
# Run only after heavy-slot.sh acquire misthos-reverify has succeeded.
set -u
cd "$(dirname "$0")/../.."
SLOT='/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh'
cleanup() {
  bash e2e/stack.sh stop >> audit/logs/stack-stop.txt 2>&1
  "$SLOT" release misthos-reverify >> audit/logs/slot-release.txt 2>&1
}
trap cleanup EXIT INT TERM
export CARGO_BUILD_JOBS=2
bash e2e/stack.sh > audit/logs/stack-start.txt 2>&1
echo "stack exit=$?"
python3 audit/run/nonbrowser.py api
node audit/run/ui.mjs > audit/logs/ui-run.txt 2>&1
echo "UI exit=$?"
(cd e2e && PLAYWRIGHT_JSON_OUTPUT_NAME=../audit/logs/e2e.json npm run test:e2e -- --reporter=list,json) > audit/logs/e2e.txt 2>&1
echo "Playwright exit=$?"
# Exercise the actual global-setup failure for each URL after browser work is done.
for server in mainnet configured setup; do
  pid=$(awk -v name="$server" '$2==name {print $1}' e2e/.run/pids)
  kill "$pid"
  SERVER_NAME="$server" node --input-type=module - <<'JS' >> audit/logs/harness-down-final.txt 2>&1
import setup from './e2e/global-setup.ts';
try { await setup(); console.log(process.env.SERVER_NAME+': UNEXPECTED setup success');process.exitCode=1 }
catch(e) {console.log(process.env.SERVER_NAME+': '+e.message);if(!e.message.includes('Start it with: npm run stack'))process.exitCode=1}
JS
done
python3 audit/run/e2e-diagnostics.py
python3 audit/run/update.py
