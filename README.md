# Private Payroll Rail (ZECATHON, Shielded Payments track)

This is a batch payroll tool for the Zcash **testnet**. An employer uploads a CSV, and the tool pays the whole batch in one shielded transaction. Each employee gets a payslip, and an auditor gets a read-only view through a viewing key.

## Status (honest)
| Part | State |
|---|---|
| Core engine: employees, schedules, pay runs, per-run caps, run status machine | **Done**, unit tested (`cargo test`: 38 lib tests) |
| CSV import and validation (`payroll plan`) | **Done**, offline only |
| Env config that fails loudly | **Done** |
| Recipient address decoding (`zcash_address` 0.14.0-pre.2) | **Done**: full decode (checksum, F4Jumble, typecodes); testnet only; must have a Sapling or Orchard receiver (`utest1…` UA or `ztestsapling1…`). Mainnet and transparent-only are refused |
| ZIP-321 URI (`src/zip321.rs`) | **Done**, offline-tested. Shown for display/audit only; zingo-cli does not accept ZIP-321 URIs |
| Building + broadcasting the tx | **Delegated to zingo-cli** (`ZCASH_WALLET_CLI`) via one `quicksend` with a JSON receiver list. Syntax checked against zingolib source (0a5cb69, 2026-10-19) and run live against testnet with an unfunded wallet: it syncs and stops at `Insufficient balance (have 0, need 110000 including fee)`. **Never run against a funded wallet: no testnet payout has happened yet.** No in-process `zcash_client_backend` builder |
| Dashboard UI (`payroll serve`): roster, run button, live payout feed, payslips, auditor UFVK panel | Built; offline build + endpoint smoke test pass. No real testnet payout through it yet |
| Per-employee proof of pay: child wallet per employee, `export-proof`, `verify-proof`, dashboard **Export proof for employee** | Built. 3 real-key tests on in-process shielded txs pass (Sapling + Ironwood). Both steps are wired to zingo-cli (child wallet per employee, pre-broadcast sync check). No funded testnet run yet |

The tool does not use NEAR Intents, does not use the `zakura-one-time-ivk` crate, and has received no grant funding.

## Requirements
- Rust **1.89**, pinned in `rust-toolchain.toml`. Testnet has activated NU7 (consensus branch `0x77190ad9`, v6 txs): stable `zcash_primitives` 0.30.1 rejects current testnet txs (`invalid consensus branch id 0x77190ad9`) and the stable crates also need Rust ≥1.88, so this repo pins the same prerelease line as zec-shielded-x402 (`zcash_address =0.14.0-pre.2`, `zcash_protocol =0.11.0-pre.1`).
- The only dependencies are those two crates; after one `cargo fetch`, `cargo test --offline` works.

- `zingo-cli` from current zingolib needs `nym-proxy` beside it (Mixnet Mode is mandatory for online sessions): `makers bundle-nym-proxy`, or `cargo run --manifest-path tools/workbench/Cargo.toml --bin bundle-nym-proxy -- --release --dest <dir of zingo-cli>`. Without it every wallet call fails with `Mixnet Mode is required for a connected session`.

## Known limits
- The address check decodes and checks network + receiver types, but does not check that the receiver bytes are valid curve points. zingo-cli does, and rejects such an address loudly at send time (`Invalid Orchard receiver in Unified Address`), before anything is broadcast.

## Do X, see Y
1. Run `cargo test`. You should see `38 passed; 0 failed` (plus 2 ignored live-testnet tests).
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
- **Double-pay guard:** each period (`schedule@date` or `adhoc:<label>`) can have only one live run. The dashboard uses `adhoc:dashboard@<UTC date>`, so a second **Run payroll** the same day is refused. A new run for the same period is allowed only after the earlier one is marked `Failed`.
- **Status flow:** `Draft → Approved → Broadcast{txid} → Confirmed{height}`, with `Approved | Broadcast → Failed`. Any other transition is an error.
- Memos are limited to 512 bytes (ZIP-321).

