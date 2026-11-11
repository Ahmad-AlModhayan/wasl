use super::*;
use crate::identity::DeviceKey;

const T0: u64 = 1_700_000_000;

fn inviter_key() -> DeviceKey {
    DeviceKey::from_seed(&[1; 32])
}

fn joiner_key() -> DeviceKey {
    DeviceKey::from_seed(&[2; 32])
}

fn attacker_key() -> DeviceKey {
    DeviceKey::from_seed(&[9; 32])
}

fn addrs() -> Vec<SocketAddr> {
    vec![
        "192.168.1.20:22000".parse().unwrap(),
        "[fe80::1234:5678]:22001".parse().unwrap(),
    ]
}

fn invitation() -> Invitation {
    Invitation::new(inviter_key().id(), addrs(), T0).unwrap()
}

/// Delivers messages both ways until both sides are quiet. Returns everything each side sent.
fn pump(
    inviter: &mut Session,
    joiner: &mut Session,
    to_inviter: Vec<Message>,
    now: u64,
) -> (Vec<Message>, Vec<Message>) {
    let (mut sent_by_inviter, mut sent_by_joiner) = (Vec::new(), to_inviter.clone());
    let mut to_inviter = to_inviter;
    let mut to_joiner = Vec::new();
    while !to_inviter.is_empty() || !to_joiner.is_empty() {
        for m in std::mem::take(&mut to_inviter) {
            let out = inviter.handle(&m, now);
            sent_by_inviter.extend(out.iter().cloned());
            to_joiner.extend(out);
        }
        for m in std::mem::take(&mut to_joiner) {
            let out = joiner.handle(&m, now);
            sent_by_joiner.extend(out.iter().cloned());
            to_inviter.extend(out);
        }
    }
    (sent_by_inviter, sent_by_joiner)
}

/// Runs a session pair up to the confirmation step.
fn to_words(invitation: &mut Invitation, code: &JoinCode) -> (Session, Session) {
    let mut inviter = invitation.start(inviter_key().public(), T0 + 5).unwrap();
    let (mut joiner, hello) = Session::join(code, joiner_key().public(), T0 + 5).unwrap();
    pump(&mut inviter, &mut joiner, vec![hello], T0 + 5);
    (inviter, joiner)
}

fn confirm_both(inviter: &mut Session, joiner: &mut Session) {
    let m = inviter.confirm(T0 + 10);
    assert_eq!(m, vec![Message::Confirm]);
    for msg in &m {
        joiner.handle(msg, T0 + 10);
    }
    let m = joiner.confirm(T0 + 11);
    for msg in &m {
        inviter.handle(msg, T0 + 11);
    }
}

fn assert_never_paired(s: &Session) {
    assert!(matches!(s.state(), State::Aborted(_)), "{:?}", s.state());
    assert_eq!(s.paired_peer(), None);
    assert_eq!(s.words(), None);
}

fn hello_from(key: &DeviceKey, kind: CodeKind) -> Message {
    Message::Hello {
        pk: key.public(),
        nonce: [7; 32],
        kind,
    }
}

fn proof_in(msgs: &[Message]) -> [u8; 32] {
    msgs.iter()
        .find_map(|m| match m {
            Message::Proof { mac } => Some(*mac),
            _ => None,
        })
        .expect("a proof was sent")
}

#[test]
fn qr_happy_path_pairs_both_roles_with_the_same_words() {
    let mut inv = invitation();
    let code = PairingCode::from_qr(&inv.code().to_qr()).unwrap();
    let (mut inviter, mut joiner) = to_words(&mut inv, &JoinCode::Qr(code));

    let waiting = State::AwaitingConfirm {
        local: false,
        peer: false,
    };
    assert_eq!(inviter.state(), waiting);
    assert_eq!(joiner.state(), waiting);
    let words = inviter.words().unwrap();
    assert_eq!(joiner.words(), Some(words));
    assert!(words.iter().all(|w| WORDS.contains(w)));
    // Words alone pair nothing.
    assert_eq!(inviter.paired_peer(), None);
    assert_eq!(joiner.paired_peer(), None);

    confirm_both(&mut inviter, &mut joiner);
    assert_eq!(inviter.state(), State::Paired);
    assert_eq!(joiner.state(), State::Paired);
    assert_eq!(inviter.paired_peer().unwrap().pk, joiner_key().public());
    assert_eq!(inviter.paired_peer().unwrap().id, joiner_key().id());
    assert_eq!(joiner.paired_peer().unwrap().pk, inviter_key().public());
    assert_eq!(joiner.words(), Some(words));
}

