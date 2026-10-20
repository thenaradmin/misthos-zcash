# Fact-check: ZECATHON and NEAR HACKATHON, plus the claims behind the ideas

Checked Oct 20, 2026, ~04:35 IST. Sources are only the URLs below. Times are converted to IST (UTC+5:30).

## 1. Events

### ZECATHON (Zcash privacy hackathon, by zkSNARKs)
**Official page:** https://thezecathon.com. **Tracks, rules, schedule and prize details are behind a sign-in** ("Sign in to see prize information and to submit your project. Tracks, rules, the schedule and the submission form are all inside. One account per project; GitHub, Google or email."). I couldn't read the official rules. The details below come from the public page plus the organizer's own posts on X.

| Item | Finding | Source |
|---|---|---|
| Dates | Launched Sep 28, 2026; "Submissions are open for the next 30 days"; the public page says "30 DAYS TO BUILD" | https://nitter.jaydenha.uk/zksnarks_/status/2104606505177899314 , https://thezecathon.com |
| Submission deadline | **Oct 28, 2026.** The string "Submissions due Oct 28." appears in the official page's source. No time or timezone is shown publicly, so confirm it after signing in | https://thezecathon.com (page source) |
| Prize pool | $100,000: Grand Prize $20,000; five track prizes of $15,000 each; Community Choice $5,000 | https://nitter.jaydenha.uk/zksnarks_/status/2104606505177899314 |
| Tracks | 01 Shielded Payments · 02 Private Markets (private bids, auctions, marketplaces) · 03 Cross-Chain (swaps, wallet integrations, onboarding) · 04 Core & Tooling · 05 Wildcard | https://nitter.jaydenha.uk/zksnarks_/status/2104699877011923155 |
| Side bounty | Wildcard onboarding bounty, $5,000 in ZEC, deadline Oct 4 (passed) | https://nitter.cf/zksnarks_/status/2104718302606205130 |
| Eligibility | **Not readable** (behind sign-in). Public page: "One account per project" | https://thezecathon.com |
| Pre-existing code / build window | **Not found publicly** (the rules are behind sign-in) | — |

### NEAR HACKATHON
**Official pages:** https://nearhackathon.com and https://nearhackathon.com/HACKATHON.md (both read in full).

| Item | Finding |
|---|---|
| Dates | Starts **Nov 1, 2026 00:00 UTC** (05:30 IST); development Nov 1–14; judging Nov 16–18; results by Nov 18 23:59 UTC |
| Submission deadline | **Nov 14, 2026 23:59 UTC = Nov 15, 05:29 IST.** Submissions are accepted only from Nov 1 00:00 UTC to Nov 14 23:59 UTC. One submission per team, and it can't be edited |
| Prize pool | Paid in NEAR: Grand prize $55K ×1; 1st $10K ×13; 2nd $3K ×13; 3rd $2K ×13. That totals **$250K** (my sum of the listed rows) |
| Tracks (13) | Unstoppable DeFi · Investment Thesis · Privacy & Private AI · Autonomous Business · AI Swarms · AI Security & Defense · Agent Economy · Real-World Money · Games With Real Economies · NFTs That Do Something · Memecoins With Mechanics · Social Money · Open Innovation. A project can select several tracks |
| Eligibility | "Developers, designers and people with ideas for the NEAR ecosystem." Solo or team. No sign-up; you need a mainnet NEAR account for the payout. No age or country rules stated |
| Pre-existing code / build window | "Build your project during the hackathon" (submission step 01). **No explicit rule on pre-existing code** |
| Submission requirements | Public repo (it can be private while building, but must be public by the deadline), plus a public X post saying it was built at NEAR HACKATHON, with a video or screenshots |
| Judging | Jury (Helius and Electric Capital are named) plus "Claude Code & Codex joining as AI judges." No fixed criteria; the page points to past criteria: "technical implementation, design, impact, originality and sustainability" |

