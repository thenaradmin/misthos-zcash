//! Employer attestation of one step-2 payout: the treasury signs, with ed25519, a
//! canonical message binding the payout txid, the paying child wallet's address, the
//! amount in zatoshis, the pay period and the network. A verifier checks it against
//! the employer public key it already trusts (published in `docs/employer-pubkey.txt`,
//! or passed in), never against the key inside the proof file alone.
//!
//! The signing key comes from `PAYROLL_EMPLOYER_SIGNING_KEY` (64 hex chars, the ed25519
//! seed) or `PAYROLL_EMPLOYER_SIGNING_KEY_FILE` (same content, file mode 0600). It is never
//! printed, logged, or put in an error message.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// First line of the signed message; separates it from anything else this key might sign.
pub const DOMAIN: &str = "zec-payroll-employer-attestation v1";
/// Repo file holding the published employer public key (hex), used when nothing else is given.
pub const PUBKEY_FILE: &str = "docs/employer-pubkey.txt";

#[derive(Debug, PartialEq, Eq)]
pub enum AttestError {
    NoKey,
    BothKeys,
    KeyFile { path: String, why: String },
    BadKey(&'static str),
    BadField(String),
    BadPubkey(String),
    NoExpectedPubkey,
    BadSignature(String),
    WrongSigner { expected: String, got: String },
    Invalid,
}

impl std::fmt::Display for AttestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use AttestError::*;
        match self {
            NoKey => write!(f, "PAYROLL_EMPLOYER_SIGNING_KEY not configured (or PAYROLL_EMPLOYER_SIGNING_KEY_FILE); create one with `payroll keygen-signing <file>`"),
            BothKeys => write!(f, "both PAYROLL_EMPLOYER_SIGNING_KEY and PAYROLL_EMPLOYER_SIGNING_KEY_FILE are set; set only one"),
            KeyFile { path, why } => write!(f, "PAYROLL_EMPLOYER_SIGNING_KEY_FILE {path}: {why}"),
            BadKey(var) => write!(f, "{var} is not 64 hex chars (value not shown)"),
            BadField(e) => write!(f, "cannot sign/verify attestation: {e}"),
            BadPubkey(e) => write!(f, "employer public key invalid: {e}"),
            NoExpectedPubkey => write!(f, "no expected employer public key: pass --employer-pubkey <hex>, set PAYROLL_EMPLOYER_PUBKEY, or publish it in {PUBKEY_FILE}"),
            BadSignature(e) => write!(f, "employer signature malformed: {e}"),
            WrongSigner { expected, got } => write!(f, "proof is signed by {got}, not the expected employer key {expected}: REJECTED"),
            Invalid => write!(f, "employer signature does not match the txid/child address/amount/period/network in this proof: REJECTED"),
        }
    }
}

impl std::error::Error for AttestError {}

/// What the employer signs for one payout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub network: String,
    /// Step-2 payout txid (display order, lowercase hex).
    pub txid: String,
    /// The employee's child wallet UA that sent the payout.
    pub child_address: String,
    /// Amount paid to the employee, in zatoshis.
    pub amount_zat: u64,
    pub pay_period: String,
}

impl Attestation {
    /// The exact bytes signed. Every field is one line; values that could smuggle in
    /// another line (or are malformed) are refused instead of escaped.
    pub fn canonical(&self) -> Result<Vec<u8>, AttestError> {
        let bad = |what: &str, v: &str| AttestError::BadField(format!("{what} `{}` is empty or has whitespace/control chars", v.escape_debug()));
        let token = |v: &str| !v.is_empty() && !v.chars().any(|c| c.is_whitespace() || c.is_control());
        if !matches!(self.network.as_str(), "testnet" | "mainnet") {
            return Err(bad("network", &self.network));
        }
        if self.txid.len() != 64 || !self.txid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(AttestError::BadField(format!("txid `{}` is not 64 lowercase hex chars", self.txid.escape_debug())));
        }
        if !token(&self.child_address) {
            return Err(bad("child address", &self.child_address));
        }
        if !token(&self.pay_period) {
            return Err(bad("pay period", &self.pay_period));
        }
        Ok(format!(
            "{DOMAIN}\nnetwork={}\ntxid={}\nchild_address={}\namount_zat={}\npay_period={}\n",
            self.network, self.txid, self.child_address, self.amount_zat, self.pay_period
        )
        .into_bytes())
    }
}

/// The employer's ed25519 signing key. `Debug` is redacted; the key zeroizes on drop.
pub struct EmployerKey(SigningKey);

impl std::fmt::Debug for EmployerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmployerKey(<redacted>)")
    }
}

fn seed_from_hex(s: &str, var: &'static str) -> Result<SigningKey, AttestError> {
    let mut b = [0u8; 32];
    hex::decode_to_slice(s.trim(), &mut b).map_err(|_| AttestError::BadKey(var))?;
    Ok(SigningKey::from_bytes(&b))
}

