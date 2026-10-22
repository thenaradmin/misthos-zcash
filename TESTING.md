# Testing

Every number below was counted on **2026-10-23** on `main`, from the commands shown. Nothing in these suites talks to a mocked chain: unit tests use synthetic, correctly encoded testnet addresses and in-process transactions, the E2E suite runs the real `payroll serve` binary, and the live tests below need real testnet variables and are `#[ignore]`d by default.

## Rust (`cargo test --offline`, Rust 1.89 from `rust-toolchain.toml`)

| Suite | Command | Passed | What it covers |
|---|---|---|---|
| Library unit tests | `cargo test --offline --lib` | **72** | money, dates, schedules, engine and caps, CSV import (incl. row-id names refused, `select_employee`), config, ZIP-321, wallet CLI wrapper and per-child ledger (incl. wallet-DB lock), delays, disclosure, ed25519 attestation (incl. the cwd key-file warning), accountant CSV (incl. formula neutralising), desk server (Host check, 431 caps, stalled client, connection slots, 410 endpoints) |
| `tests/cli_label.rs` | `cargo test --offline --test cli_label` | **4** | `pay --two-step` and `export-proof` need `--label`; a held wallet-DB lock stops a second `pay --two-step` before any `ZCASH_WALLET_CLI` call; `export-proof` usage says the amount comes from the roster |
| `tests/cli_serve.rs` | `cargo test --offline --test cli_serve` | **1** | `serve --addr 0.0.0.0:0` is refused without `--allow-remote` |
| `tests/proof_real_tx.rs` | `cargo test --offline --test proof_real_tx` | **7** | real ZIP 32 keys, real note encryption, real Halo2 (Ironwood) and real ed25519 signatures; forged/edited proof rejection; an edited `employee_id` still verifies and is printed only as unsigned (CLI, offline mode); a planted `docs/employer-pubkey.txt` in the cwd is warned about with its absolute path |
| `tests/sig_attacks.rs` | `cargo test --offline --test sig_attacks` | **1** | signature forgeries, field changes, field-boundary smuggling, truncated signatures |
| **Total** | `cargo test --offline` | **85 passed, 0 failed, 4 ignored** | |

Synthetic parts of `proof_real_tx`: the transparent inputs spend made-up outpoints and the Sapling zk-proof bytes are sapling-crypto's mock proofs, so the chain would reject those transactions. The on-chain check is the live run in the README ("Proven on testnet").

### Ignored live-testnet tests (`cargo test --test testnet -- --ignored`)

Without their variables each one panics with `TESTNET TEST CANNOT RUN: <VAR> not configured`; none of them is counted as passing above.

| Test | Needs |
|---|---|
| `lightwalletd_reports_chain_height` | `ZCASH_NETWORK=testnet`, `ZCASH_LWD_URL`, `ZCASH_WALLET_DB`, `ZCASH_WALLET_CLI` |
| `lightwalletd_grpc_tip_height_for_delays` | `ZCASH_LWD_URL` (printed `testnet tip 4521111` against testnet.zec.rocks on 2026-10-20) |
| `sends_one_shielded_tx_to_all_csv_recipients` | the four chain vars above, `PAYROLL_TEST_CSV`, and a funded wallet (spends testnet ZEC) |
| `testnet_employee_a_key_cannot_see_employee_b_payout` | `PAYROLL_EMPLOYER_SEED`, `PAYROLL_PAYOUT_A`, `PAYROLL_PAYOUT_B` (`<txid>:<height>:<raw-tx-hex-file>`), `PAYROLL_EMPLOYEE_A_UA`, `PAYROLL_EMPLOYEE_B_UA` |

## Browser E2E (`e2e/`, Playwright 1.63.0 in Google Chrome)

```
CARGO_BUILD_JOBS=2 cargo build && cd e2e && npm install
npm run stack          # 3 real desk servers on 127.0.0.1:8791-8793, env -i (no .env, seed or key)
npm run test:e2e
npm run stack:stop     # kills those PIDs
```

| Spec | Tests | Desktop 1280×800 | Mobile 375×812 |
|---|---|---|---|
| `tests/landing.spec.ts` (payslip page) | 11 | 11 passed | 11 passed |
| `tests/desk.spec.ts` (desk at `/#desk`) | 13 | 13 passed | 13 passed |
| **Total** | **24** | **24** | **24** → **48 passed, 0 failed** |

Every E2E test also fails on any console error, page error, failed same-origin request or same-origin HTTP status ≥ 400, except the server's honest `400` answers that a test names explicitly. The desk tests include a DNS-rebinding check (`Host: attacker.example` gets `403` on `/api/audit`, `/api/state` and `/`) and the RFC 4180 parse of the accountant CSV.

## Other checks

- `cargo run -q -- plan examples/bad.csv` exits 1 with one error per bad row (README "Do X, see Y").
- `cargo run -q -- check-env` with no `.env` exits 1 with `ERROR: ZCASH_NETWORK not configured`.
