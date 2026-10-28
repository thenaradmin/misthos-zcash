#!/usr/bin/env bash
# Start (or stop) the real Misthos desk servers for the E2E run. Every server is the real
# `payroll serve` binary on 127.0.0.1, started with a clean environment (`env -i`), so no
# .env, seed or signing key is ever loaded. PIDs go to e2e/.run/pids; `stop` kills by PID.
#
#   8791  no chain vars          -> the desk's "Setup required" state (what a fresh checkout shows)
#   8792  all five chain vars    -> "Chain config loaded"; auditor UFVK = synthetic fixture key
#   8793  ZCASH_NETWORK=mainnet  -> refused; auditor var holds spending-key-looking text -> refused
#   8794  --payouts              -> payout mode with every var set, but ZCASH_WALLET_CLI=false (always fails), a
#                                   public BIP 39 test-vector seed and a throwaway signing key: every run stops at
#                                   the CLI's own pre-broadcast check, so nothing can be sent
set -euo pipefail
cd "$(dirname "$0")"
RUN=.run
BIN=../target/debug/payroll
mkdir -p "$RUN"

if [[ "${1:-}" == "stop" ]]; then
  [[ -f $RUN/pids ]] || { echo "no $RUN/pids"; exit 0; }
  while read -r pid name; do
    if kill "$pid" 2>/dev/null; then echo "killed $name ($pid)"; else echo "$name ($pid) already gone"; fi
  done < "$RUN/pids"
  rm -f "$RUN/pids"
  exit 0
fi

[[ -x $BIN ]] || { echo "build first: CARGO_BUILD_JOBS=2 cargo build (from the repo root)"; exit 1; }
[[ -f fixtures/roster.csv ]] || python3 -I make-fixtures.py
: > "$RUN/pids"
ROSTER="$PWD/fixtures/roster.csv"
UFVK="$(cat fixtures/auditor-ufvk.txt)"
start() { # name port env...
  local name=$1 port=$2; shift 2
  local flags=(); [[ $name == payouts ]] && flags=(--payouts)
  env -i PATH="$PATH" PAYROLL_ROSTER_CSV="$ROSTER" "$@" "$BIN" serve --addr "127.0.0.1:$port" ${flags[@]+"${flags[@]}"} > "$RUN/$name.log" 2>&1 &
  echo "$! $name" >> "$RUN/pids"
  echo "$name pid $! on http://127.0.0.1:$port"
}
start setup 8791
start configured 8792 ZCASH_NETWORK=testnet ZCASH_LWD_URL=https://testnet.zec.rocks:443 \
  ZCASH_WALLET_DB="$PWD/$RUN/no-wallet.sqlite" ZCASH_WALLET_CLI=zingo-cli ZCASH_AUDITOR_UFVK="$UFVK"
start mainnet 8793 ZCASH_NETWORK=mainnet ZCASH_LWD_URL=https://testnet.zec.rocks:443 \
  ZCASH_WALLET_DB="$PWD/$RUN/no-wallet.sqlite" ZCASH_WALLET_CLI=zingo-cli ZCASH_AUDITOR_UFVK="secret-extended-key-test1-not-a-real-key"
rm -rf "$RUN/payout-wallet" && mkdir -p "$RUN/payout-wallet"
start payouts 8794 ZCASH_NETWORK=testnet ZCASH_LWD_URL=https://testnet.zec.rocks:443 \
  ZCASH_WALLET_DB="$PWD/$RUN/payout-wallet" ZCASH_WALLET_CLI=false ZCASH_AUDITOR_UFVK="$UFVK" \
  PAYROLL_EMPLOYER_SEED="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art" \
  PAYROLL_CHILD_BIRTHDAY=4500000 PAYROLL_EMPLOYER_SIGNING_KEY=0707070707070707070707070707070707070707070707070707070707070707 \
  PAYROLL_DELAY_MIN_BLOCKS=1 PAYROLL_DELAY_MAX_BLOCKS=2
for port in 8791 8792 8793 8794; do
  for _ in $(seq 50); do curl -sf "http://127.0.0.1:$port/api/state" > /dev/null && break; sleep 0.1; done
  curl -sf "http://127.0.0.1:$port/api/state" > /dev/null || { echo "port $port did not come up:"; cat "$RUN"/*.log; exit 1; }
done
echo "stack up"
