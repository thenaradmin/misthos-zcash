//! Per-employee child wallets and the proof-of-pay export.
//!
//! Every employee gets their own child wallet: a seed derived from the employer
//! seed and their payout address (see [`child_wallet`]), used at account 0 in its
//! own zingo-cli data dir. The employer treasury key is never exported. A payout
//! is two transactions:
//!
//! 1. **Funding**: treasury -> the employee's child wallet (account 0) UA,
//!    `pay + PAYOUT_FEE`.
//! 2. **Payout**: child wallet -> the employee's own address, `pay`, as an ordinary
//!    zingo-cli `quicksend` from the child wallet's own data dir.
//!    Its outputs are encrypted with that account's outgoing viewing key.
//!
//! The export is that account's **full** viewing key (UFVK). It shows the
//! incoming funding notes and, through the OVK, every payout sent from that
//! account. It shows ALL of that one employee's payouts (not just one), and
//! nothing from the treasury or any other employee's account.
//!
//! The seed is never printed, logged, or put in an error message.

use std::collections::HashMap;

use zcash_client_backend::{decrypt_transaction, TransferType};
use zcash_keys::keys::{ReceiverRequirement, UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::transaction::{Transaction};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, Network},
    memo::Memo,
    ShieldedPool,
};

pub const NETWORK: Network = Network::TestNetwork;

/// ZIP 317 fee of the payout tx: exactly one funding note in, one output
/// (no change, because funding is exact). Sapling: max(1 spend, 1 output) and
/// Orchard: 1 action are both padded to the 2-action grace minimum,
/// so 2 * 5_000 zat.
pub const PAYOUT_FEE_ZAT: u64 = 10_000;


#[derive(Debug, PartialEq, Eq)]
pub enum DisclosureError {
    NoSeed,
    BadSeed,
    Treasury,
    AccountCollision(String, String),
    Key(String),
    BadExport(String),
    Unparseable(String),
    TxidMismatch { want: String, got: String },
    NothingVisible,
}

impl std::fmt::Display for DisclosureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use DisclosureError::*;
        match self {
            NoSeed => write!(f, "PAYROLL_EMPLOYER_SEED not configured (24-word BIP 39 mnemonic of the employer wallet)"),
            BadSeed => write!(f, "PAYROLL_EMPLOYER_SEED is not a valid BIP 39 mnemonic (value not shown)"),
            Treasury => write!(f, "refusing to export the employer treasury viewing key: it would reveal all of payroll"),
            AccountCollision(a, _) => write!(f, "payout address `{a}` appears twice in the roster; each employee needs their own"),
            Key(e) => write!(f, "key derivation failed: {e}"),
            BadExport(e) => write!(f, "not a zec-payroll proof file: {e}"),
            Unparseable(e) => write!(f, "transaction did not parse: {e}"),
            TxidMismatch { want, got } => write!(f, "the raw tx is {got}, not the claimed {want}"),
            NothingVisible => write!(f, "this viewing key sees nothing in this transaction: it is not a payout from or funding of this employee's account"),
        }
    }
}

impl std::error::Error for DisclosureError {}

/// Employer seed bytes. `Debug` is redacted so it cannot leak through `{:?}`.
pub struct EmployerSeed(Vec<u8>);

impl std::fmt::Debug for EmployerSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmployerSeed(<redacted>)")
    }
}

impl EmployerSeed {
    pub fn from_mnemonic(phrase: &str) -> Result<Self, DisclosureError> {
        let m = bip0039::Mnemonic::<bip0039::English>::from_phrase(phrase.trim()).map_err(|_| DisclosureError::BadSeed)?;
        Ok(EmployerSeed(m.to_seed("").to_vec()))
    }
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, DisclosureError> {
        let v = get("PAYROLL_EMPLOYER_SEED").filter(|v| !v.trim().is_empty()).ok_or(DisclosureError::NoSeed)?;
        Self::from_mnemonic(&v)
    }
    /// Raw seed bytes (tests use fixed bytes; production uses the mnemonic).
    pub fn from_bytes(b: &[u8]) -> Self {
        EmployerSeed(b.to_vec())
    }
}

