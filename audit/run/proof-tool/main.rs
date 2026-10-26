//! Proof-of-pay against real shielded transactions built in-process:
//! real keys from a fixed employer seed (treasury = account 0; each employee's
//! child wallet seed derived from it), real note encryption with each child
//! wallet's OVK, real trial decryption by the lender-side verifier.
//! Synthetic parts: each tx is funded by a transparent input spending a made-up outpoint
//! (so the chain would reject it), and Sapling uses sapling-crypto's mock zk-proof bytes.
//! Ironwood bundles carry real Halo2 proofs. The verifier does not check zk-proofs (it
//! trial-decrypts and checks the employer signature), so it reads what it would read in a
//! mined tx. The on-chain check is the 2026-10-20 live run (README "Proven on testnet");
//! see tests/testnet.rs and scripts/testnet-two-employees.sh.

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


fn main() {
 let seed=employer();let child=d::employee_ufvk(&seed,"emp-a").unwrap();
 let tx=build_tx(vec![Out::Sapling(Some(child.clone()),*personal_ua(0xA1).sapling().unwrap(),PAY_A,"payroll emp-a")]);
 let txid=d::verify_tx(&child,&tx,None,height()).unwrap().txid;
 let proof=d::export(&seed,&signer(),"emp-a","emp-a","Ada",&payout(&txid,PAY_A)).unwrap();
 let dir=std::env::temp_dir().join(format!("misthos-audit-offline-proof-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();
 std::fs::write(dir.join("proof.txt"),proof).unwrap();std::fs::write(dir.join("tx.hex"),hex::encode(&tx)).unwrap();
 let result=std::process::Command::new("env").args(["-i",&format!("PATH={}",std::env::var("PATH").unwrap()),&std::env::var("PAYROLL_BIN").unwrap(),"verify-proof","proof.txt","tx.hex",&txid,&height().to_string(),"--employer-pubkey",&signer().public_hex()]).current_dir(&dir).output().unwrap();
 std::fs::remove_dir_all(dir).unwrap();
 let out=String::from_utf8(result.stdout).unwrap();let err=String::from_utf8(result.stderr).unwrap();print!("{out}");eprint!("{err}");assert!(result.status.success());
 for s in ["EMPLOYER SIGNATURE OK","DECRYPTED OFFLINE (NOT checked on chain","PayoutOut Sapling 0.0125 ZEC recipient=","memo=\"payroll emp-a\"","PAID OUT 0.0125 ZEC"]{assert!(out.contains(s),"missing required output: {}",s)}
 assert!(!out.contains("VERIFIED ON CHAIN"));
 println!("AUDIT PROOF-VERIFY-OFFLINE PASS (synthetic fixture; not chain confirmation)");
}
