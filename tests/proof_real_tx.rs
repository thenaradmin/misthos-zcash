//! Proof-of-pay against real shielded transactions built in-process:
//! real keys from a fixed employer seed (treasury = account 0; each employee's
//! child wallet seed derived from it), real note encryption with each child
//! wallet's OVK, real trial decryption by the lender-side verifier.
//! Each tx is funded by a transparent input spending a made-up outpoint (so the
//! chain would reject it) and Sapling uses sapling-crypto's mock zk-proof bytes;
//! Ironwood bundles carry real Halo2 proofs. Every byte the verifier reads is genuine.
//! No funded testnet payout exists yet: see tests/testnet.rs and scripts/testnet-two-employees.sh.

use std::str::FromStr;

use secp256k1::SecretKey;
use zcash_keys::keys::{UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::transaction::{
    builder::{BuildConfig, Builder, BundlePadding},
    fees::zip317,
};
use zcash_protocol::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
    ShieldedPool,
};
use zcash_transparent::{
    address::TransparentAddress,
    builder::TransparentSigningSet,
    bundle::{OutPoint, TxOut},
};
use zec_payroll::attest::EmployerKey;
use zec_payroll::disclosure::{self as d, Direction, DisclosureError, EmployerSeed, NETWORK};

fn employer() -> EmployerSeed {
    EmployerSeed::from_bytes(&[0x5a; 32])
}

/// The employer's ed25519 attestation key (fixed bytes for tests).
fn signer() -> EmployerKey {
    EmployerKey::from_bytes(&[0x5c; 32])
}

/// What a verifier trusts, e.g. from docs/employer-pubkey.txt.
fn employer_pubkey() -> ed25519_dalek::VerifyingKey {
    zec_payroll::attest::parse_pubkey(&signer().public_hex()).unwrap()
}

fn payout(txid: &str, amount_zat: u64) -> d::Payout<'_> {
    d::Payout { txid, amount_zat, pay_period: "adhoc:2026-10" }
}

/// An employee's own wallet (not derived from the employer seed).
fn personal_ua(byte: u8) -> zcash_keys::address::UnifiedAddress {
    use zcash_keys::keys::ReceiverRequirement::{Omit, Require};
    let k = UnifiedSpendingKey::from_seed(&NETWORK, &[byte; 32], zip32::AccountId::ZERO).unwrap().to_unified_full_viewing_key();
    k.default_address(UnifiedAddressRequest::unsafe_custom(Require, Require, Omit)).unwrap().0
}

fn height() -> u32 {
    u32::from(NETWORK.activation_height(NetworkUpgrade::Nu7).unwrap()) + 100
}

fn memo(t: &str) -> MemoBytes {
    MemoBytes::from(Memo::from_str(t).unwrap())
}