#[test]
fn short_code_happy_path_pairs_with_the_same_words() {
    let mut inv = invitation();
    let typed = inv.enable_short_code().to_string().to_lowercase();
    let code = ShortCode::parse(&typed).unwrap();
    let (mut inviter, mut joiner) = to_words(&mut inv, &JoinCode::Short(code));
    assert!(inviter.words().is_some());
    assert_eq!(inviter.words(), joiner.words());
    confirm_both(&mut inviter, &mut joiner);
    assert_eq!(inviter.state(), State::Paired);
    assert_eq!(joiner.state(), State::Paired);
}

#[test]
fn words_differ_between_sessions() {
    let mut a = invitation();
    let code_a = a.code().clone();
    let (ia, _) = to_words(&mut a, &JoinCode::Qr(code_a));
    let mut b = invitation();
    let code_b = b.code().clone();
    let (ib, _) = to_words(&mut b, &JoinCode::Qr(code_b));
    assert_ne!(ia.words(), ib.words());
}

#[test]
fn wrong_qr_secret_never_pairs() {
    let mut inv = invitation();
    let mut code = inv.code().clone();
    code.secret[19] ^= 1;
    let (inviter, joiner) = to_words(&mut inv, &JoinCode::Qr(code));
    // The joiner checks the inviter's proof first, fails, and aborts; the inviter follows.
    assert_eq!(joiner.state(), State::Aborted(AbortReason::ProofFailed));
    assert_eq!(inviter.state(), State::Aborted(AbortReason::PeerAborted));
    assert_never_paired(&inviter);
    assert_never_paired(&joiner);
}

#[test]
fn wrong_short_code_never_pairs() {
    let mut inv = invitation();
    let real = inv.enable_short_code().to_string();
    let first = if real.as_bytes()[0] == b'2' { '3' } else { '2' };
    let wrong: String = std::iter::once(first).chain(real.chars().skip(1)).collect();
    let (inviter, joiner) = to_words(&mut inv, &JoinCode::Short(ShortCode::parse(&wrong).unwrap()));
    assert_eq!(joiner.state(), State::Aborted(AbortReason::ProofFailed));
    assert_never_paired(&inviter);
    assert_never_paired(&joiner);
}

#[test]
fn a_joiner_without_the_secret_fails_at_the_inviter() {
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    let out = inviter.handle(&hello_from(&attacker_key(), CodeKind::Qr), T0);
    assert!(matches!(out[0], Message::Hello { .. }));
    assert!(matches!(out[1], Message::Proof { .. }));
    let out = inviter.handle(&Message::Proof { mac: [0x5a; 32] }, T0);
    assert_eq!(out, vec![Message::Abort]);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::ProofFailed));
    // Nothing afterwards revives it.
    assert!(inviter.confirm(T0).is_empty());
    inviter.handle(&Message::Confirm, T0);
    assert_never_paired(&inviter);
}

#[test]
fn a_fake_inviter_cannot_prove_and_learns_nothing() {
    let inv = invitation();
    let code = JoinCode::Qr(inv.code().clone());
    // A QR joiner refuses a key that is not the coded device…
    let (mut joiner, _) = Session::join(&code, joiner_key().public(), T0).unwrap();
    let out = joiner.handle(&hello_from(&attacker_key(), CodeKind::Qr), T0);
    assert_eq!(out, vec![Message::Abort]);
    assert_eq!(joiner.state(), State::Aborted(AbortReason::WrongDevice));

    // …and a short-code joiner sends nothing until the candidate's proof checks out.
    let short = JoinCode::Short(inv.code().short_code());
    let (mut joiner, _) = Session::join(&short, joiner_key().public(), T0).unwrap();
    assert!(
        joiner
            .handle(&hello_from(&attacker_key(), CodeKind::Short), T0)
            .is_empty()
    );
    let out = joiner.handle(&Message::Proof { mac: [1; 32] }, T0);
    assert_eq!(out, vec![Message::Abort]);
    assert_never_paired(&joiner);
}

