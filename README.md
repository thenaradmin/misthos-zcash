# Private Payroll Rail (ZECATHON, Shielded Payments track)

This is a batch payroll tool for the Zcash **testnet**. An employer uploads a CSV, and the tool pays the whole batch in one shielded transaction. Each employee gets a payslip, and an auditor gets a read-only view through a viewing key.

## Status (honest)
| Part | State |
|---|---|
| Core engine: employees, schedules, pay runs, per-run caps, run status machine | **Done**, 27 unit tests |
| CSV import and validation (`payroll plan`) | **Done**, offline only |
| Env config that fails loudly | **Done** |
| Full Unified Address decoding (`zcash_address`) | Not started. Today there is only a structural check: the `utest1` prefix, the bech32 charset and the length |
| ZIP-321 URI, building and broadcasting the tx (`zcash_client_backend`) | Not started (M2) |
| Dashboard UI (`payroll serve`): roster, run button, live payout feed, payslips, auditor UFVK panel | Built; offline build + endpoint smoke test pass. No real testnet payout through it yet |

The tool does not use NEAR Intents, does not use the `zakura-one-time-ivk` crate, and has received no grant funding.

## Requirements
- Rust **1.85**, pinned in `rust-toolchain.toml` (`rustup` installs it automatically).
- The core has no dependencies, so `cargo test --offline` works.

## Do X, see Y
1. Run `cargo test`. You should see `27 passed; 0 failed`.
2. Run `cargo run -q -- plan examples/bad.csv`. You should see one error per bad row, for example `line 2: address ... is a MAINNET address`, and the command exits with code 1. Nothing is half-imported.
3. Write a CSV with real testnet Unified Addresses from your own testnet wallets:
   ```
   name,ua,amountZEC,memo
   Alice,utest1...,1.5,"Oct, salary"
   ```
   Then run `cargo run -q -- plan pay.csv --max-total 5 --max-per 2`. You should see a Draft run with each line and an exact zatoshi total. If you go over a cap, every violation is listed at once.
4. Run `cargo run -q -- check-env` without a `.env` loaded. It prints `ERROR: ZCASH_NETWORK not configured` and exits with code 1. If you set `ZCASH_NETWORK=mainnet`, it refuses.

## Engine rules
- **Amounts** are stored as integer zatoshis. Parsing is exact to 8 decimals. Negative values, exponents, zero and amounts above 21M ZEC are rejected.
- **Schedules** can be weekly, biweekly (anchored to a date) or monthly. Monthly schedules on day 29–31 fall back to the last day of the month.
- **Caps** are checked per run: a maximum total, a maximum per employee and a maximum number of recipients (default 50, a policy setting rather than a consensus rule). Caps are checked again at approval, so a cap tightened after planning still applies.
- **Double-pay guard:** each period (`schedule@date` or `adhoc:<label>`) can have only one live run. A new run for the same period is allowed only after the earlier one is marked `Failed`.
- **Status flow:** `Draft → Approved → Broadcast{txid} → Confirmed{height}`, with `Approved | Broadcast → Failed`. Any other transition is an error.
- Memos are limited to 512 bytes (ZIP-321).

## Layout
`src/money.rs` · `date.rs` · `employee.rs` · `schedule.rs` · `engine.rs` · `csv.rs` · `config.rs` · `zip321.rs` · `wallet.rs` · `audit.rs` · `main.rs` (the CLI) · `tests/testnet.rs`.

## Shielded payout + accountant export
- `payroll pay <file.csv>`: plans and approves the run, builds one ZIP-321 URI with many outputs, and sends it as ONE shielded testnet tx through `ZCASH_WALLET_CLI` (zingo-cli) using `ZCASH_LWD_URL`. It prints `BROADCAST txid=…`. If a var is missing it prints `ERROR: <VAR> not configured` and exits 1.
- `payroll audit-export <file.csv> [txid]`: writes a CSV ledger (run, txid, output index, name, address, amount, memo), headed by `ZCASH_AUDITOR_UFVK`. It refuses spending keys and seed phrases.
- `cargo test --offline`: offline unit tests (ZIP-321 encoding, key checks).
- `cargo test --test testnet -- --ignored`: real testnet tests. Without env vars they FAIL with `TESTNET TEST CANNOT RUN: <VAR> not configured`.
- Not verified yet: the exact `send`/`confirm` argument syntax of zingo-cli. Check it before the first real run.

## Rules
The tool uses only the real Zcash testnet, with no mocks and no fake chain data. Secrets go only in `.env` (see `.env.example`). Mainnet is refused in code.

## Employer dashboard

Do: `set -a; . ./.env; set +a; cargo run -- serve` (default `--addr 127.0.0.1:8787`), then open http://127.0.0.1:8787.
See, within a few seconds of opening the page:
- a red banner listing every unset var by name (e.g. `ZCASH_LWD_URL not configured`), or none if `.env` is complete;
- the roster from `PAYROLL_ROSTER_CSV` with the run total;
- **Run payroll**, which fills the live feed step by step: validate → approve → ZIP-321 proposal → wallet sync → send → confirm → one txid, then one payslip line per employee (name, amount, output index, txid, memo). Each step is the real `ZCASH_WALLET_CLI`. The first failure stops the run and shows the CLI's own error;
- **Open auditor view**, which shows the UFVK (view-only; spending keys are refused) and the ledger of this session's runs.

The UI is one dependency-free HTML file (`ui/index.html`), compiled into the binary. Run history lives in memory and is lost on restart.