/// The employee's child wallet: 32 bytes of entropy from a keyed, domain-separated
/// BLAKE2b-256 (key = employer seed, personalization `zecpayroll_child`, input =
/// payout address), encoded as a 24-word BIP 39 mnemonic so zingo-cli can restore it
/// (`ZINGO_SEED`). Keys come from that mnemonic's BIP 39 seed at ZIP 32 account 0,
/// exactly as zingo-cli derives them. A new payout address means a new child wallet.
pub struct ChildWallet {
    mnemonic: bip0039::Mnemonic<bip0039::English>,
}

impl std::fmt::Debug for ChildWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChildWallet(<redacted>)")
    }
}

impl ChildWallet {
    /// The mnemonic, for handing to zingo-cli through the `ZINGO_SEED` env var only.
    pub fn phrase_for_wallet_env(&self) -> &str {
        self.mnemonic.phrase()
    }
    pub fn ufvk(&self) -> Result<UnifiedFullViewingKey, DisclosureError> {
        let usk = UnifiedSpendingKey::from_seed(&NETWORK, &self.mnemonic.to_seed(""), zip32::AccountId::ZERO)
            .map_err(|e| DisclosureError::Key(format!("{e:?}")))?;
        Ok(usk.to_unified_full_viewing_key())
    }
}

pub fn child_wallet(seed: &EmployerSeed, payout_address: &str) -> Result<ChildWallet, DisclosureError> {
    if seed.0.len() < 32 || seed.0.len() > 64 {
        return Err(DisclosureError::BadSeed);
    }
    let h = blake2b_simd::Params::new()
        .hash_length(32)
        .key(&seed.0)
        .personal(b"zecpayroll_child")
        .hash(payout_address.trim().as_bytes());
    let mnemonic = bip0039::Mnemonic::<bip0039::English>::from_entropy(h.as_bytes().to_vec())
        .map_err(|e| DisclosureError::Key(e.to_string()))?;
    Ok(ChildWallet { mnemonic })
}

/// Public, non-secret directory name for an employee's child wallet.
pub fn child_dir_name(payout_address: &str) -> String {
    let h = blake2b_simd::Params::new().hash_length(8).personal(b"zecpayroll_cdir_").hash(payout_address.trim().as_bytes());
    format!("child-{}", h.to_hex())
}

/// Duplicate payout addresses would share one child wallet; refuse them.
pub fn check_accounts<'a>(addresses: impl IntoIterator<Item = &'a str>) -> Result<(), DisclosureError> {
    let mut seen = std::collections::HashSet::new();
    for a in addresses {
        if !seen.insert(a.trim()) {
            return Err(DisclosureError::AccountCollision(a.into(), a.into()));
        }
    }
    Ok(())
}

/// UFVK of an employee's child wallet (account 0 of the child seed).
pub fn employee_ufvk(seed: &EmployerSeed, payout_address: &str) -> Result<UnifiedFullViewingKey, DisclosureError> {
    child_wallet(seed, payout_address)?.ufvk()
}

/// Treasury UFVK (employer seed, account 0); used only to refuse exporting it.
pub fn treasury_ufvk(seed: &EmployerSeed) -> Result<UnifiedFullViewingKey, DisclosureError> {
    let usk = UnifiedSpendingKey::from_seed(&NETWORK, &seed.0, zip32::AccountId::ZERO)
        .map_err(|e| DisclosureError::Key(format!("{e:?}")))?;
    Ok(usk.to_unified_full_viewing_key())
}

/// Default (diversifier 0+) UA with Orchard + Sapling receivers: the funding destination.
pub fn account_address(ufvk: &UnifiedFullViewingKey) -> Result<zcash_keys::address::UnifiedAddress, DisclosureError> {
    use ReceiverRequirement::{Omit, Require};
    let req = UnifiedAddressRequest::unsafe_custom(Require, Require, Omit);
    ufvk.default_address(req).map(|(ua, _)| ua).map_err(|e| DisclosureError::Key(format!("{e:?}")))
}

pub fn funding_address(seed: &EmployerSeed, employee_id: &str) -> Result<String, DisclosureError> {
    Ok(account_address(&employee_ufvk(seed, employee_id)?)?.encode(&NETWORK))
}

pub fn encode_ufvk(k: &UnifiedFullViewingKey) -> Result<String, DisclosureError> {
    k.encode(&NETWORK).map_err(|e| DisclosureError::Key(format!("{e:?}")))
}