## Layout
`src/money.rs` · `date.rs` · `employee.rs` · `schedule.rs` · `engine.rs` · `csv.rs` · `config.rs` · `zip321.rs` · `wallet.rs` · `audit.rs` · `main.rs` (the CLI) · `tests/testnet.rs`.

## Shielded payout + accountant export
- `payroll pay <file.csv>`: plans and approves the run, prints the ZIP-321 URI, then runs `ZCASH_WALLET_CLI --chain testnet --server $ZCASH_LWD_URL --data-dir $ZCASH_WALLET_DB --waitsync quicksend '[{"address":…,"amount":<zatoshis>,"memo":…},…]'` (one proposal for all recipients; zingolib normally makes one tx but may split it). It prints `BROADCAST txid=…` for each txid in zingo-cli's `{"txids":[…]}` output. A non-zero exit, or output without a `txids` array, is an error; nothing is marked broadcast. If a var is missing it prints `ERROR: <VAR> not configured` and exits 1.
- `payroll audit-export <file.csv> [txid]`: writes a CSV ledger (run, txid, line index (row order, not the on-chain output index), name, address, amount, memo), headed by `ZCASH_AUDITOR_UFVK`. It refuses spending keys and seed phrases.
- `cargo test --offline`: offline unit tests (ZIP-321 encoding, key checks).
- `cargo test --test testnet -- --ignored`: real testnet tests. Without env vars they FAIL with `TESTNET TEST CANNOT RUN: <VAR> not configured`.
- `audit-export`'s optional `[txid]` is taken from the operator as-is (it must be 64 hex) and is NOT checked on-chain.
- Not verified yet: a real paid run. It needs a funded testnet wallet in `ZCASH_WALLET_DB`.

## Rules
The tool uses only the real Zcash testnet, with no mocks and no fake chain data. Unit tests use correctly encoded but synthetic testnet addresses that are never broadcast. Secrets go only in `.env` (see `.env.example`). Mainnet is refused in code.

## Employer dashboard

Do: `set -a; . ./.env; set +a; cargo run -- serve` (default `--addr 127.0.0.1:8787`), then open http://127.0.0.1:8787.
See, within a few seconds of opening the page:
- a red banner listing every unset var by name (e.g. `ZCASH_LWD_URL not configured`), or none if `.env` is complete;
- the roster from `PAYROLL_ROSTER_CSV` with the run total;
- **Run payroll**, which fills the live feed step by step: validate → approve → ZIP-321 URI (display) → wallet sync + `quicksend` → txid(s), then one payslip line per employee (name, amount, txid, memo). The wallet step is the real `ZCASH_WALLET_CLI`. The first failure stops the run and shows the CLI's own error;
- **Open auditor view**, which shows the UFVK (view-only; spending keys are refused) and the ledger of this session's runs.

The UI is one dependency-free HTML file (`ui/index.html`), compiled into the binary. Run history lives in memory and is lost on restart.

## Per-employee proof of pay

An employee can prove their own pay (amount, recipient, memo, txid, block) to a lender or tax office without exposing the rest of payroll.

