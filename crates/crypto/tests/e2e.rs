//! Alice and Bob set up an Olm session through OutpostWave's signed bundles.

use ow_crypto::{
    CryptoAccount, CryptoError, Envelope, EnvelopeKind, KeyKind, PeerSessions, PrekeyBundle,
    pickle_key,
};
use ow_identity::DeviceKey;

struct Device {
    key: DeviceKey,
    account: CryptoAccount,
}

fn device() -> Device {
    Device {
        key: DeviceKey::generate().unwrap(),
        account: CryptoAccount::new(),
    }
}

/// Sends `text` from `from` to `to` through the wire format.
fn send(
    from: &mut Device,
    from_sessions: &mut PeerSessions,
    to: &mut Device,
    to_sessions: &mut PeerSessions,
    text: &[u8],
) -> (EnvelopeKind, Vec<u8>) {
    let env = from_sessions
        .encrypt(&mut from.account, &from.key, text, 1)
        .unwrap();
    let wire = env.to_bytes();
    let received = Envelope::from_bytes(&wire).unwrap();
    let plain = to_sessions
        .decrypt(&mut to.account, &received, &from.key.public_key())
        .unwrap();
    (received.kind(), plain)
}

#[test]
fn full_conversation() {
    let mut alice = device();
    let mut bob = device();

    // Bob's bundle reaches Alice (invite code); she checks it against Bob's device key.
    let wire = bob.account.fallback_bundle(&bob.key, 100).to_bytes();
    let bundle = PrekeyBundle::from_bytes(&wire, &bob.key.public_key()).unwrap();
    assert_eq!(bundle.kind(), KeyKind::Fallback);

    let mut a2b = PeerSessions::new(bob.key.public_key());
    let mut b2a = PeerSessions::new(alice.key.public_key());
    a2b.start(&alice.account, &bundle).unwrap();

    // First messages are pre-key messages until Bob answers.
    assert_eq!(
        send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"salut"),
        (EnvelopeKind::PreKey, b"salut".to_vec())
    );
    assert_eq!(
        send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"ca va ?"),
        (EnvelopeKind::PreKey, b"ca va ?".to_vec())
    );
    // Bob answers: normal messages from now on, both ways.
    assert_eq!(
        send(&mut bob, &mut b2a, &mut alice, &mut a2b, b"oui"),
        (EnvelopeKind::Normal, b"oui".to_vec())
    );
    assert_eq!(
        send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"top"),
        (EnvelopeKind::Normal, b"top".to_vec())
    );
}

#[test]
fn one_time_bundle_works() {
    let mut alice = device();
    let mut bob = device();
    let bundle = bob.account.one_time_bundle(&bob.key, 1);
    assert_eq!(bundle.kind(), KeyKind::OneTime);
    let mut a2b = PeerSessions::new(bob.key.public_key());
    let mut b2a = PeerSessions::new(alice.key.public_key());
    a2b.start(&alice.account, &bundle).unwrap();
    assert_eq!(send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"x").1, b"x");
}

#[test]
fn bundle_from_other_device_is_rejected() {
    let mut bob = device();
    let mallory = device();
    let wire = bob.account.fallback_bundle(&bob.key, 1).to_bytes();
    assert_eq!(
        PrekeyBundle::from_bytes(&wire, &mallory.key.public_key()),
        Err(CryptoError::WrongDevice)
    );
    let mut bad = wire.clone();
    bad[50] ^= 1;
    assert_eq!(
        PrekeyBundle::from_bytes(&bad, &bob.key.public_key()),
        Err(CryptoError::InvalidSignature)
    );
}

#[test]
fn replay_tampering_and_spoofing_fail() {
    let mut alice = device();
    let mut bob = device();
    let bundle = bob.account.fallback_bundle(&bob.key, 1);
    let mut a2b = PeerSessions::new(bob.key.public_key());
    let mut b2a = PeerSessions::new(alice.key.public_key());
    a2b.start(&alice.account, &bundle).unwrap();
    send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"hello");
    send(&mut bob, &mut b2a, &mut alice, &mut a2b, b"hi");

    let env = a2b
        .encrypt(&mut alice.account, &alice.key, b"secret", 1)
        .unwrap();
    let wire = env.to_bytes();

    // Tampered ciphertext.
    let mut t = wire.clone();
    let last = t.len() - 1;
    t[last] ^= 1;
    let tampered = Envelope::from_bytes(&t).unwrap();
    assert!(
        b2a.decrypt(&mut bob.account, &tampered, &alice.key.public_key())
            .is_err()
    );

    // Wrong transport sender (someone else relays Alice's envelope as their own).
    let mallory = device();
    let e = Envelope::from_bytes(&wire).unwrap();
    assert_eq!(
        b2a.decrypt(&mut bob.account, &e, &mallory.key.public_key()),
        Err(CryptoError::WrongDevice)
    );

    // Legit delivery, then replay.
    assert_eq!(
        b2a.decrypt(&mut bob.account, &e, &alice.key.public_key())
            .unwrap(),
        b"secret"
    );
    assert!(
        b2a.decrypt(&mut bob.account, &e, &alice.key.public_key())
            .is_err(),
        "replay accepted"
    );
}

#[test]
fn state_survives_restart() {
    let mut alice = device();
    let mut bob = device();
    let bundle = bob.account.fallback_bundle(&bob.key, 1);
    let mut a2b = PeerSessions::new(bob.key.public_key());
    let mut b2a = PeerSessions::new(alice.key.public_key());
    a2b.start(&alice.account, &bundle).unwrap();
    send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"1");

    // Bob restarts: account and sessions come back from (encrypted) pickles.
    let pk = pickle_key(&bob.key);
    let acc = bob.account.pickle(&pk);
    let ses = b2a.pickle(&pk);
    bob.account = CryptoAccount::from_pickle(&acc, &pk).unwrap();
    let mut b2a = PeerSessions::from_pickle(alice.key.public_key(), &ses, &pk).unwrap();
    assert!(b2a.is_established());
    assert_eq!(send(&mut bob, &mut b2a, &mut alice, &mut a2b, b"2").1, b"2");
    assert_eq!(send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"3").1, b"3");

    // Wrong pickle key.
    assert!(CryptoAccount::from_pickle(&acc, &[0; 32]).is_err());
}

#[test]
fn simultaneous_session_setup_converges() {
    let mut alice = device();
    let mut bob = device();
    let ab = bob.account.fallback_bundle(&bob.key, 1);
    let ba = alice.account.fallback_bundle(&alice.key, 1);
    let mut a2b = PeerSessions::new(bob.key.public_key());
    let mut b2a = PeerSessions::new(alice.key.public_key());
    // Both start a session at the same time.
    a2b.start(&alice.account, &ab).unwrap();
    b2a.start(&bob.account, &ba).unwrap();
    assert_eq!(
        send(&mut alice, &mut a2b, &mut bob, &mut b2a, b"from a").1,
        b"from a"
    );
    assert_eq!(
        send(&mut bob, &mut b2a, &mut alice, &mut a2b, b"from b").1,
        b"from b"
    );
    for i in 0..3u8 {
        assert_eq!(send(&mut alice, &mut a2b, &mut bob, &mut b2a, &[i]).1, [i]);
        assert_eq!(send(&mut bob, &mut b2a, &mut alice, &mut a2b, &[i]).1, [i]);
    }
}