impl EmployerKey {
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        EmployerKey(SigningKey::from_bytes(seed))
    }

    /// From `PAYROLL_EMPLOYER_SIGNING_KEY` or a 0600 `PAYROLL_EMPLOYER_SIGNING_KEY_FILE`.
    pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self, AttestError> {
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        match (get("PAYROLL_EMPLOYER_SIGNING_KEY"), get("PAYROLL_EMPLOYER_SIGNING_KEY_FILE")) {
            (Some(_), Some(_)) => Err(AttestError::BothKeys),
            (None, None) => Err(AttestError::NoKey),
            (Some(v), None) => Ok(EmployerKey(seed_from_hex(&v, "PAYROLL_EMPLOYER_SIGNING_KEY")?)),
            (None, Some(path)) => {
                use std::os::unix::fs::PermissionsExt;
                let err = |why: String| AttestError::KeyFile { path: path.clone(), why };
                let meta = std::fs::metadata(&path).map_err(|e| err(e.to_string()))?;
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    return Err(err(format!("mode {mode:o} lets other users read it; run `chmod 600 {path}`")));
                }
                let text = std::fs::read_to_string(&path).map_err(|e| err(e.to_string()))?;
                Ok(EmployerKey(seed_from_hex(&text, "PAYROLL_EMPLOYER_SIGNING_KEY_FILE")?))
            }
        }
    }

    /// New random key written to `path` (created 0600, never overwritten). Returns the public key hex.
    pub fn generate_to_file(path: &str) -> Result<String, AttestError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let err = |why: String| AttestError::KeyFile { path: path.to_string(), why };
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| err(format!("OS random number generator: {e}")))?;
        let key = EmployerKey::from_bytes(&seed);
        let opened = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
        let res = opened.and_then(|mut f| writeln!(f, "{}", hex::encode(seed)).and_then(|_| f.sync_all()));
        seed.fill(0);
        res.map_err(|e| err(e.to_string()))?;
        Ok(key.public_hex())
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.0.verifying_key().as_bytes())
    }

    /// Hex ed25519 signature over [`Attestation::canonical`].
    pub fn sign(&self, a: &Attestation) -> Result<String, AttestError> {
        Ok(hex::encode(self.0.sign(&a.canonical()?).to_bytes()))
    }
}

pub fn parse_pubkey(s: &str) -> Result<VerifyingKey, AttestError> {
    let mut b = [0u8; 32];
    hex::decode_to_slice(s.trim(), &mut b).map_err(|e| AttestError::BadPubkey(format!("not 64 hex chars: {e}")))?;
    VerifyingKey::from_bytes(&b).map_err(|e| AttestError::BadPubkey(e.to_string()))
}

/// The key a verifier trusts: `--employer-pubkey`, else `PAYROLL_EMPLOYER_PUBKEY`, else the
/// first non-comment line of [`PUBKEY_FILE`]. Returns the key and where it came from.
pub fn expected_pubkey(flag: Option<&str>, get: impl Fn(&str) -> Option<String>) -> Result<(VerifyingKey, String), AttestError> {
    if let Some(v) = flag {
        return Ok((parse_pubkey(v)?, "--employer-pubkey".into()));
    }
    if let Some(v) = get("PAYROLL_EMPLOYER_PUBKEY").filter(|v| !v.trim().is_empty()) {
        return Ok((parse_pubkey(&v)?, "PAYROLL_EMPLOYER_PUBKEY".into()));
    }
    let text = std::fs::read_to_string(PUBKEY_FILE).map_err(|_| AttestError::NoExpectedPubkey)?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).ok_or(AttestError::NoExpectedPubkey)?;
    Ok((parse_pubkey(line)?, PUBKEY_FILE.into()))
}

