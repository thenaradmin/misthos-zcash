# Testing

Every suite count below was counted on **2026-10-28**, after the payroll desk's payout mode was added, from the commands shown (the 2026-10-26 audit counts are in `audit/`). Nothing in these suites talks to a mocked chain: unit tests use synthetic, correctly encoded testnet addresses and in-process transactions, the E2E suite runs the real `payroll serve` binary, and the live tests below need real testnet variables and are `#[ignore]`d by default.

## Rust (`cargo test --offline`, Rust 1.89 from `rust-toolchain.toml`)

| Suite | Command | Passed | What it covers |
|---|---|---|---|
| Library unit tests | `cargo test --offline --lib` | **82** | money, dates, schedules, engine and caps, CSV import (incl. row-id names refused, `select_employee`), config, ZIP-321, wallet CLI wrapper and per-child ledger (incl. wallet-DB lock), delays, disclosure, ed25519 attestation (incl. the cwd key-file warning), accountant CSV (incl. formula neutralising), desk server (Host check, 431 caps, stalled client, connection slots, 410 endpoints, CSRF guard on POSTs, chunked/oversized bodies), payout mode (pay-run output parsed into per-employee progress, period labels, roster snapshots, secret scrubbing, zingo-cli `balance`/`value_transfers` parsing and the nothing-left rule, the JSON body parser) |
| `tests/cli_label.rs` | `cargo test --offline --test cli_label` | **4** | `pay --two-step` and `export-proof` need `--label`; a held wallet-DB lock stops a second `pay --two-step` before any `ZCASH_WALLET_CLI` call; `export-proof` usage says the amount comes from the roster |
| `tests/cli_serve.rs` | `cargo test --offline --test cli_serve` | **1** | `serve --addr 0.0.0.0:0` is refused without `--allow-remote` |
| `tests/desk_api.rs` | `cargo test --offline --test desk_api` | **8** | the real binary over HTTP: `--payouts` refused off loopback; read-only desk answers 410; caps violations listed; roster upload validated; a run streams the CLI and stops before broadcast (wallet CLI `false`); a different roster for the same period refused; proof export refuses an unpaid period; an unknown payout outcome is not cleared when the chain check fails; accountant rows with the employer UFVK; a bad proof refused |
| `tests/proof_real_tx.rs` | `cargo test --offline --test proof_real_tx` | **7** | real ZIP 32 keys, real note encryption, real Halo2 (Ironwood) and real ed25519 signatures; forged/edited proof rejection; an edited `employee_id` still verifies and is printed only as unsigned (CLI, offline mode); a planted `docs/employer-pubkey.txt` in the cwd is warned about with its absolute path |
| `tests/sig_attacks.rs` | `cargo test --offline --test sig_attacks` | **1** | signature forgeries, field changes, field-boundary smuggling, truncated signatures |
| **Total** | `cargo test --offline` | **103 passed, 0 failed, 4 ignored** | |

Synthetic parts of `proof_real_tx`: the transparent inputs spend made-up outpoints and the Sapling zk-proof bytes are sapling-crypto's mock proofs, so the chain would reject those transactions. The on-chain check is the live run in the README ("Proven on testnet").

### Ignored live-testnet tests (`cargo test --test testnet -- --ignored`)

These four tests were not executed on 2026-10-26. Without their variables each one panics with `TESTNET TEST CANNOT RUN: <VAR> not configured`; none of them is counted as passing above.

| Test | Needs |
|---|---|
| `lightwalletd_reports_chain_height` | `ZCASH_NETWORK=testnet`, `ZCASH_LWD_URL`, `ZCASH_WALLET_DB`, `ZCASH_WALLET_CLI` |
| `lightwalletd_grpc_tip_height_for_delays` | `ZCASH_LWD_URL` (printed `testnet tip 4521111` against testnet.zec.rocks on 2026-10-20) |
| `sends_one_shielded_tx_to_all_csv_recipients` | the four chain vars above, `PAYROLL_TEST_CSV`, and a funded wallet (spends testnet ZEC) |
| `testnet_employee_a_key_cannot_see_employee_b_payout` | `PAYROLL_EMPLOYER_SEED`, `PAYROLL_PAYOUT_A`, `PAYROLL_PAYOUT_B` (`<txid>:<height>:<raw-tx-hex-file>`), `PAYROLL_EMPLOYEE_A_UA`, `PAYROLL_EMPLOYEE_B_UA` |

The executed Rust suite commands ran with `env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2`; full and named-suite logs are in `audit/logs/cargo-*.txt`. The named integration suites were also rerun together with `cargo test --offline --test cli_label --test cli_serve --test proof_real_tx --test sig_attacks`; the counts matched the full run. The main binary and doctest targets each ran 0 tests. The ignored-test command above is documented only and was never run.

## Browser E2E (`e2e/`, Playwright 1.63.0 in Google Chrome)

```
CARGO_BUILD_JOBS=2 cargo build --offline
cd e2e && npm install
npm run stack          # 4 real desk servers on 127.0.0.1:8791-8794, env -i (no .env, real seed or key; 8794 runs --payouts with a failing wallet CLI)
npm run test:e2e
npm run stack:stop     # kills those PIDs
```

| Spec | Tests | Desktop 1280×800 | Mobile 375×812 |
|---|---|---|---|
| `tests/landing.spec.ts` (payslip page) | 11 | 11 passed | 11 passed |
| `tests/desk.spec.ts` (payroll desk at `/desk`) | 10 | 10 passed | 10 passed |
| **Total** | **21** | **21** | **21** → **42 passed, 0 failed** (2026-10-28) |

The 2026-10-26 final audit runs `npm run test:e2e -- --reporter=list,json` through this config, plus independent assertions for every UI audit row in both viewports. Console and network diagnostics are retained in `audit/logs/`; videos and traces remain untracked.

Every E2E test also fails on any console error, page error, failed same-origin request or same-origin HTTP status ≥ 400, with expected API refusal assertions recorded separately. Deliberately stopping the server produces the expected unreachable-state diagnostics. The spending-key refusal has no error exemption and makes no failing fetch. The desk tests cover Setup required, the read-only desk (410 on the payout endpoints), DNS rebinding (`Host: attacker.example` gets `403` on `/desk` and `/api/state`) and cross-site POSTs (403), mainnet refusal, every cap violation listed, roster upload errors, a pay run that stops at the CLI's pre-broadcast check (wallet CLI `false` on 8794, so nothing is sent), the payslip register, the accountant view and CSV download, a rejected proof file, and hash routing.

## Other checks

- `env -i PATH="$PATH" target/debug/payroll plan examples/bad.csv` (2026-10-26) exits 1 with one error per bad row (README "Do X, see Y").
- `env -i PATH="$PATH" target/debug/payroll check-env` (2026-10-26) with no `.env` exits 1 with `ERROR: ZCASH_NETWORK not configured`.
