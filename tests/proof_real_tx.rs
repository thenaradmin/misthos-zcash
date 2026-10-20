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
use zec_payroll::disclosure::{self as d, Direction, DisclosureError, EmployerSeed, NETWORK};

fn employer() -> EmployerSeed {
    EmployerSeed::from_bytes(&[0x5a; 32])
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

    // A's exported proof file, as a lender would receive it.
    let (id, lender_key) = d::parse_export(&d::export(&s, "emp-a", "emp-a", "Ada").unwrap()).unwrap();
    assert_eq!(id, "emp-a");
    let p = d::verify_tx(&lender_key, &tx_a, None, height()).unwrap();
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
