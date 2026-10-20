use zec_payroll::attest::*;
fn base() -> Attestation { Attestation { network: "testnet".into(), txid: "ab".repeat(32), child_address: "utest1abc".into(), amount_zat: 1250000, pay_period: "adhoc:oct".into() } }
#[test]
fn attacks() {
    let k = EmployerKey::from_bytes(&[1u8; 32]);
    let pk = parse_pubkey(&k.public_hex()).unwrap();
    let a = base(); let sig = k.sign(&a).unwrap();
    verify(&pk, &k.public_hex(), &sig, &a).unwrap();
    let f = EmployerKey::from_bytes(&[2u8; 32]);
    // random key, with its own pubkey swapped into the proof
    assert!(matches!(verify(&pk, &f.public_hex(), &f.sign(&a).unwrap(), &a), Err(AttestError::WrongSigner { .. })));
    // random key signature under the real pubkey
    assert!(verify(&pk, &k.public_hex(), &f.sign(&a).unwrap(), &a).is_err());
    // each field changed / replay onto other txid or employee
    let muts: Vec<Attestation> = vec![
        Attestation { network: "mainnet".into(), ..base() }, Attestation { txid: "cd".repeat(32), ..base() },
        Attestation { child_address: "utest1abd".into(), ..base() }, Attestation { amount_zat: 1250001, ..base() },
        Attestation { pay_period: "adhoc:nov".into(), ..base() }];
    for m in &muts { assert_eq!(verify(&pk, &k.public_hex(), &sig, m), Err(AttestError::Invalid)); }
    // field-boundary smuggling refused at canonicalisation
    for bad in [Attestation { child_address: "utest1abc\namount_zat=1".into(), ..base() }, Attestation { pay_period: "a b".into(), ..base() }, Attestation { txid: "AB".repeat(32), ..base() }] {
        assert!(matches!(bad.canonical(), Err(AttestError::BadField(_))));
    }
    // stripped / truncated sig
    assert!(verify(&pk, &k.public_hex(), "", &a).is_err());
    assert!(verify(&pk, &k.public_hex(), &sig[..126], &a).is_err());
}
