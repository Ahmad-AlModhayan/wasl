use super::*;
use crate::identity::DeviceKey;

const T0: u64 = 1_700_000_000;

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed(&[n; 32])
}

/// A creates, adds B. Returns the group and every operation so far.
fn group_ab() -> (GroupOps, Vec<GroupOp>, DeviceKey, DeviceKey) {
    let (a, b) = (key(1), key(2));
    let (mut g, create) = GroupOps::create(&a, "Laptop", T0).unwrap();
    let add_b = g.add_device(b.public(), "Phone", T0 + 1, &a).unwrap();
    g.insert(add_b.clone()).unwrap();
    (g, vec![create, add_b], a, b)
}

/// A, B, then C added by A.
fn group_abc() -> (GroupOps, Vec<GroupOp>, DeviceKey, DeviceKey, DeviceKey) {
    let (mut g, mut ops, a, b) = group_ab();
    let c = key(3);
    let add_c = g.add_device(c.public(), "Tablet", T0 + 2, &a).unwrap();
    g.insert(add_c.clone()).unwrap();
    ops.push(add_c);
    (g, ops, a, b, c)
}

/// The group a device would build from `base` plus `extra` in this order (errors ignored, as a
/// device receiving junk would).
fn received(base: &[GroupOp], extra: &[&GroupOp]) -> GroupOps {
    let mut g = GroupOps::bootstrap(base, &key(1).public()).unwrap();
    for op in extra {
        let _ = g.insert((*op).clone());
    }
    g
}

fn names(g: &GroupOps) -> Vec<String> {
    let mut n: Vec<String> = g.state().devices().iter().map(|d| d.name.clone()).collect();
    n.sort();
    n
}

/// `g` plus `op` kept without checks: what the signer of `op` sees on its own branch.
fn branch_with(g: &GroupOps, op: &GroupOp) -> GroupOps {
    let mut b = g.clone();
    b.ops.insert(op.id(), op.clone());
    b.recompute();
    b
}

fn permute<'a>(items: &[&'a GroupOp], prefix: &mut Vec<&'a GroupOp>, out: &mut Vec<Vec<&'a GroupOp>>) {
    if prefix.len() == items.len() {
        out.push(prefix.clone());
        return;
    }
    for it in items {
        if !prefix.iter().any(|p| p.id() == it.id()) {
            prefix.push(it);
            permute(items, prefix, out);
            prefix.pop();
        }
    }
}

#[test]
fn create_add_and_remove() {
    let (mut g, _, a, b) = group_ab();
    assert!(g.state().trusts_key(&a.public()));
    assert!(g.state().trusts_key(&b.public()));
    let phone = g.state().device(&b.id()).unwrap();
    assert_eq!(
        (phone.name.as_str(), phone.added_by, phone.added_at),
        ("Phone", a.id(), T0 + 1)
    );

    // B, now a member, may add and remove too.
    let c = key(3);
    g.insert(g.add_device(c.public(), "Tablet", T0 + 2, &b).unwrap())
        .unwrap();
    assert_eq!(3, g.state().devices().len());
    g.insert(g.remove_device(c.id(), &b).unwrap()).unwrap();
    assert!(!g.state().trusts_key(&c.public()));
    assert_eq!(g.state().removed(), &[c.id()]);
    assert_eq!(
        g.add_device(c.public(), "Tablet", T0 + 3, &a),
        Err(GroupError::Revoked(c.id()))
    );
    assert_eq!(
        g.add_device(b.public(), "Again", T0 + 3, &a),
        Err(GroupError::AlreadyMember(b.id()))
    );
}

#[test]
fn a_removed_device_signs_nothing_that_counts() {
    let (mut g, _, a, b) = group_ab();
    let before_removal = g.clone();
    g.insert(g.remove_device(b.id(), &a).unwrap()).unwrap();
    assert!(!g.state().trusts_key(&b.public()));
    assert_eq!(
        g.add_device(key(7).public(), "Friend", T0, &b),
        Err(GroupError::NotMember(b.id()))
    );
    // Built on top of its removal: refused.
    let after = GroupOp::signed(
        Change::Add(GroupDevice {
            id: key(7).id(),
            name: "Friend".into(),
            pk: key(7).public(),
            added_at: T0,
            added_by: b.id(),
        }),
        g.heads(),
        &b,
    );
    assert_eq!(g.insert(after), Err(GroupError::UntrustedSigner(b.id())));
    // Forged on old parents (concurrent with its removal): kept out.
    let forged = before_removal
        .add_device(key(7).public(), "Friend", T0, &b)
        .unwrap();
    assert!(g.insert(forged).is_err());
    assert!(!g.state().trusts_key(&key(7).public()));
}

