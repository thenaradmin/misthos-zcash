# Private Payroll Rail (ZECATHON)

## Goal & target track
ZECATHON Shielded Payments. Also NEAR `realworld` later only if rebuilt in Nov window.

## Fact check (do not overclaim)
One-time IVK idea is REAL (Zakura, Oct 18 2026, crate `zakura-one-time-ivk` 0.0.0 on docs.rs, still in review). Only use it if the crate actually builds; otherwise use fresh diversified addresses + per-payslip payment disclosure. Do NOT claim NEAR Intents payroll unless 1Click API works on testnet/mainnet for you (Intents is mainnet; ZEC route real but costs money).

## Honest feasibility
Medium-high for ZEC-only batch payroll. Intents leg adds real-money mainnet risk — make it optional phase 2.

## Scoped MVP (1 Claude CLI agent, ~1-2 days)
1. Employer uploads CSV (name, UA, amountZEC, memo).
2. Backend builds a ZIP-321 multi-payment URI / proposes one shielded tx with many outputs via zcash_client_backend; signs with employer wallet (testnet).
3. Each employee gets a payslip: txid + per-output payment disclosure / memo; auditor gets a read-only viewing key view.
4. UI: run payroll, history, auditor mode.

## Stack & install
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
sudo apt-get install -y protobuf-compiler build-essential pkg-config libssl-dev
# Zcash wallet libs (librustzcash): zcash_client_backend, zcash_client_sqlite, zcash_keys, zcash_primitives, zcash_protocol
cargo add zcash_client_backend zcash_client_sqlite zcash_keys zcash_primitives zcash_protocol --features zcash_client_backend/lightwalletd-tonic
# CLI wallet alternative: zingolib / zingo-cli
git clone https://github.com/zingolabs/zingolib && cd zingolib && cargo build --release -p zingo-cli
# testnet: public lightwalletd (verify current host in zcash docs), e.g. https://testnet.zec.rocks:443
# testnet faucet: https://testnet.zecfaucet.com (verify live)
```
Check versions on crates.io before pinning; librustzcash crates move fast (Ironwood/NU6.3 pool is live since Jul 28 2026 — Orchard is sealed, use the pool the current crates support).

## Architecture
CSV -> validator -> ZIP-321 proposal -> local wallet (zcash_client_sqlite) -> lightwalletd broadcast -> scan -> payslips.

## Milestones & acceptance tests
- M1 CSV->ZIP-321 URI + validation. AT: invalid UA rejected, unit tests.
- M2 real testnet batch tx with 3 recipients. AT: all 3 test wallets receive correct amounts.
- M3 auditor view with UFVK. AT: auditor sees payroll, no spend capability.
- M4 UI + video.

## Demo script (≤2 min)
Upload 3-person CSV -> one shielded tx -> 3 recipient wallets show funds -> explorer shows nothing -> auditor view shows the ledger.

## Hard rules
- NO mocked APIs, stubbed responses, or fake chain data. Zcash testnet + NEAR testnet only, real endpoints.
- Missing key/endpoint/wallet => fail loudly: exit non-zero in CLI, red banner in UI naming the missing var (e.g. `ZCASH_LWD_URL not configured`).
- Secrets only in `.env` (gitignored); ship `.env.example`.
- One project at a time; `cargo clean` / delete `node_modules` and `target/` of finished projects to free disk.
- Do not push, make repos public, deploy to mainnet, or sign mainnet txs without Nick's explicit OK.
- README written for AI judges: "do X -> see Y" steps.
