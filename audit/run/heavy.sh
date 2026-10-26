#!/bin/bash
# Run only after heavy-slot.sh acquire misthos-audit has succeeded.
set -u
cd "$(dirname "$0")/../.."
SLOT='/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh'
cleanup() {
  bash e2e/stack.sh stop >> audit/logs/stack-stop.txt 2>&1
  "$SLOT" release misthos-audit >> audit/logs/slot-release.txt 2>&1
}
trap cleanup EXIT INT TERM
export CARGO_BUILD_JOBS=2
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo build --offline --bin payroll --lib > audit/logs/build.txt 2>&1
build=$?
echo "build exit=$build"
if [ "$build" -ne 0 ]; then exit "$build"; fi
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline > audit/logs/cargo-test.txt 2>&1
echo "cargo test exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline --lib > audit/logs/cargo-lib.txt 2>&1
echo "cargo lib exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline --test cli_label --test cli_serve --test proof_real_tx --test sig_attacks > audit/logs/cargo-integrations.txt 2>&1
echo "cargo integrations exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 rustc --edition=2021 audit/run/probe.rs --extern zec_payroll=target/debug/libzec_payroll.rlib -L dependency=target/debug/deps -o target/audit-probe > audit/logs/probe-build.txt 2>&1
if [ "$?" -eq 0 ]; then env -i PATH="$PATH" target/audit-probe > audit/logs/probe.txt 2>&1; fi
python3 audit/run/nonbrowser.py cli
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
python3 audit/run/update.py