/// The proof file handed to the employee. Contains the account UFVK, never a spending key.
/// `key` is the employee's payout address (see [`child_wallet`]).
pub fn export(seed: &EmployerSeed, key: &str, employee_id: &str, name: &str) -> Result<String, DisclosureError> {
    let child = encode_ufvk(&employee_ufvk(seed, key)?)?;
    if child == encode_ufvk(&treasury_ufvk(seed)?)? {
        return Err(DisclosureError::Treasury);
    }
    let ufvk = child;
    let wallet = child_dir_name(key);
    Ok(format!(
        "zec-payroll-proof v1\nnetwork=testnet\nemployee_id={employee_id}\nname={name}\nwallet={wallet}\nufvk={ufvk}\n\
         # This UFVK shows every payout this employer sent to this employee from their payroll\n\
         # account, and the funding into it. It shows nothing from other employees or the treasury.\n\
         # Verify with: payroll verify-proof <this file> <raw-tx-hex-file> <txid> <block-height>\n"
    ))
}

pub fn parse_export(text: &str) -> Result<(String, UnifiedFullViewingKey), DisclosureError> {
    if !text.starts_with("zec-payroll-proof v1") {
        return Err(DisclosureError::BadExport("missing `zec-payroll-proof v1` header".into()));
    }
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(str::trim)).map(str::to_string);
    let id = field("employee_id=").ok_or_else(|| DisclosureError::BadExport("no employee_id".into()))?;
    let key = field("ufvk=").ok_or_else(|| DisclosureError::BadExport("no ufvk".into()))?;
    let ufvk = UnifiedFullViewingKey::decode(&NETWORK, &key).map_err(|e| DisclosureError::BadExport(e.to_string()))?;
    Ok((id, ufvk))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    /// Funding note received by the child wallet.
    FundingIn,
    /// Payout sent from the child wallet to the employee (recovered with the OVK).
    PayoutOut,
    /// Change back to the child wallet.
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub direction: Direction,
    pub pool: ShieldedPool,
    pub amount_zat: u64,
    /// Raw receiver bytes (43 bytes, hex) of the note's recipient.
    pub recipient_hex: String,
    pub memo: String,
}

pub struct Proof {
    pub txid: String,
    pub height: u32,
    pub outputs: Vec<Seen>,
}

impl Proof {
    pub fn paid_out_zat(&self) -> u64 {
        self.outputs.iter().filter(|o| o.direction == Direction::PayoutOut).map(|o| o.amount_zat).sum()
    }
}

fn memo_text(m: &zcash_protocol::memo::MemoBytes) -> String {
    match Memo::try_from(m) {
        Ok(Memo::Text(t)) => t.to_string(),
        _ => String::new(),
    }
}

/// Lender/tax-office side: trial-decrypt raw tx bytes with the employee's UFVK.
/// `height` is the block the tx was mined in (picks the consensus branch; the
/// lender reads the date from that block on any explorer).
pub fn verify_tx(ufvk: &UnifiedFullViewingKey, raw: &[u8], claimed_txid: Option<&str>, height: u32) -> Result<Proof, DisclosureError> {
    let h = BlockHeight::from_u32(height);
    let tx = Transaction::read(raw, BranchId::for_height(&NETWORK, h)).map_err(|e| DisclosureError::Unparseable(e.to_string()))?;
    let txid = tx.txid().to_string();
    if let Some(want) = claimed_txid {
        if want.trim() != txid {
            return Err(DisclosureError::TxidMismatch { want: want.trim().into(), got: txid });
        }
    }
    let keys = HashMap::from([(0u32, ufvk.clone())]);
    let d = decrypt_transaction(&NETWORK, Some(h), Some(h), &tx, &keys);
    let dir = |t: TransferType| match t {
        TransferType::Incoming => Direction::FundingIn,
        TransferType::Outgoing => Direction::PayoutOut,
        TransferType::WalletInternal | TransferType::AccountInternal => Direction::Change,
    };
    let mut outputs = Vec::new();
    for o in d.sapling_outputs() {
        outputs.push(Seen {
            direction: dir(o.transfer_type()),
            pool: o.value_pool(),
            amount_zat: o.note().value().inner(),
            recipient_hex: hex::encode(o.note().recipient().to_bytes()),
            memo: memo_text(o.memo()),
        });
    }
    for o in d.orchard_outputs().iter().chain(d.ironwood_outputs()) {
        let (note, _) = o.note();
        outputs.push(Seen {
            direction: dir(o.transfer_type()),
            pool: o.value_pool(),
            amount_zat: note.value().inner(),
            recipient_hex: hex::encode(note.recipient().to_raw_address_bytes()),
            memo: memo_text(o.memo()),
        });
    }
    if outputs.is_empty() {
        return Err(DisclosureError::NothingVisible);
    }
    Ok(Proof { txid, height, outputs })
}