**Design chosen: one child wallet per employee.** Each employee gets a child seed: 32 bytes = keyed BLAKE2b-256 (key = employer BIP 39 seed, personalization `zecpayroll_child`, input = the employee's payout address). It is encoded as a 24-word BIP 39 mnemonic and used at ZIP 32 account 0, in its own zingo-cli data dir at `<ZCASH_WALLET_DB>/children/child-<hash of address>`. Only the employer seed is needed to rebuild every child wallet. Payout addresses are the input because roster ids are row numbers that change on reorder, so a new payout address means a new child wallet. Duplicate addresses are refused. The treasury (employer seed, account 0) viewing key is **never exported**: `export` refuses it.

Why not ZIP 32 accounts under one seed: zingo-cli creates exactly one account per wallet (`no_of_accounts = 1`), so it could not spend from account N. A separate child seed makes step 2 an ordinary `quicksend`. Why not ZIP 311 payment disclosures: ZIP 311 is still a draft, and the pinned crates have no implementation of it. It is future work. It would allow a per-*payment* proof without the extra funding transaction.

**Two-step payout (one extra tx per employee per run), `payroll pay <csv> --two-step`:**
0. **Pre-broadcast check:** every employee's child wallet is created (restored from its mnemonic at `PAYROLL_CHILD_BIRTHDAY`, passed through the `ZINGO_SEED` env var, never argv) or opened, and synced (`balance --waitsync`). If any child wallet fails, the run stops and **step 1 is not sent**.
1. **Funding:** the treasury sends `pay + 0.0001 ZEC` to each child wallet's UA, in one `quicksend`. This costs the normal ZIP 317 fee of `5_000 zat x max(2, logical actions)`.
2. **Payout:** each child wallet makes one `quicksend` of `pay` to the employee. Funding is exact, so this is 1 note in and 1 output with no change, which costs `10_000 zat` (2 padded actions x 5_000). That fee is what the 0.0001 ZEC top-up covers. Outputs are encrypted with the child wallet's OVK. The command retries every 60 s until the funding note is spendable (`PAYROLL_STEP2_TIMEOUT_SECS`, default 1800). On timeout it exits 1 and names the unpaid employees, and the funds stay in child wallets the employer seed can rebuild.

**Not yet verified:** no funded testnet run of either step. The zingo-cli restore via `ZINGO_SEED` + `--birthday` follows the zingo-cli source but has not run live. If a first restore fails halfway, delete that child dir before retrying.

**What the export contains:** the child wallet's account-0 **full** viewing key (UFVK `uviewtest1...`). It is a full key, not an incoming-only one, because payouts are *outgoing* from that wallet and are recovered with its OVK. Trust model:
- It reveals **all** of that employee's payouts from this employer, past and future (not just one), plus the funding notes into that wallet, the amounts, and the employer-chosen memos. Anyone holding the file can see this forever. There is no revocation.
- It reveals nothing about other employees' child wallets or the treasury. Child seeds are one-way from the employer seed, so a child key reveals nothing about the employer seed. Tests show that A's key cannot decrypt B's payout, even when both outputs are in the same transaction, and that it sees only its own funding output in a shared funding transaction.
- The lender trusts that the employer controls this child wallet. The proof shows "this wallet paid X to address Y", and linking the account to the employer is a social claim (for example, the employer signs or publishes the account UA).
- The sending side of the treasury -> child wallet funding tx is not hidden from the treasury key holder, of course. Timing correlation between funding and payout transactions is visible on-chain as two shielded transactions.
- The seed (`PAYROLL_EMPLOYER_SEED`, a BIP 39 mnemonic) is never printed. `EmployerSeed`'s `Debug` output is redacted.

**Commands:**
- `payroll export-proof <roster.csv> <employee id|name>`: prints the proof file.
- `payroll verify-proof <proof-file> <raw-tx-hex-file> <txid> <block-height>`: the third-party check. It parses the raw transaction (from any explorer or lightwalletd), checks that the txid matches, trial-decrypts it with the UFVK, and prints each visible output with its direction (`PayoutOut`/`FundingIn`/`Change`), pool, amount, recipient receiver bytes and memo. Read the date from the block at `<block-height>`. It exits 1 with "sees nothing" if the transaction isn't this employee's.
- Dashboard: an **Export proof for employee** button on each roster row downloads the proof file (`GET /api/proof?employee=<id>`).

**Tests:** `tests/proof_real_tx.rs` uses real ZIP 32 keys, real note encryption with each child wallet's OVK, and real Halo2 proofs for Ironwood. The transparent inputs are made up, and Sapling zk-proof bytes are mocks, so the chain would reject these transactions. Every byte the verifier reads is real. The live check `scripts/testnet-two-employees.sh` and the `--ignored` test `testnet_employee_a_key_cannot_see_employee_b_payout` exit or panic, naming the missing variable, because no funded step-2 payout exists yet.

The older auditor export (`ZCASH_AUDITOR_UFVK`) is the *employer* key and shows all of payroll. It is for the company's accountant only, never for employees.