/// Accept only if the proof names the expected signer AND the signature is valid over `a`.
pub fn verify(expected: &VerifyingKey, signer_hex: &str, sig_hex: &str, a: &Attestation) -> Result<(), AttestError> {
    let signer = parse_pubkey(signer_hex)?;
    if signer != *expected {
        return Err(AttestError::WrongSigner { expected: hex::encode(expected.as_bytes()), got: hex::encode(signer.as_bytes()) });
    }
    let mut b = [0u8; 64];
    hex::decode_to_slice(sig_hex.trim(), &mut b).map_err(|e| AttestError::BadSignature(format!("not 128 hex chars: {e}")))?;
    expected.verify_strict(&a.canonical()?, &Signature::from_bytes(&b)).map_err(|_| AttestError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att() -> Attestation {
        Attestation {
            network: "testnet".into(),
            txid: "ab".repeat(32),
            child_address: "utest1childaddress".into(),
            amount_zat: 1_250_000,
            pay_period: "adhoc:oct-2026".into(),
        }
    }

    fn employer() -> EmployerKey {
        EmployerKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn valid_signature_passes() {
        let k = employer();
        let sig = k.sign(&att()).unwrap();
        let pk = parse_pubkey(&k.public_hex()).unwrap();
        verify(&pk, &k.public_hex(), &sig, &att()).unwrap();
    }

    #[test]
    fn forged_with_other_key_is_rejected() {
        let (k, forger) = (employer(), EmployerKey::from_bytes(&[8u8; 32]));
        let expected = parse_pubkey(&k.public_hex()).unwrap();
        let sig = forger.sign(&att()).unwrap();
        // Forger names their own key: wrong signer.
        assert!(matches!(verify(&expected, &forger.public_hex(), &sig, &att()), Err(AttestError::WrongSigner { .. })));
        // Forger names the employer's key: signature invalid.
        assert_eq!(verify(&expected, &k.public_hex(), &sig, &att()), Err(AttestError::Invalid));
    }

    #[test]
    fn any_changed_field_is_rejected() {
        let k = employer();
        let (pk, sig) = (parse_pubkey(&k.public_hex()).unwrap(), k.sign(&att()).unwrap());
        let changed = [
            Attestation { amount_zat: 1_250_001, ..att() },
            Attestation { pay_period: "adhoc:nov-2026".into(), ..att() },
            Attestation { network: "mainnet".into(), ..att() },
            Attestation { txid: "cd".repeat(32), ..att() },
            Attestation { child_address: "utest1other".into(), ..att() },
        ];
        for a in &changed {
            assert_eq!(verify(&pk, &k.public_hex(), &sig, a), Err(AttestError::Invalid), "{a:?}");
        }
        // And a mainnet-signed attestation does not verify as testnet.
        let main_sig = k.sign(&Attestation { network: "mainnet".into(), ..att() }).unwrap();
        assert_eq!(verify(&pk, &k.public_hex(), &main_sig, &att()), Err(AttestError::Invalid));
    }

    #[test]
    fn fields_cannot_smuggle_lines() {
        let k = employer();
        for a in [
            Attestation { pay_period: "p\namount_zat=99".into(), ..att() },
            Attestation { child_address: "".into(), ..att() },
            Attestation { network: "regtest".into(), ..att() },
            Attestation { txid: "AB".repeat(32), ..att() },
        ] {
            assert!(matches!(k.sign(&a), Err(AttestError::BadField(_))), "{a:?}");
        }
    }

    #[test]
    fn key_loading_never_shows_the_secret() {
        assert_eq!(format!("{:?}", employer()), "EmployerKey(<redacted>)");
        assert_eq!(EmployerKey::load(|_| None).unwrap_err(), AttestError::NoKey);
        let secret = "zz".repeat(32);
        let e = EmployerKey::load(|k| (k == "PAYROLL_EMPLOYER_SIGNING_KEY").then(|| secret.clone())).unwrap_err().to_string();
        assert!(!e.contains(&secret) && e.contains("PAYROLL_EMPLOYER_SIGNING_KEY"), "{e}");
        let both = EmployerKey::load(|_| Some("00".repeat(32))).unwrap_err();
        assert_eq!(both, AttestError::BothKeys);
        let k = EmployerKey::load(|k| (k == "PAYROLL_EMPLOYER_SIGNING_KEY").then(|| "07".repeat(32))).unwrap();
        assert_eq!(k.public_hex(), employer().public_hex());
    }

    #[test]
    fn key_file_must_be_0600_and_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("zpp-signing-{}.key", std::process::id()));
        let path = p.to_str().unwrap().to_string();
        std::fs::remove_file(&p).ok();
        let pubhex = EmployerKey::generate_to_file(&path).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(EmployerKey::generate_to_file(&path).is_err(), "must not overwrite an existing key");
        let from_file = |k: &str| (k == "PAYROLL_EMPLOYER_SIGNING_KEY_FILE").then(|| path.clone());
        assert_eq!(EmployerKey::load(from_file).unwrap().public_hex(), pubhex);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = EmployerKey::load(from_file).unwrap_err().to_string();
        assert!(e.contains("chmod 600"), "{e}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn expected_pubkey_prefers_flag_then_env() {
        let k = employer().public_hex();
        let other = EmployerKey::from_bytes(&[9u8; 32]).public_hex();
        let (pk, src) = expected_pubkey(Some(&k), |_| Some(other.clone())).unwrap();
        assert_eq!((hex::encode(pk.as_bytes()), src.as_str()), (k.clone(), "--employer-pubkey"));
        let (pk, src) = expected_pubkey(None, |_| Some(other.clone())).unwrap();
        assert_eq!((hex::encode(pk.as_bytes()), src.as_str()), (other, "PAYROLL_EMPLOYER_PUBKEY"));
        assert!(matches!(expected_pubkey(Some("nothex"), |_| None), Err(AttestError::BadPubkey(_))));
    }
}