/// Raw receiver bytes of a UA's Sapling and Orchard receivers, for matching [`Seen::recipient_hex`].
pub fn receiver_hexes(ua: &str) -> Vec<String> {
    match zcash_keys::address::Address::decode(&NETWORK, ua) {
        Some(zcash_keys::address::Address::Unified(u)) => {
            let mut v = Vec::new();
            if let Some(s) = u.sapling() {
                v.push(hex::encode(s.to_bytes()));
            }
            if let Some(o) = u.orchard() {
                v.push(hex::encode(o.to_raw_address_bytes()));
            }
            v
        }
        Some(zcash_keys::address::Address::Sapling(s)) => vec![hex::encode(s.to_bytes())],
        _ => Vec::new(),
    }
}

/// Step 1 of the two-step payout: the same lines, re-pointed at each employee's
/// child wallet UA and topped up by [`PAYOUT_FEE_ZAT`] so step 2 needs no change output.
pub fn funding_lines(seed: &EmployerSeed, lines: &[crate::PayLine]) -> Result<Vec<crate::PayLine>, DisclosureError> {
    check_accounts(lines.iter().map(|l| l.address.as_str()))?;
    lines
        .iter()
        .map(|l| {
            Ok(crate::PayLine {
                employee_id: l.employee_id.clone(),
                name: l.name.clone(),
                address: funding_address(seed, &l.address)?,
                amount: crate::money::Zat(l.amount.0 + PAYOUT_FEE_ZAT),
                memo: format!("payroll funding {}", l.employee_id),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> EmployerSeed {
        EmployerSeed::from_bytes(&[9u8; 32])
    }

    #[test]
    fn child_wallets_are_stable_distinct_and_keyed_on_the_employer_seed() {
        let k = |s: &EmployerSeed, a: &str| encode_ufvk(&employee_ufvk(s, a).unwrap()).unwrap();
        assert_eq!(k(&seed(), "emp-001"), k(&seed(), "emp-001"));
        assert_ne!(k(&seed(), "emp-001"), k(&seed(), "emp-002"));
        assert_ne!(k(&seed(), "emp-001"), k(&EmployerSeed::from_bytes(&[8u8; 32]), "emp-001"));
        assert_ne!(k(&seed(), "emp-001"), encode_ufvk(&treasury_ufvk(&seed()).unwrap()).unwrap());
        assert_eq!(child_wallet(&seed(), "emp-001").unwrap().phrase_for_wallet_env().split(' ').count(), 24);
        assert_eq!(format!("{:?}", child_wallet(&seed(), "emp-001").unwrap()), "ChildWallet(<redacted>)");
        check_accounts(["emp-001", "emp-002", "emp-003"]).unwrap();
        assert!(matches!(check_accounts(["a", "a"]), Err(DisclosureError::AccountCollision(..))));
    }

    #[test]
    fn treasury_is_never_exported_and_export_has_no_spending_key() {
        let text = export(&seed(), "emp-001", "emp-001", "Ada").unwrap();
        crate::audit::check_viewing_key(text.lines().find_map(|l| l.strip_prefix("ufvk=")).unwrap()).unwrap();
        assert!(!text.contains("secret") && !text.contains("usk"));
        let treasury = encode_ufvk(&treasury_ufvk(&seed()).unwrap()).unwrap();
        assert!(!text.contains(&treasury));
        let (id, k) = parse_export(&text).unwrap();
        assert_eq!(id, "emp-001");
        assert_eq!(encode_ufvk(&k).unwrap(), encode_ufvk(&employee_ufvk(&seed(), "emp-001").unwrap()).unwrap());
    }

    #[test]
    fn seed_debug_is_redacted_and_bad_mnemonic_hides_value() {
        assert_eq!(format!("{:?}", seed()), "EmployerSeed(<redacted>)");
        let phrase = bip0039::Mnemonic::<bip0039::English>::from_entropy(vec![3u8; 32]).unwrap();
        EmployerSeed::from_mnemonic(phrase.phrase()).unwrap();
        let e = EmployerSeed::from_mnemonic("hunter2 not a phrase").unwrap_err().to_string();
        assert!(!e.contains("hunter2"));
    }
}