#[test]
fn a_removed_device_cannot_stay_or_bring_anyone_in_by_racing() {
    // A removes C. C, still a member where it stands, races: it adds Rogue; Rogue adds Rogue 2
    // and removes B. Whatever order the operations arrive in, C and both rogues are out, and B
    // stays (Rogue never counts, so neither does its removal of B).
    let (g3, base, a, b, c) = group_abc();
    let removal = g3.remove_device(c.id(), &a).unwrap();
    let rogue = g3.add_device(key(9).public(), "Rogue", T0 + 3, &c).unwrap();
    let branch = branch_with(&g3, &rogue);
    let r9 = key(9);
    let rogue2 = branch
        .add_device(key(10).public(), "Rogue 2", T0 + 4, &r9)
        .unwrap();
    let kick_b = branch.remove_device(b.id(), &r9).unwrap();

    let ops = [&removal, &rogue, &rogue2, &kick_b];
    let mut orders: Vec<Vec<&GroupOp>> = Vec::new();
    permute(&ops, &mut Vec::new(), &mut orders);
    assert_eq!(24, orders.len());
    for order in &orders {
        let g = received(&base, order);
        assert_eq!(names(&g), ["Laptop", "Phone"], "{order:?}");
        assert_eq!(g.state().removed(), &[c.id()]);
    }
}

#[test]
fn concurrent_additions_both_count_in_any_order() {
    let (g, base, a, b) = group_ab();
    let by_a = g.add_device(key(3).public(), "Tablet", T0 + 2, &a).unwrap();
    let by_b = g.add_device(key(4).public(), "Desktop", T0 + 2, &b).unwrap();
    for order in [[&by_a, &by_b], [&by_b, &by_a]] {
        let g = received(&base, &order);
        assert_eq!(names(&g), ["Desktop", "Laptop", "Phone", "Tablet"]);
        assert_eq!(g.heads().len(), 2);
    }
}

#[test]
fn devices_added_before_a_removal_stay() {
    let (mut g, _, a, _, c) = group_abc();
    let d = key(4);
    g.insert(g.add_device(d.public(), "Desktop", T0 + 3, &c).unwrap())
        .unwrap();
    g.insert(g.remove_device(c.id(), &a).unwrap()).unwrap();
    assert!(g.state().trusts_key(&d.public()));
    assert!(!g.state().trusts_key(&c.public()));
}

#[test]
fn mutual_removal_removes_both() {
    let (g, base, a, _, c) = group_abc();
    let a_removes_c = g.remove_device(c.id(), &a).unwrap();
    let c_removes_a = g.remove_device(a.id(), &c).unwrap();
    for order in [[&a_removes_c, &c_removes_a], [&c_removes_a, &a_removes_c]] {
        let g = received(&base, &order);
        assert_eq!(names(&g), ["Phone"]);
    }
}

#[test]
fn an_honest_addition_survives_a_rogue_removal_that_does_not_count() {
    // A removes C. C's rogue "removes" B concurrently with B adding E. The rogue never counts,
    // so neither does its removal of B, and B's addition stands, in any arrival order.
    let (g3, base, a, b, c) = group_abc();
    let removal = g3.remove_device(c.id(), &a).unwrap();
    let rogue = g3.add_device(key(9).public(), "Rogue", T0, &c).unwrap();
    let kick_b = branch_with(&g3, &rogue).remove_device(b.id(), &key(9)).unwrap();
    let honest = received(&base, &[&removal]);
    let add_e = honest.add_device(key(5).public(), "E", T0 + 5, &b).unwrap();

    let ops = [&removal, &rogue, &kick_b, &add_e];
    let mut orders: Vec<Vec<&GroupOp>> = Vec::new();
    permute(&ops, &mut Vec::new(), &mut orders);
    for order in &orders {
        let g = received(&base, order);
        assert_eq!(names(&g), ["E", "Laptop", "Phone"], "{order:?}");
    }
}

