#!/usr/bin/env bash
# Two-employee testnet check of the per-employee proof (emp-a, emp-b).
# Never prints the seed. Fails loudly on anything missing.
#
# Needs: PAYROLL_EMPLOYER_SEED, ZCASH_NETWORK=testnet, ZCASH_LWD_URL, ZCASH_WALLET_DB,
#        ZCASH_WALLET_CLI, PAYROLL_CHILD_BIRTHDAY, and a funded treasury.
# Then, after `pay --two-step` printed both PAYOUT BROADCAST txids:
#        PAYROLL_PAYOUT_A / PAYROLL_PAYOUT_B = <txid>:<height>:<raw-tx-hex-file>
#        PAYROLL_EMPLOYEE_A_UA / PAYROLL_EMPLOYEE_B_UA = the two roster UAs (for the cargo test)
set -euo pipefail
cd "$(dirname "$0")/.."
: "${PAYROLL_EMPLOYER_SEED:?PAYROLL_EMPLOYER_SEED not configured}"
ROSTER=${ROSTER:-examples/two-employees.csv}
[ -f "$ROSTER" ] || { echo "ERROR: $ROSTER missing (name,ua,amountZEC,memo; two rows named emp-a and emp-b)" >&2; exit 1; }
mkdir -p .data/proofs
cargo run -q --bin payroll -- export-proof "$ROSTER" emp-a > .data/proofs/emp-a.txt
cargo run -q --bin payroll -- export-proof "$ROSTER" emp-b > .data/proofs/emp-b.txt
echo "proof files: .data/proofs/emp-a.txt .data/proofs/emp-b.txt"

if [ -z "${PAYROLL_PAYOUT_A:-}" ]; then
  echo "Run both steps (spends testnet ZEC):"
  echo "  cargo run --bin payroll -- pay $ROSTER --two-step"
  echo "ERROR: PAYROLL_PAYOUT_A / PAYROLL_PAYOUT_B not configured: no step-2 payout exists to verify" >&2
  exit 1
fi
: "${PAYROLL_PAYOUT_B:?PAYROLL_PAYOUT_B not configured}"
IFS=: read -r TA HA FA <<<"$PAYROLL_PAYOUT_A"
IFS=: read -r TB HB FB <<<"$PAYROLL_PAYOUT_B"
cargo run -q --bin payroll -- verify-proof .data/proofs/emp-a.txt "$FA" "$TA" "$HA"
cargo run -q --bin payroll -- verify-proof .data/proofs/emp-b.txt "$FB" "$TB" "$HB"
if cargo run -q --bin payroll -- verify-proof .data/proofs/emp-a.txt "$FB" "$TB" "$HB" 2>/dev/null; then
  echo "FAIL: emp-a's key decrypted emp-b's payout" >&2; exit 1
fi
echo "OK: emp-a's key cannot see emp-b's payout"
cargo test --test testnet -- --ignored testnet_employee_a_key_cannot_see_employee_b_payout