enum Out {
    /// (sender child wallet UFVK for the OVK, recipient, value, memo)
    Sapling(Option<UnifiedFullViewingKey>, sapling::PaymentAddress, u64, &'static str),
    Ironwood(Option<UnifiedFullViewingKey>, orchard::Address, u64, &'static str),
}

fn build_tx(outs: Vec<Out>) -> Vec<u8> {
    let sk = SecretKey::from_secret_bytes([0x42; 32]).unwrap();
    let pk = sk.public_key();
    let make = |input: u64| {
        let mut b = Builder::new(
            NETWORK,
            BlockHeight::from_u32(height()),
            BuildConfig::Standard {
                sapling_anchor: Some(sapling::Anchor::empty_tree()),
                orchard_anchor: None,
                ironwood_anchor: Some(orchard::Anchor::empty_tree()),
                orchard_padding: BundlePadding::DEFAULT,
                ironwood_padding: BundlePadding::DEFAULT,
            },
        );
        let coin = TxOut::new(Zatoshis::from_u64(input).unwrap(), TransparentAddress::from_pubkey(&pk).script().into());
        b.add_transparent_p2pkh_input(pk, OutPoint::new([7u8; 32], 0), coin).unwrap();
        for o in &outs {
            match o {
                Out::Sapling(from, to, v, m) => {
                    let ovk = from.as_ref().map(|k| k.sapling().unwrap().to_ovk(zip32::Scope::External));
                    b.add_sapling_output::<zip317::FeeError>(ovk, *to, Zatoshis::from_u64(*v).unwrap(), memo(m)).unwrap()
                }
                Out::Ironwood(from, to, v, m) => {
                    let ovk = from.as_ref().map(|k| k.orchard().unwrap().to_ovk(zip32::Scope::External));
                    b.add_ironwood_output::<zip317::FeeError>(ovk, *to, Zatoshis::from_u64(*v).unwrap(), memo(m)).unwrap()
                }
            }
        }
        b
    };
    let rule = zip317::FeeRule::standard();
    let paid: u64 = outs.iter().map(|o| match o {
        Out::Sapling(_, _, v, _) | Out::Ironwood(_, _, v, _) => *v,
    }).sum();
    let fee = make(paid).get_fee(&rule).unwrap().into_u64();
    let mut signing = TransparentSigningSet::new();
    signing.add_key(sk);
    let built = make(paid + fee)
        .build(
            &signing,
            &[],
            &[],
            rand10::rand_core::UnwrapErr(rand10::rngs::SysRng),
            &sapling::prover::mock::MockSpendProver,
            &sapling::prover::mock::MockOutputProver,
            &rule,
        )
        .expect("build tx");
    let mut bytes = Vec::new();
    built.transaction().write(&mut bytes).unwrap();
    bytes
}

const PAY_A: u64 = 1_250_000;
const PAY_B: u64 = 3_000_000;

#[test]
fn employee_a_proves_own_payout_and_b_key_sees_nothing() {
    let s = employer();
    let (ka, kb) = (d::employee_ufvk(&s, "emp-a").unwrap(), d::employee_ufvk(&s, "emp-b").unwrap());
    let (ua_a, ua_b) = (personal_ua(0xA1), personal_ua(0xB2));

    // Step 2 payouts, one tx per employee, each encrypted with its own child wallet's OVK.
    let tx_a = build_tx(vec![Out::Sapling(Some(ka.clone()), *ua_a.sapling().unwrap(), PAY_A, "payroll 2026-10 emp-a")]);
    let tx_b = build_tx(vec![Out::Sapling(Some(kb.clone()), *ua_b.sapling().unwrap(), PAY_B, "payroll 2026-10 emp-b")]);

    // A's employer-signed proof file, as a lender would receive it.
    let txid_a = d::verify_tx(&ka, &tx_a, None, height()).unwrap().txid;
    let text = d::export(&s, &signer(), "emp-a", "emp-a", "Ada", &payout(&txid_a, PAY_A)).unwrap();
    let signed = d::parse_export(&text).unwrap();
    assert_eq!(signed.employee_id, "emp-a");
    d::verify_signature(&signed, &employer_pubkey()).unwrap();
    let lender_key = signed.ufvk.clone();
    let p = d::verify_tx(&lender_key, &tx_a, None, height()).unwrap();
    d::check_payout(&signed.attestation, &p).unwrap();
    assert_eq!(p.paid_out_zat(), PAY_A);
    assert_eq!(p.outputs.len(), 1);
    assert_eq!(p.outputs[0].direction, Direction::PayoutOut);
    assert_eq!(p.outputs[0].memo, "payroll 2026-10 emp-a");
    assert!(d::receiver_hexes(&ua_a.encode(&NETWORK)).contains(&p.outputs[0].recipient_hex));
    // txid binding: correct txid accepted, wrong one refused.
    d::verify_tx(&lender_key, &tx_a, Some(&p.txid), height()).unwrap();
    assert!(matches!(d::verify_tx(&lender_key, &tx_a, Some(&"00".repeat(32)), height()), Err(DisclosureError::TxidMismatch { .. })));

    // A's key cannot decrypt B's payout; B's key cannot decrypt A's.
    assert!(matches!(d::verify_tx(&lender_key, &tx_b, None, height()), Err(DisclosureError::NothingVisible)));
    assert!(matches!(d::verify_tx(&kb, &tx_a, None, height()), Err(DisclosureError::NothingVisible)));
    assert_eq!(d::verify_tx(&kb, &tx_b, None, height()).unwrap().paid_out_zat(), PAY_B);
}

/// The employer signature binds the proof to one real payout: amount, txid, child wallet, period, network.
#[test]
fn signed_proof_is_bound_to_the_real_payout() {
    let s = employer();
    let (ka, kb) = (d::employee_ufvk(&s, "emp-a").unwrap(), d::employee_ufvk(&s, "emp-b").unwrap());
    let ua_a = personal_ua(0xA1);
    let tx_a = build_tx(vec![Out::Sapling(Some(ka.clone()), *ua_a.sapling().unwrap(), PAY_A, "payroll emp-a")]);
    let tx_b = build_tx(vec![Out::Sapling(Some(kb.clone()), *personal_ua(0xB2).sapling().unwrap(), PAY_B, "payroll emp-b")]);
    let txid_a = d::verify_tx(&ka, &tx_a, None, height()).unwrap().txid;
    let seen_a = d::verify_tx(&ka, &tx_a, None, height()).unwrap();
    let full_check = |text: &str, raw: &[u8]| -> Result<(), DisclosureError> {
        let p = d::parse_export(text)?;
        d::verify_signature(&p, &employer_pubkey())?;
        let seen = d::verify_tx(&p.ufvk, raw, Some(&p.attestation.txid), height())?;
        d::check_payout(&p.attestation, &seen)
    };
    let good = d::export(&s, &signer(), "emp-a", "emp-a", "Ada", &payout(&txid_a, PAY_A)).unwrap();
    full_check(&good, &tx_a).unwrap();

    // Employer signed the wrong amount (or the roster changed): the chain disagrees.
    let wrong_amount = d::export(&s, &signer(), "emp-a", "emp-a", "Ada", &payout(&txid_a, PAY_A + 1)).unwrap();
    let signed = d::parse_export(&wrong_amount).unwrap();
    assert!(matches!(d::check_payout(&signed.attestation, &seen_a), Err(DisclosureError::AmountMismatch { .. })));
    // Employee edits the amount in the file: signature fails before any tx is read.
    assert!(matches!(full_check(&good.replace(&format!("amount_zat={PAY_A}"), "amount_zat=99000000"), &tx_a), Err(DisclosureError::Attest(_))));
    // Changed period / network swap.
    assert!(matches!(full_check(&good.replace("pay_period=adhoc:2026-10", "pay_period=adhoc:2026-11"), &tx_a), Err(DisclosureError::Attest(_))));
    assert!(matches!(full_check(&good.replace("network=testnet", "network=mainnet"), &tx_a), Err(DisclosureError::WrongNetwork(_))));
    // A's valid proof presented with B's tx: txid mismatch (the signed txid is A's).
    assert!(matches!(full_check(&good, &tx_b), Err(DisclosureError::TxidMismatch { .. })));

    // Forged: an employee pays themselves from their own wallet and signs with their own key.
    let forger_seed = EmployerSeed::from_bytes(&[0xEE; 32]);
    let fk = d::employee_ufvk(&forger_seed, "emp-a").unwrap();
    let forged_tx = build_tx(vec![Out::Sapling(Some(fk.clone()), *ua_a.sapling().unwrap(), 99_000_000, "payroll emp-a")]);
    let ftxid = d::verify_tx(&fk, &forged_tx, None, height()).unwrap().txid;
    let forger_key = EmployerKey::from_bytes(&[0xEE; 32]);
    let forged = d::export(&forger_seed, &forger_key, "emp-a", "emp-a", "Ada", &payout(&ftxid, 99_000_000)).unwrap();
    assert!(matches!(full_check(&forged, &forged_tx), Err(DisclosureError::Attest(ref m)) if m.contains("not the expected employer key")));
    // Same forgery, but the file claims the employer's public key: signature invalid.
    let claimed = forged.replace(&forger_key.public_hex(), &signer().public_hex());
    assert!(matches!(full_check(&claimed, &forged_tx), Err(DisclosureError::Attest(ref m)) if m.contains("does not match")));
}

#[test]
fn shared_tx_reveals_only_the_holders_outputs() {
    let s = employer();
    let (ka, kb) = (d::employee_ufvk(&s, "emp-a").unwrap(), d::employee_ufvk(&s, "emp-b").unwrap());
    let (fa, fb) = (d::account_address(&ka).unwrap(), d::account_address(&kb).unwrap());
    let fee = d::PAYOUT_FEE_ZAT;

    // Step 1 funding: one treasury tx funds both child wallets (pay + payout fee).
    let funding = build_tx(vec![
        Out::Sapling(None, *fa.sapling().unwrap(), PAY_A + fee, "funding emp-a"),
        Out::Sapling(None, *fb.sapling().unwrap(), PAY_B + fee, "funding emp-b"),
    ]);
    let p = d::verify_tx(&ka, &funding, None, height()).unwrap();
    assert_eq!(p.outputs.len(), 1, "A must not see B's funding output");
    assert_eq!((p.outputs[0].direction.clone(), p.outputs[0].amount_zat), (Direction::FundingIn, PAY_A + fee));
    assert_eq!(p.paid_out_zat(), 0);

    // Even if two child wallets' outputs share one tx, each key sees only its own.
    let (ua_a, ua_b) = (personal_ua(0xA1), personal_ua(0xB2));
    let both = build_tx(vec![
        Out::Sapling(Some(ka.clone()), *ua_a.sapling().unwrap(), PAY_A, "a"),
        Out::Sapling(Some(kb.clone()), *ua_b.sapling().unwrap(), PAY_B, "b"),
    ]);
    let pa = d::verify_tx(&ka, &both, None, height()).unwrap();
    assert_eq!((pa.outputs.len(), pa.paid_out_zat()), (1, PAY_A));
    let pb = d::verify_tx(&kb, &both, None, height()).unwrap();
    assert_eq!((pb.outputs.len(), pb.paid_out_zat()), (1, PAY_B));
}

#[test]
fn ironwood_payout_verifies_with_real_halo2_bundle() {
    let s = employer();
    let ka = d::employee_ufvk(&s, "emp-a").unwrap();
    let kb = d::employee_ufvk(&s, "emp-b").unwrap();
    let ua_a = personal_ua(0xA1);
    let tx = build_tx(vec![Out::Ironwood(Some(ka.clone()), *ua_a.orchard().unwrap(), PAY_A, "payroll iw")]);
    let p = d::verify_tx(&ka, &tx, None, height()).unwrap();
    assert_eq!(p.paid_out_zat(), PAY_A);
    assert_eq!(p.outputs[0].pool, ShieldedPool::Ironwood);
    assert!(d::receiver_hexes(&ua_a.encode(&NETWORK)).contains(&p.outputs[0].recipient_hex));
    assert!(matches!(d::verify_tx(&kb, &tx, None, height()), Err(DisclosureError::NothingVisible)));
}

/// Verifier attacks (audit 7). Documents what verify-proof does and does NOT check.
#[test]
fn verify_proof_attacks() {
    let s = employer();
    let ka = d::employee_ufvk(&s, "emp-a").unwrap();
    let ua_a = personal_ua(0xA1);
    let tx_a = build_tx(vec![Out::Sapling(Some(ka.clone()), *ua_a.sapling().unwrap(), PAY_A, "payroll emp-a")]);
    let txid = d::verify_tx(&ka, &tx_a, None, height()).unwrap().txid;

    // Tampered tx hex. The v6 txid (ZIP 244) commits to effects, not to proofs/signatures, so
    // a flip in authorizing data can keep the txid; it must then decrypt to the SAME outputs.
    // Every other flip must be a parse error, txid mismatch or nothing-visible.
    let want = d::verify_tx(&ka, &tx_a, None, height()).unwrap().outputs;
    for i in 0..tx_a.len() {
        let mut t = tx_a.clone();
        t[i] ^= 1;
        if let Ok(p) = d::verify_tx(&ka, &t, Some(&txid), height()) {
            assert_eq!(p.outputs, want, "tamper at {i} changed what the lender sees");
        }
    }
    // Truncated tx.
    assert!(d::verify_tx(&ka, &tx_a[..tx_a.len() - 1], Some(&txid), height()).is_err());

    // A pre-NU7 height is rejected (wrong branch / decryption rules for that height).
    assert!(d::verify_tx(&ka, &tx_a, Some(&txid), 1).is_err());
    // A different post-NU7 height is ACCEPTED: height only picks the consensus branch and is
    // never checked against a chain. (Trust-only; README says so.)
    assert!(d::verify_tx(&ka, &tx_a, Some(&txid), height() + 50_000).is_ok());

    // Malformed / unsigned proof files.
    for bad in ["", "zec-payroll-proof v1\nemployee_id=x\n", "zec-payroll-proof v2\nemployee_id=x\nufvk=uviewtest1garbage\n", "nope\nufvk=x"] {
        assert!(matches!(d::parse_export(bad), Err(DisclosureError::BadExport(_))), "{bad:?}");
    }
    // Trial decryption alone still accepts any key over a self-made tx (these Sapling proofs are
    // dummy bytes the chain would reject); that is why verify-proof checks the employer signature
    // first (see signed_proof_is_bound_to_the_real_payout) and the chain with --lwd.
    let forger = UnifiedSpendingKey::from_seed(&NETWORK, &[0xEE; 32], zip32::AccountId::ZERO).unwrap().to_unified_full_viewing_key();
    let forged_tx = build_tx(vec![Out::Sapling(Some(forger.clone()), *ua_a.sapling().unwrap(), 99_000_000, "payroll emp-a")]);
    assert_eq!(d::verify_tx(&forger, &forged_tx, None, height()).unwrap().paid_out_zat(), 99_000_000);

    // Treasury key never comes out of export, and child keys differ from it and each other.
    let t = d::encode_ufvk(&d::treasury_ufvk(&s).unwrap()).unwrap();
    let e = d::export(&s, &signer(), "emp-a", "emp-a", "A", &payout(&txid, PAY_A)).unwrap();
    assert!(!e.contains(&t));
    assert_ne!(d::encode_ufvk(&ka).unwrap(), d::encode_ufvk(&d::employee_ufvk(&s, "emp-b").unwrap()).unwrap());
    // Duplicate address (only possible "collision" with 256-bit derivation) is refused.
    assert!(matches!(d::check_accounts(["emp-a", " emp-a "]), Err(DisclosureError::AccountCollision(..))));
    // Child seed depends on the employer seed: another employer, same address -> different key.
    assert_ne!(d::encode_ufvk(&ka).unwrap(), d::encode_ufvk(&d::employee_ufvk(&EmployerSeed::from_bytes(&[0x5b; 32]), "emp-a").unwrap()).unwrap());
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("zpp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs `payroll verify-proof` in OFFLINE mode (no network) from `cwd` with a clean env.
fn verify_cli(cwd: &std::path::Path, proof: &str, raw: &[u8], txid: &str, extra: &[&str]) -> std::process::Output {
    std::fs::write(cwd.join("proof.txt"), proof).unwrap();
    std::fs::write(cwd.join("tx.hex"), hex::encode(raw)).unwrap();
    let h = height().to_string();
    let mut args = vec!["verify-proof", "proof.txt", "tx.hex", txid, h.as_str()];
    args.extend_from_slice(extra);
    std::process::Command::new(env!("CARGO_BIN_EXE_payroll"))
        .args(&args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .output()
        .unwrap()
}

/// employee_id is not signed: editing it keeps the proof valid, so verify-proof must not
/// print it on its verified line, only labelled as unsigned.
#[test]
fn edited_employee_id_still_verifies_and_is_labelled_unsigned() {
    let s = employer();
    let ka = d::employee_ufvk(&s, "emp-a").unwrap();
    let tx = build_tx(vec![Out::Sapling(Some(ka.clone()), *personal_ua(0xA1).sapling().unwrap(), PAY_A, "payroll emp-a")]);
    let txid = d::verify_tx(&ka, &tx, None, height()).unwrap().txid;
    let good = d::export(&s, &signer(), "emp-a", "emp-a", "Ada", &payout(&txid, PAY_A)).unwrap();
    let edited = good.replace("employee_id=emp-a", "employee_id=CEO");
    assert_ne!(good, edited);

    // Library level: the signature still verifies with the edited id.
    let p = d::parse_export(&edited).unwrap();
    d::verify_signature(&p, &employer_pubkey()).unwrap();
    assert_eq!(p.employee_id, "CEO");
    assert_eq!(p.unsigned_employee_line(), "employee_id (NOT signed; as written in the file): CEO");

    // CLI level: exit 0, and the id appears only on the unsigned line.
    let dir = tmp_dir("unsigned-id");
    let o = verify_cli(&dir, &edited, &tx, &txid, &["--employer-pubkey", &signer().public_hex()]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{out}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("employee_id (NOT signed; as written in the file): CEO"), "{out}");
    for l in out.lines().filter(|l| l.starts_with("VERIFIED") || l.starts_with("DECRYPTED") || l.starts_with("EMPLOYER SIGNATURE OK")) {
        assert!(!l.contains("CEO") && !l.contains("employee="), "unsigned id on a verified line: {l}");
    }
    assert!(out.contains("bearer document"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Without --employer-pubkey, verify-proof trusts docs/employer-pubkey.txt in the CURRENT
/// directory. Whoever controls that folder controls the root of trust, so it must say so.
#[test]
fn pubkey_file_from_current_dir_is_warned_with_its_absolute_path() {
    let forger_seed = EmployerSeed::from_bytes(&[0xEE; 32]);
    let forger_key = EmployerKey::from_bytes(&[0xEE; 32]);
    let fk = d::employee_ufvk(&forger_seed, "emp-a").unwrap();
    let tx = build_tx(vec![Out::Sapling(Some(fk.clone()), *personal_ua(0xA1).sapling().unwrap(), 99_000_000, "payroll emp-a")]);
    let txid = d::verify_tx(&fk, &tx, None, height()).unwrap().txid;
    let forged = d::export(&forger_seed, &forger_key, "emp-a", "emp-a", "Ada", &payout(&txid, 99_000_000)).unwrap();

    let dir = tmp_dir("cwd-pubkey");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/employer-pubkey.txt"), format!("# planted\n{}\n", forger_key.public_hex())).unwrap();
    let o = verify_cli(&dir, &forged, &tx, &txid, &[]);
    let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    // The forgery passes against the planted key: the only defence is the warning.
    assert!(o.status.success(), "{out}\n{err}");
    let abs = std::fs::canonicalize(dir.join("docs/employer-pubkey.txt")).unwrap().display().to_string();
    assert!(err.contains(&format!("WARNING: employer key taken from {abs} in the current directory")), "{err}");
    assert!(out.contains(&format!("expected key from {abs}")), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}
