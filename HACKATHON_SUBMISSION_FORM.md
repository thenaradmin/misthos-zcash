# Submission form: Misthos

> **Portfolio, not a Zecathon entry.** The team's one Zecathon entry is Shielded x402. Misthos (`zec-private-payroll`) is a portfolio piece (feature-frozen, except the browser payout flow Nick asked for on 2026-10-28); this file exists so the fields are ready if that ever changes.

| Field | Value |
|---|---|
| Project name | Misthos (private payroll processing) |
| Repository | https://github.com/nickthelegend/zec-private-payroll |
| License | MIT, Copyright (c) 2026 Nivesh Gajengi |
| Network | Zcash testnet only (mainnet refused in code) |
| Demo | Local: `PAYROLL_ROSTER_CSV=e2e/fixtures/roster.csv cargo run -- serve`, then http://127.0.0.1:8787 and the payroll desk at `/desk` (`--payouts` to run payroll from the browser). No hosted demo. |
| Live evidence | From the desk (2026-10-28): funding `82ca8b30…0b6ea3` (4550378), Alice `44686ece…744a2c` (4550386), Bob `3ff05411…40ac7b` (4550389). CLI run (2026-10-20): funding `e0a93ebc…2cca87` (4521170), Alice `f0e6ce45…0a46dc` (4521181), Bob `287318aa…6fa65a` (4521189). Links in the README |
| Tests | 103 Rust tests passing + 4 ignored live tests; 42 Playwright E2E in Chrome ([TESTING.md](TESTING.md)) |

## Description (under 2,000 characters)

Misthos pays a payroll from shielded Zcash testnet wallets so that the public sees only that transactions happened, while each employee can be given a key that reads their own payout and nothing else. The employer loads a CSV roster. `payroll pay --two-step` funds one child wallet per employee in one transaction, waits random block delays, then each child wallet pays its employee. The employer exports, for one employee, a proof file holding that child wallet's viewing key and an ed25519 employer signature over the payout's txid, amount, period and network; a lender runs `verify-proof --lwd`, which checks the signature against the published employer key, fetches the mined transaction from lightwalletd and decrypts it.

What is real: a funded two-employee run on testnet on 2026-10-20 (three Ironwood v6 transactions, linked in the README), checked with `verify-proof --lwd`, which also rejected the wrong txid, a changed amount, period or signature and a different key. The proof tests use real ZIP 32 keys, real note encryption, real Halo2 proofs for Ironwood and real ed25519 signatures.

What is synthetic: the UAs and the auditor UFVK in the E2E fixtures and unit tests are correctly encoded but owned by no wallet; in `proof_real_tx` the Sapling zk-proof bytes are mocks and the transparent inputs spend made-up outpoints, so those transactions could not be mined. The payslip page shows recorded data from the live run, not a live query.

Disclosed limits: timing, headcount (funding action count) and the fixed payout shape can link a run on a quiet chain; employee keys cannot be revoked; `employee_id` in a proof is not signed (the proof is a bearer document); the desk has no login and stays on loopback; transactions are built by zingo-cli.