#[test]
fn tampered_operations_are_refused() {
    let (g, _, a, _) = group_ab();
    let good = g.add_device(key(3).public(), "Tablet", T0 + 2, &a).unwrap();

    let mut f = good.clone();
    if let Change::Add(d) = &mut f.change {
        d.name = "Evil".into();
    }
    assert_eq!(g.clone().insert(f), Err(GroupError::BadSignature));

    let mut f = good.clone();
    f.signature[0] ^= 1;
    assert_eq!(g.clone().insert(f), Err(GroupError::BadSignature));

    // An attacker's key under an existing ID: the ID no longer matches the key.
    let mut f = good.clone();
    if let Change::Add(d) = &mut f.change {
        d.pk = key(66).public();
    }
    assert!(matches!(g.clone().insert(f), Err(GroupError::Malformed(_))));

    // Tampering in the JSON text is caught the same way.
    let text = String::from_utf8(good.to_json()).unwrap();
    let f = GroupOp::from_json(text.replace("\"Tablet\"", "\"Tablet2\"").as_bytes()).unwrap();
    assert_eq!(g.clone().insert(f), Err(GroupError::BadSignature));

    let mut ok = g.clone();
    assert!(ok.insert(good.clone()).unwrap());
    assert!(!ok.insert(good).unwrap(), "already kept");
}

#[test]
fn an_outsider_signs_nothing() {
    let (g, _, a, _) = group_ab();
    let outsider = key(50);
    let (own, _) = GroupOps::create(&outsider, "Intruder", T0).unwrap();
    let mut op = own.add_device(key(51).public(), "X", T0, &outsider).unwrap();
    op.parents = g.heads();
    op.signature = outsider.sign(&op.signed_bytes());
    assert_eq!(
        g.clone().insert(op.clone()),
        Err(GroupError::UntrustedSigner(outsider.id()))
    );

    // Claiming a member's ID while signing with another key fails.
    op.signed_by = a.id();
    op.signature = outsider.sign(&op.signed_bytes());
    assert!(g.clone().insert(op).is_err());
    // A second create is never accepted.
    let (_, create) = GroupOps::create(&outsider, "Intruder", T0).unwrap();
    assert_eq!(g.clone().insert(create), Err(GroupError::WrongGenesis));
    assert_eq!(
        g.remove_device(a.id(), &outsider).err(),
        Some(GroupError::NotMember(outsider.id()))
    );
}

#[test]
fn operations_wait_for_their_parents() {
    let (g, base, a, _) = group_ab();
    let mut g2 = g.clone();
    let add_c = g2.add_device(key(3).public(), "C", T0, &a).unwrap();
    g2.insert(add_c.clone()).unwrap();
    let add_d = g2.add_device(key(4).public(), "D", T0, &a).unwrap();

    let g = received(&base, &[&add_d, &add_c]);
    assert_eq!(names(&g), ["C", "D", "Laptop", "Phone"]);
}

#[test]
fn bootstrap_trusts_only_a_member_peer() {
    let (_, ops, a, b) = group_ab();
    assert!(GroupOps::bootstrap(&ops, &a.public()).is_ok());
    assert!(GroupOps::bootstrap(&ops, &b.public()).is_ok());
    assert_eq!(
        GroupOps::bootstrap(&ops, &key(9).public()).err(),
        Some(GroupError::UntrustedSigner(key(9).id()))
    );
    assert_eq!(
        GroupOps::bootstrap(&ops[1..], &a.public()).err(),
        Some(GroupError::WrongGenesis)
    );
    let (_, other) = GroupOps::create(&key(9), "Other", T0).unwrap();
    let mut two = ops.clone();
    two.push(other);
    assert_eq!(
        GroupOps::bootstrap(&two, &a.public()).err(),
        Some(GroupError::WrongGenesis)
    );
}

#[test]
fn a_group_survives_a_round_trip_and_is_checked_again() {
    let (mut g, _, a, b) = group_ab();
    g.insert(g.remove_device(a.id(), &b).unwrap()).unwrap();
    let back = GroupOps::from_json(&g.to_json(), g.genesis()).unwrap();
    assert_eq!(back.state(), g.state());
    assert_eq!(
        names(&back),
        ["Phone"],
        "the creator was removed; the group goes on"
    );

    let text = String::from_utf8(g.to_json()).unwrap().replace("Phone", "Phony");
    assert!(GroupOps::from_json(text.as_bytes(), g.genesis()).is_err());
    assert_eq!(
        GroupOps::from_json(&g.to_json(), OpId([7; 32])).err(),
        Some(GroupError::WrongGenesis)
    );
}

