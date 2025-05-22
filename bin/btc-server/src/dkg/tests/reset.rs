use super::*;
use crate::dkg::{tests::stage_three::complete_stage_three, Stage};

#[test]
fn dkg_session_reset_stage_one() {
    let config = test_config();
    let pending_session_timeout = config.pending_session_timeout.unwrap();
    let (alice_addr, bob_addr, eve_addr, mut alice, mut bob, mut eve) = setup(config);

    let mut now = Instant::now();

    assert_eq!(alice.stage(), Stage::RoundOne);
    assert_eq!(bob.stage(), Stage::AwaitingInit);
    assert_eq!(eve.stage(), Stage::AwaitingInit);

    // Partially complete round one, with session nonce `0`.

    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            // round1(Alice) -> Bob
            .round1(alice_addr, bob_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = a1.msg else { panic!() };
        assert_eq!(nonce, 0);
        let DkgMessage::Round1 { nonce, .. } = a2.msg else { panic!() };
        assert_eq!(nonce, 0);

        eve.recv(a1).unwrap();
        bob.recv(a2).unwrap();
    }

    assert_eq!(alice.stage(), Stage::RoundOne);
    assert_eq!(bob.stage(), Stage::RoundOne);
    assert_eq!(eve.stage(), Stage::RoundOne);

    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Bob) -> Alice
            .round1(bob_addr, alice_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = b2.msg else { panic!() };
        assert_eq!(nonce, 0);

        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
    }

    {
        let [e1, e2] = CheckedSend::new(&mut eve, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Eve) -> Alice
            .round1(eve_addr, alice_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = e2.msg else { panic!() };
        assert_eq!(nonce, 0);

        alice.recv(e1).unwrap();
        alice.recv(e2).unwrap();
    }

    // Trigger session reset timeout.
    now += pending_session_timeout;
    alice.on_timeout(now);

    // (Re-)start a new session from scratch, with session nonce `1`.

    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            // round1(Alice) -> Bob
            .round1(alice_addr, bob_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = a1.msg else { panic!() };
        assert_eq!(nonce, 1);

        let DkgMessage::Round1 { nonce, .. } = a2.msg else { panic!() };
        assert_eq!(nonce, 1);

        eve.recv(a1).unwrap();
        bob.recv(a2).unwrap();
    }

    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Bob) -> Alice
            .round1(bob_addr, alice_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = b2.msg else { panic!() };
        assert_eq!(nonce, 1);

        let _ = (b1, b2);
    }

    {
        let [e1, e2] = CheckedSend::new(&mut eve, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Eve) -> Alice
            .round1(eve_addr, alice_addr)
            .msgs();

        let DkgMessage::Round1 { nonce, .. } = e2.msg else { panic!() };
        assert_eq!(nonce, 1);

        let _ = (e1, e2);
    }
}

#[test]
fn dkg_session_reset_stage_two() {
    let config = test_config();
    let pending_session_timeout = config.pending_session_timeout.unwrap();
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(config);

    let mut now = Instant::now();

    // Complete round one.
    let (mut alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    assert_eq!(alice.stage(), Stage::RoundTwo);
    assert_eq!(bob.stage(), Stage::RoundTwo);
    assert_eq!(eve.stage(), Stage::RoundTwo);

    // Trigger session reset timeout.
    now += pending_session_timeout;
    alice.on_timeout(now);

    assert_eq!(alice.stage(), Stage::RoundOne); // Alice reset.
    assert_eq!(bob.stage(), Stage::RoundTwo);
    assert_eq!(eve.stage(), Stage::RoundTwo);

    // All rounds can be completed again, starting from round one.
    let (alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (alice, bob, eve) =
        complete_stage_two(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (alice, bob, eve) =
        complete_stage_three(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (_sec, alice_aggr) = alice.aggregate_key_packages().unwrap();
    let (_sec, bob_aggr) = bob.aggregate_key_packages().unwrap();
    let (_sec, eve_aggr) = eve.aggregate_key_packages().unwrap();

    assert_eq!(alice_aggr, bob_aggr);
    assert_eq!(alice_aggr, eve_aggr);
    assert_eq!(bob_aggr, eve_aggr);
}

#[test]
fn dkg_session_reset_stage_three() {
    let config = test_config();
    let pending_session_timeout = config.pending_session_timeout.unwrap();
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(config);

    let mut now = Instant::now();

    // Complete round one and round two.
    let (alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (mut alice, bob, eve) =
        complete_stage_two(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    assert_eq!(alice.stage(), Stage::RoundThree);
    assert_eq!(bob.stage(), Stage::RoundThree);
    assert_eq!(eve.stage(), Stage::RoundThree);

    // Trigger session reset timeout.
    now += pending_session_timeout;
    alice.on_timeout(now);

    assert_eq!(alice.stage(), Stage::RoundOne); // Alice reset.
    assert_eq!(bob.stage(), Stage::RoundThree);
    assert_eq!(eve.stage(), Stage::RoundThree);

    // All rounds can be completed again, starting from round one.
    let (alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (alice, bob, eve) =
        complete_stage_two(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (alice, bob, eve) =
        complete_stage_three(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (_sec, alice_aggr) = alice.aggregate_key_packages().unwrap();
    let (_sec, bob_aggr) = bob.aggregate_key_packages().unwrap();
    let (_sec, eve_aggr) = eve.aggregate_key_packages().unwrap();

    assert_eq!(alice_aggr, bob_aggr);
    assert_eq!(alice_aggr, eve_aggr);
    assert_eq!(bob_aggr, eve_aggr);
}
