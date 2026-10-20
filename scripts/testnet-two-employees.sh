#!/usr/bin/env bash
# Two-employee testnet check of the per-employee signed proof (emp-a, emp-b).
# Never prints the seed or the signing key. Fails loudly on anything missing.
#
# Needs: PAYROLL_EMPLOYER_SEED, ZCASH_NETWORK=testnet, ZCASH_LWD_URL, ZCASH_WALLET_DB,
#        ZCASH_WALLET_CLI, PAYROLL_CHILD_BIRTHDAY, PAYROLL_EMPLOYER_SIGNING_KEY_FILE
#        (or PAYROLL_EMPLOYER_SIGNING_KEY), and a funded treasury.
# Run `pay --two-step` first (it spends testnet ZEC). Then:
#        PAYROLL_PAYOUT_A / PAYROLL_PAYOUT_B = <txid>:<height>:<raw-tx-hex-file>
#        PAYROLL_EMPLOYEE_A_UA / PAYROLL_EMPLOYEE_B_UA = the two roster UAs (for the cargo test)
#
# Timing, for the demo narration: `pay --two-step` waits for the funding tx to be mined, then
# a random 2..=6 blocks, then pays emp-a; emp-b goes a further random 2..=6 blocks later
# (PAYROLL_DELAY_MIN_BLOCKS / PAYROLL_DELAY_MAX_BLOCKS). Expect roughly 5-25 minutes on testnet.
# The delays make the link weaker, NOT gone: anyone watching the chain still sees a funding
# tx followed within a few blocks by payouts, and on a quiet testnet that alone can tie them.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${PAYROLL_EMPLOYER_SEED:?PAYROLL_EMPLOYER_SEED not configured}"
ROSTER=${ROSTER:-examples/two-employees.csv}
LABEL=${LABEL:-two-employees}
[ -f "$ROSTER" ] || { echo "ERROR: $ROSTER missing (name,ua,amountZEC,memo; two rows named emp-a and emp-b)" >&2; exit 1; }

if [ -z "${PAYROLL_PAYOUT_A:-}" ]; then
  echo "Run both steps first (spends testnet ZEC; prints FUNDING MINED, PAYOUT SCHEDULED, WAIT, PAYOUT BROADCAST):"
  echo "  cargo run --bin payroll -- pay $ROSTER --two-step --label $LABEL"
  echo "ERROR: PAYROLL_PAYOUT_A / PAYROLL_PAYOUT_B not configured: no step-2 payout exists to verify" >&2
  exit 1
fi
: "${PAYROLL_PAYOUT_B:?PAYROLL_PAYOUT_B not configured}"

# One signed proof per employee, each from that child's ledger (`paid` txid for this label).
mkdir -p .data/proofs
chmod 700 .data .data/proofs
cargo run -q --bin payroll -- export-proof "$ROSTER" --employee emp-a --label "$LABEL" > .data/proofs/emp-a.txt
cargo run -q --bin payroll -- export-proof "$ROSTER" --employee emp-b --label "$LABEL" > .data/proofs/emp-b.txt
echo "proof files: .data/proofs/emp-a.txt .data/proofs/emp-b.txt"

IFS=: read -r TA HA FA <<<"$PAYROLL_PAYOUT_A"
IFS=: read -r TB HB FB <<<"$PAYROLL_PAYOUT_B"
# Against the chain (signature checked against docs/employer-pubkey.txt first).
cargo run -q --bin payroll -- verify-proof .data/proofs/emp-a.txt "$TA" --lwd "$ZCASH_LWD_URL"
cargo run -q --bin payroll -- verify-proof .data/proofs/emp-b.txt "$TB" --lwd "$ZCASH_LWD_URL"
# emp-a's signed proof must not verify emp-b's payout (signed txid differs, and the key sees nothing).
if cargo run -q --bin payroll -- verify-proof .data/proofs/emp-a.txt "$FB" "$TB" "$HB" 2>/dev/null; then
  echo "FAIL: emp-a's proof accepted emp-b's payout" >&2; exit 1
fi
echo "OK: emp-a's proof cannot claim emp-b's payout"
cargo test --test testnet -- --ignored testnet_employee_a_key_cannot_see_employee_b_payout