#[test]
fn a_signer_has_a_bounded_share() {
    let (mut g, _, a, _) = group_ab();
    let mut kept = 2; // create and add B
    let err = loop {
        let k = key((kept % 200) as u8 + 20);
        let op = match g.add_device(k.public(), "X", T0, &a) {
            Ok(op) => op,
            Err(_) => match g.remove_device(k.id(), &a) {
                Ok(op) => op,
                Err(_) => {
                    kept += 1;
                    continue;
                }
            },
        };
        match g.insert(op) {
            Ok(_) => kept += 1,
            Err(e) => break e,
        }
        assert!(kept <= MAX_OPS + 1);
    };
    assert_eq!(err, GroupError::Full);
    let by_a = g.ops.values().filter(|o| o.signed_by == a.id()).count();
    assert_eq!(by_a, MAX_OPS_PER_SIGNER);
}

#[test]
fn malformed_operations_are_refused() {
    let (g, _, a, _) = group_ab();
    let good = String::from_utf8(g.add_device(key(3).public(), "T", T0, &a).unwrap().to_json()).unwrap();
    for bad in [
        String::new(),
        "{}".into(),
        good.replacen('{', "{\"extra\":1,", 1),
        good.replace("\"signature\":\"", "\"signature\":\"0"),
    ] {
        assert!(GroupOp::from_json(bad.as_bytes()).is_err(), "{bad}");
    }
    assert_eq!(
        GroupOp::from_json(&vec![b' '; MAX_OP_BYTES + 1]),
        Err(GroupError::TooLarge)
    );

    // Repeated parents, and an add without parents.
    let mut op = GroupOp::from_json(good.as_bytes()).unwrap();
    op.parents = vec![op.parents[0], op.parents[0]];
    assert!(matches!(op.validate(), Err(GroupError::Malformed(_))));
    op.parents.clear();
    assert!(matches!(op.validate(), Err(GroupError::Malformed(_))));
}

#[test]
fn canonical_json_is_stable_across_key_order_and_whitespace() {
    let (g, _, a, _) = group_ab();
    let op = g.add_device(key(3).public(), "Tablet", T0 + 2, &a).unwrap();
    let canonical = String::from_utf8(op.to_json()).unwrap();
    let value: Value = serde_json::from_str(&canonical).unwrap();
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    let reparsed = GroupOp::from_json(pretty.as_bytes()).unwrap();
    assert_eq!(reparsed.to_json(), op.to_json());
    assert_eq!(reparsed.id(), op.id());
}

#[test]
fn canonical_json_sorts_by_bytes() {
    let v: Value = serde_json::json!({"b": 1, "a": {"z": [1, {"y": 2, "x": 3}], "Z": null}, "ä": true});
    assert_eq!(
        canonical_json(&v),
        r#"{"a":{"Z":null,"z":[1,{"x":3,"y":2}]},"b":1,"ä":true}"#
    );
}

#[test]
fn a_racing_removal_saves_and_loads_in_any_order() {
    // Rogue's addition counts until C's removal arrives; saved after both, the group must load
    // (the rogue operation is kept but does not count) and give the same members.
    let (g3, base, a, _, c) = group_abc();
    let removal = g3.remove_device(c.id(), &a).unwrap();
    let rogue = g3.add_device(key(9).public(), "Rogue", T0, &c).unwrap();
    for order in [[&rogue, &removal], [&removal, &rogue]] {
        let g = received(&base, &order);
        let back = GroupOps::from_json(&g.to_json(), g.genesis()).unwrap();
        assert_eq!(back.state(), g.state());
        assert_eq!(names(&back), ["Laptop", "Phone"]);
    }
}

fn orphan(n: u8, k: &DeviceKey) -> GroupOp {
    GroupOp::signed(Change::Remove(key(n).id()), vec![OpId([n; 32])], k)
}

#[test]
fn waiting_operations_are_bounded_per_signer_and_never_block_newcomers() {
    let (mut g, _, a, b) = group_ab();
    for n in 0..MAX_PENDING_PER_SIGNER as u8 {
        assert!(g.insert(orphan(n + 100, &a)).unwrap());
    }
    assert_eq!(g.insert(orphan(200, &a)), Err(GroupError::Full));
    // A bad signature from a known signer is refused before waiting.
    let mut forged = orphan(201, &b);
    forged.signature[0] ^= 1;
    assert_eq!(g.insert(forged), Err(GroupError::BadSignature));
    // Unknown signers wait in their own small place: they never push out a member's operation.
    assert!(g.insert(orphan(202, &b)).unwrap());
    for n in 0..(MAX_PENDING_UNKNOWN as u16 + 10) {
        let mut seed = [7u8; 32];
        seed[0] = (n % 251) as u8;
        seed[1] = (n / 251) as u8;
        assert!(g.insert(orphan(1, &DeviceKey::from_seed(&seed))).unwrap());
    }
    let unknown = g
        .pending
        .iter()
        .filter(|p| g.state().device(&p.signed_by).is_none())
        .count();
    assert_eq!(unknown, MAX_PENDING_UNKNOWN);
    assert_eq!(
        g.pending.iter().filter(|p| p.signed_by == a.id()).count(),
        MAX_PENDING_PER_SIGNER
    );
    assert!(g.pending.iter().any(|p| p.signed_by == b.id()));
}