## 2. Claims

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 1 | Zakura published on Oct 18–19, 2026 about NEAR Intents refund-address quantum linkability | **Confirmed** (the article date is Oct 19; Zakura's own page shows no date) | Zakura post: https://zakura.com/engineering/quantum-unlinkability/ (the `refundTo` shielded address is posted on NEAR's public chain; a quantum attacker could link payments). Coverage dated Oct 19, 2026, 06:30 UTC: https://crypto.news/zcash-developer-proposes-quantum-privacy-fix-for-near-intents-swaps/ . It's a *proposed* wallet-level fix (one-time IVK); the crate is only reserved: https://docs.rs/crate/zakura-one-time-ivk/latest |
| 2 | ML-DSA is live on NEAR mainnet | **Confirmed** | 2.13 mainnet upgrade on Jul 20, 2026: https://www.prnewswire.com/news-releases/near-protocol-brings-quantum-safe-signing-to-mainnet-302829646.html ; ML-DSA-65 stabilized at protocol v85: https://github.com/near/nearcore/commit/ab0dc49772a4a152fe15a77d273c2b2b19260906 . Scope: transaction signatures, access keys and the `ml_dsa_verify` host function; validator and block signatures are unchanged: https://github.com/near/nearcore/blob/master/docs/architecture/how/post_quantum_signatures.md |
| 3 | NEP-655 and a Q-Day Recovery Registry previewed by Near One | **Confirmed** (as in development) | NEP-655 (commitment-based implicit accounts) is live on testnet, with mainnet "in the coming weeks"; Near One is "actively working on a Q-day recovery registry". Quoting NEAR's X post: https://www.kucoin.com/news/insight/NEAR/6ac9357838a26400079361c4 ; https://www.tokenpost.com/news/technology/29177 . The registry is *in development*, not launched |
| 4 | A $42K Zcash community grant for Shielded x402 | **Contradicted.** The $42K was *requested* and **rejected** on Sep 28, 2026 | Application ($42,000 ask): https://forum.zcashcommunity.com/t/grant-application-shielded-x402-private-machine-payments-for-zcash/57826 ; decision "Rejected", Sep 28, 2026: https://openzcash.org/zcg/decisions |
| 5 | An existing live zkSNARKs + NEAR Intents ZEC auction integration | **Confirmed** | NEAR Intents settled the zkSNARKs auction in ZEC; ~$19M through 1,718 cross-chain swaps: https://intents.aurora.dev/blog/zcash-auction-cross-chain-swaps ; https://nightash.org/en/news/near-intents-zec-auction-settlement-2026-09-14/ ; auction cleared at 1.5 ZEC with 16,971 bids: https://cryptobriefing.com/zksnarks-auction-clears-zcash-blind-bids/ (search summaries; the Aurora page didn't render via curl) |
| 6 | Ironwood has 2,700+ Lean theorems | **Confirmed** | "Ironwood's balance integrity proofs contain over 2,700 theorems" (Jul 28, 2026): https://tachyon.z.cash/blog/ironwood-verification-complete/ ; https://zcash.github.io/ironwood/formal-verification.html . Note: it covers balance integrity, not privacy |
| 7 | Zcash post-quantum signature plans for Jan 2027 | **Confirmed** (a *target*, not an activation date) | CoinDesk, Oct 9, 2026: https://www.coindesk.com/tech/2026/10/09/zcash-developers-set-january-target-for-quantum-resistant-payments-after-bunker-mode-scare ; https://thepaypers.com/payments/news/zcash-developers-target-january-2027-for-quantum-resistant-payments . Scope: hash-based PQ signatures for **transparent** payments first; shielded payments need separate work |
| 8 | NEAR co-hosted a Quantum and Privacy Day with Quantus | **Confirmed** | Official page: "Quantum & Privacy Day — 8 October 2026, Tower Club Singapore… Brought to you by Quantus and NEAR": https://q.day/ ; https://luma.com/jtof4o9b |

## Implications for our ideas
- Don't pitch "funded by a $42K ZCG grant". It was rejected. That does mean the Shielded x402 *gap* is still open, though.
- Pitch Zakura's one-time IVK as a *proposal* with a reserved crate, not a shipped library.
- Pitch the NEAR Q-Day registry as in development and NEP-655 as testnet-only.
- ZECATHON's rules (including pre-existing code and eligibility) need a sign-in to read. Nick must sign in himself to confirm them and the exact Oct 28 deadline time.