#[test]
fn a_reflected_proof_fails() {
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    let out = inviter.handle(&hello_from(&attacker_key(), CodeKind::Qr), T0);
    let mac = proof_in(&out);
    inviter.handle(&Message::Proof { mac }, T0);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::ProofFailed));

    // A Hello carrying the inviter's own key is refused outright.
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    inviter.handle(&hello_from(&inviter_key(), CodeKind::Qr), T0);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::BadHello));
}

#[test]
fn a_replayed_proof_fails_in_a_new_session() {
    // Two invitations with the same secret, as if the attacker could get the code reused.
    let mut first = invitation();
    let code = first.code().clone();
    let mut second = Invitation {
        code: code.clone(),
        created_at: T0,
        short_shown: false,
        used: false,
    };

    let mut inviter = first.start(inviter_key().public(), T0).unwrap();
    let (mut joiner, hello) = Session::join(&JoinCode::Qr(code.clone()), joiner_key().public(), T0).unwrap();
    let (sent_by_inviter, sent_by_joiner) = pump(&mut inviter, &mut joiner, vec![hello.clone()], T0);
    let joiner_mac = proof_in(&sent_by_joiner);

    // Replay the joiner's Hello and Proof into the second inviter session: fresh nonce_I.
    let mut inviter2 = second.start(inviter_key().public(), T0).unwrap();
    inviter2.handle(&hello, T0);
    inviter2.handle(&Message::Proof { mac: joiner_mac }, T0);
    assert_eq!(inviter2.state(), State::Aborted(AbortReason::ProofFailed));

    // Replay the inviter's Hello and Proof to a new joiner session: fresh nonce_J.
    let (mut joiner2, _) = Session::join(&JoinCode::Qr(code), joiner_key().public(), T0).unwrap();
    for m in &sent_by_inviter {
        joiner2.handle(m, T0);
    }
    assert_eq!(joiner2.state(), State::Aborted(AbortReason::ProofFailed));
}

#[test]
fn an_expired_code_fails() {
    let mut inv = invitation();
    assert_eq!(
        inv.start(inviter_key().public(), T0 + CODE_TTL_SECS).err(),
        Some(PairError::Expired)
    );

    // Started in time, but the Hello arrives after expiry.
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0 + CODE_TTL_SECS - 1).unwrap();
    let out = inviter.handle(&hello_from(&joiner_key(), CodeKind::Qr), T0 + CODE_TTL_SECS);
    assert_eq!(out, vec![Message::Abort]);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::Expired));

    // Words shown, but confirmed after expiry.
    let mut inv = invitation();
    let code = JoinCode::Qr(inv.code().clone());
    let (mut inviter, _) = to_words(&mut inv, &code);
    assert_eq!(inviter.confirm(T0 + CODE_TTL_SECS), vec![Message::Abort]);
    assert_never_paired(&inviter);
}

#[test]
fn a_slow_peer_times_out() {
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    inviter.handle(&hello_from(&joiner_key(), CodeKind::Qr), T0);
    assert_eq!(inviter.tick(T0 + PROOF_TIMEOUT_SECS), Vec::new());
    assert_eq!(inviter.tick(T0 + PROOF_TIMEOUT_SECS + 1), vec![Message::Abort]);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::Timeout));
}

#[test]
fn a_code_used_twice_fails() {
    let mut inv = invitation();
    let code = JoinCode::Qr(inv.code().clone());
    let (mut inviter, mut joiner) = to_words(&mut inv, &code);
    confirm_both(&mut inviter, &mut joiner);
    assert_eq!(inviter.state(), State::Paired);
    assert_eq!(
        inv.start(inviter_key().public(), T0 + 20).err(),
        Some(PairError::Used)
    );

    // Used up even when the first attempt failed.
    let mut inv = invitation();
    let mut s = inv.start(inviter_key().public(), T0).unwrap();
    s.handle(&Message::Abort, T0);
    assert_eq!(inv.start(inviter_key().public(), T0).err(), Some(PairError::Used));
}