#[test]
fn a_removal_carries_the_history_of_its_target_when_heads_are_many() {
    // More concurrent heads than an operation may name: the removal still names one whose past
    // adds the target, so it counts.
    let (g, _, a, _) = group_ab();
    let mut wide = g.clone();
    for n in 0..(MAX_PARENTS as u8 + 4) {
        wide.insert(g.add_device(key(n + 30).public(), "W", T0, &a).unwrap())
            .unwrap();
    }
    assert!(wide.heads().len() > MAX_PARENTS);
    // The target is the device added by the largest head ID: the one plain truncation drops.
    let last = *wide.heads().last().unwrap();
    let Change::Add(target) = wide.ops[&last].change.clone() else {
        panic!("heads are additions")
    };
    let removal = wide.remove_device(target.id, &a).unwrap();
    assert!(removal.parents.len() <= MAX_PARENTS);
    assert!(removal.parents.contains(&last));
    wide.insert(removal).unwrap();
    assert!(wide.state().device(&target.id).is_none());
    assert!(wide.state().removed().contains(&target.id));
}

#[test]
fn a_removed_device_waits_only_in_the_small_place() {
    let (mut g, _, a, b) = group_ab();
    g.insert(g.remove_device(b.id(), &a).unwrap()).unwrap();
    for n in 0..(MAX_PENDING_UNKNOWN as u8 + 5) {
        assert!(g.insert(orphan(n + 40, &b)).unwrap());
    }
    assert_eq!(MAX_PENDING_UNKNOWN, g.pending.len());
    for n in 0..MAX_PENDING_PER_SIGNER as u8 {
        assert!(g.insert(orphan(n + 100, &a)).unwrap(), "members still wait");
    }
}

#[test]
fn a_member_is_sent_only_the_operations_it_lacks() {
    let (g, ops, a, b, c) = group_abc();
    // B knows the create and its own addition: it lacks only C's.
    let on_b = GroupOps::bootstrap(&ops[..2], &a.public()).unwrap();
    let missing = g.missing_for(&on_b.heads());
    assert_eq!(missing, vec![ops[2].clone()]);
    // Up to date: nothing; heads unknown here (the other device's news) send everything.
    assert!(g.missing_for(&g.heads()).is_empty());
    assert_eq!(g.missing_for(&[OpId([9; 32])]).len(), ops.len());

    // A removal reaches B the same way, and B stops trusting C.
    let mut on_a = g.clone();
    let rm = on_a.remove_device(c.id(), &a).unwrap();
    on_a.insert(rm.clone()).unwrap();
    let mut on_b = GroupOps::bootstrap(&ops, &a.public()).unwrap();
    let missing = on_a.missing_for(&on_b.heads());
    assert_eq!(missing, vec![rm]);
    for op in missing {
        on_b.insert(op).unwrap();
    }
    assert!(!on_b.state().trusts_key(&c.public()));
    assert!(on_b.state().trusts_key(&b.public()));
}

#[test]
fn an_offline_removal_still_counts_when_its_signer_is_removed_meanwhile() {
    // S2-2, the rule kept: A, offline, removes C; meanwhile B removes A. When the two meet, both
    // removals count (neither device is trusted to stay), in any order. Voiding A's removal
    // because A was being removed would leave C, a device the person removed, trusted.
    let (g3, base, a, b, c) = group_abc();
    let a_removes_c = g3.remove_device(c.id(), &a).unwrap();
    let b_removes_a = g3.remove_device(a.id(), &b).unwrap();
    for order in [[&a_removes_c, &b_removes_a], [&b_removes_a, &a_removes_c]] {
        let g = received(&base, &order);
        assert_eq!(names(&g), ["Phone"], "{order:?}");
    }
}

#[test]
fn a_removed_device_can_act_only_on_devices_that_have_not_heard() {
    // S2-2: C, removed by A, forges a removal of B on parents from before its removal. A device
    // that already holds C's removal never asks C (a non-member) for anything, so C's forgery
    // reaches it only through a member that took it before it heard; then it counts everywhere,
    // as the group's rule says (fail closed: B re-pairs, C never stays).
    let (g3, base, a, b, c) = group_abc();
    let removal = g3.remove_device(c.id(), &a).unwrap();
    let forged = g3.remove_device(b.id(), &c).unwrap();
    let mut heard = received(&base, &[&removal]);
    assert!(heard.state().device(&c.id()).is_none());
    // Same verdict whichever arrives first: deterministic.
    for order in [[&removal, &forged], [&forged, &removal]] {
        assert_eq!(names(&received(&base, &order)), ["Laptop"], "{order:?}");
    }
    assert!(heard.insert(forged).unwrap());
    assert!(heard.state().device(&c.id()).is_none(), "C never comes back");
}

#[test]
fn a_signer_is_throttled_per_minute() {
    let (a, b) = (key(1).id(), key(2).id());
    let mut t = SignerThrottle::default();
    for _ in 0..OPS_PER_SIGNER_PER_MINUTE {
        assert!(t.allow(a, T0));
    }
    assert!(!t.allow(a, T0 + 30), "over its share for this minute");
    assert!(t.allow(b, T0 + 30), "another signer has its own share");
    assert!(t.allow(a, T0 + 61), "a minute later it may go on");
}

#[test]
fn the_throttle_forgets_old_signers_and_stays_bounded() {
    let mut t = SignerThrottle::default();
    for n in 0..(THROTTLE_SIGNERS + 50) {
        let mut seed = [5u8; 32];
        seed[0] = (n % 251) as u8;
        seed[1] = (n / 251) as u8;
        assert!(t.allow(DeviceKey::from_seed(&seed).id(), T0 + n as u64 / 100));
    }
    assert!(t.taken.len() <= THROTTLE_SIGNERS);
}

#[test]
fn operations_are_screened_before_anyone_is_charged() {
    // Review of S2–S5: the throttle used to charge the claimed signer before the signature was
    // checked, so anyone able to write a group file could use up a member's share in its name.
    let (mut g, _, a, b, c) = group_abc();
    let real = g.remove_device(c.id(), &a).unwrap();
    assert_eq!(Screened::Signed(a.id()), g.screen(&real));
    // Claims A, signed by someone else: refused, charged to no one.
    let mut forged = real.clone();
    forged.signature = b.sign(&forged.signed_bytes());
    assert_eq!(Screened::Refused, g.screen(&forged));
    // Malformed, or a second create: refused too.
    let mut bad = real.clone();
    bad.parents.clear();
    assert_eq!(Screened::Refused, g.screen(&bad));
    // A signer this group has never heard of: the shared bucket.
    let stranger = key(40);
    let (other, _) = GroupOps::create(&stranger, "Elsewhere", T0).unwrap();
    let unknown = other.add_device(key(41).public(), "X", T0, &stranger).unwrap();
    assert_eq!(Screened::Unknown, g.screen(&unknown));
    // Kept, or waiting for its parents: costs nothing when it comes again.
    assert!(g.insert(real.clone()).unwrap());
    assert_eq!(Screened::Held, g.screen(&real));
    let mut ahead = g.clone();
    let later = ahead.add_device(key(42).public(), "Later", T0 + 9, &a).unwrap();
    ahead.insert(later).unwrap();
    let after = ahead.add_device(key(43).public(), "After", T0 + 10, &a).unwrap();
    assert!(g.insert(after.clone()).unwrap(), "waits for its parent");
    assert!(!g.contains(&after.id()));
    assert_eq!(Screened::Held, g.screen(&after));
    // A removed device is still known by its key: its operations are its own to pay for, and
    // forgeries in its name are refused.
    let by_c = GroupOp::signed(Change::Remove(b.id()), g.heads(), &c);
    assert_eq!(Screened::Signed(c.id()), g.screen(&by_c));
}

#[test]
fn unknown_signers_share_one_share() {
    let mut t = SignerThrottle::default();
    for _ in 0..OPS_PER_SIGNER_PER_MINUTE {
        assert!(t.allow_unknown(T0));
    }
    assert!(!t.allow_unknown(T0 + 30));
    assert!(t.allow(key(1).id(), T0 + 30), "members keep their own shares");
    assert!(t.allow_unknown(T0 + 61));
}