#[test]
fn rejected_words_never_add() {
    for rejecter in [Role::Inviter, Role::Joiner] {
        let mut inv = invitation();
        let code = JoinCode::Qr(inv.code().clone());
        let (mut inviter, mut joiner) = to_words(&mut inv, &code);
        // One side already confirmed; the other says the words differ.
        let (confirmer, other) = match rejecter {
            Role::Inviter => (&mut joiner, &mut inviter),
            Role::Joiner => (&mut inviter, &mut joiner),
        };
        let c = confirmer.confirm(T0 + 6);
        for m in &c {
            other.handle(m, T0 + 6);
        }
        let abort = other.reject();
        assert_eq!(abort, vec![Message::Abort]);
        for m in &abort {
            confirmer.handle(m, T0 + 7);
        }
        assert_eq!(other.state(), State::Aborted(AbortReason::Rejected));
        assert_eq!(confirmer.state(), State::Aborted(AbortReason::PeerAborted));
        assert_never_paired(&inviter);
        assert_never_paired(&joiner);
    }
}

#[test]
fn a_short_code_needs_an_invitation_that_showed_one() {
    let mut inv = invitation();
    let short = JoinCode::Short(inv.code().short_code());
    let (inviter, joiner) = to_words(&mut inv, &short);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::ShortCodeNotShown));
    assert_never_paired(&joiner);
}

#[test]
fn out_of_order_messages_abort() {
    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    inviter.handle(&Message::Confirm, T0);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::Unexpected));

    let mut inv = invitation();
    let mut inviter = inv.start(inviter_key().public(), T0).unwrap();
    inviter.handle(&hello_from(&joiner_key(), CodeKind::Qr), T0);
    inviter.handle(&Message::Confirm, T0);
    assert_eq!(inviter.state(), State::Aborted(AbortReason::Unexpected));
}

#[test]
fn qr_round_trip_and_strictness() {
    let inv = invitation();
    let text = inv.code().to_qr();
    assert!(text.starts_with("WASL:"));
    assert!(text[5..].bytes().all(|c| base32::ALPHABET.contains(&c)));
    assert_eq!(PairingCode::from_qr(&text).unwrap(), *inv.code());

    assert!(PairingCode::from_qr(&text.to_lowercase()).is_err());
    assert!(PairingCode::from_qr(&text[5..]).is_err());
    assert!(PairingCode::from_qr(&text[..text.len() - 1]).is_err());
    assert!(PairingCode::from_qr(&format!("{text}0")).is_err());
    for pos in 5..text.len() {
        for &sym in b"0Z" {
            if text.as_bytes()[pos] == sym {
                continue;
            }
            let mut bad = text.clone().into_bytes();
            bad[pos] = sym;
            let bad = String::from_utf8(bad).unwrap();
            assert!(PairingCode::from_qr(&bad).is_err(), "accepted {bad}");
        }
    }

    // No addresses, and the maximum.
    for n in [0, MAX_ADDRS] {
        let code = PairingCode {
            device_id: inviter_key().id(),
            secret: [3; SECRET_LEN],
            addrs: (0..n)
                .map(|i| SocketAddr::from(([10, 0, 0, i as u8], 1000 + i as u16)))
                .collect(),
        };
        assert_eq!(PairingCode::from_qr(&code.to_qr()).unwrap(), code);
    }
}

#[test]
fn short_code_display_and_typing() {
    let code = PairingCode {
        device_id: inviter_key().id(),
        secret: [0; SECRET_LEN],
        addrs: Vec::new(),
    };
    let short = code.short_code();
    assert_eq!(short.to_string(), "0000-0000");
    assert_eq!(ShortCode::parse("oOoO iIlL").unwrap().to_string(), "0000-1111");
    assert_eq!(ShortCode::parse("0000-0000").unwrap(), short);
    for bad in ["", "0000-000", "0000-00000", "0000-000U", "0000_0000"] {
        assert!(ShortCode::parse(bad).is_err(), "{bad}");
    }
    assert!(!format!("{short:?}").contains("0000"));
}

#[test]
fn words_are_symmetric_in_key_order() {
    let a = [1u8; 32];
    let b = [2u8; 32];
    assert_eq!(confirmation_words(b"k", &a, &b), confirmation_words(b"k", &b, &a));
    assert_ne!(confirmation_words(b"k", &a, &b), confirmation_words(b"K", &a, &b));
}
