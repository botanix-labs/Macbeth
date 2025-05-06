use super::*;
use crate::dkg::Stage;

pub fn complete_stage_three(
    alice_addr: frost::Identifier,
    bob_addr: frost::Identifier,
    eve_addr: frost::Identifier,
    //
    mut alice: DkgStateMachine,
    mut bob: DkgStateMachine,
    mut eve: DkgStateMachine,
    //
    now: Instant,
) -> (DkgStateMachine, DkgStateMachine, DkgStateMachine) {
    // Bob and Eve are waiting for Alice to send the initial message.
    assert!(bob.send(now).is_none());
    assert!(eve.send(now).is_none());

    assert!(alice.timeout(now).is_none());
    assert!(bob.timeout(now).is_none());
    assert!(eve.timeout(now).is_none());

    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round3(Alice) -> Eve
            .round3(alice_addr, eve_addr)
            // round3(Alice) -> Bob
            .round3(alice_addr, bob_addr)
            .msgs();

        eve.recv(a1).unwrap();
        bob.recv(a2).unwrap();
    }

    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack3(Alice) -> Alice
            .ack_round3(alice_addr, alice_addr)
            // round3(Bob) -> Alice
            .round3(bob_addr, alice_addr)
            .msgs();

        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
    }

    {
        let [e1, e2] = CheckedSend::new(&mut eve, now)
            // ack3(Alice) -> Alice
            .ack_round3(alice_addr, alice_addr)
            // round3(Eve) -> Alice
            .round3(eve_addr, alice_addr)
            .msgs();

        alice.recv(e1).unwrap();
        alice.recv(e2).unwrap();
    }

    {
        let [a1, a2, a3, a4] = CheckedSend::new(&mut alice, now)
            // ack3(Bob) -> Bob
            .ack_round3(bob_addr, bob_addr)
            // round3(Bob) -> Eve (forwarded)
            .round3(bob_addr, eve_addr)
            // ack3(Eve) -> Eve
            .ack_round3(eve_addr, eve_addr)
            // round3(Eve) -> Bob (forwarded)
            .round3(eve_addr, bob_addr)
            .msgs();

        bob.recv(a1).unwrap();
        eve.recv(a2).unwrap();
        eve.recv(a3).unwrap();
        bob.recv(a4).unwrap();
    }

    {
        let [b1] = CheckedSend::new(&mut bob, now)
            // ack3(Eve) -> Alice
            .ack_round3(eve_addr, alice_addr)
            .msgs();

        alice.recv(b1).unwrap();
    }

    {
        let [e1] = CheckedSend::new(&mut eve, now)
            // ack3(Bob) -> Alice
            .ack_round3(bob_addr, alice_addr)
            .msgs();

        alice.recv(e1).unwrap();
    }

    // TODO: Check that no other messages are sent.

    assert_eq!(alice.stage(), Stage::Finalized);
    assert_eq!(bob.stage(), Stage::Finalized);
    assert_eq!(eve.stage(), Stage::Finalized);

    // All member reproduced the same public key!
    let (_, alice_pub) = alice.aggregate_key_packages().unwrap();
    let (_, bob_pub) = bob.aggregate_key_packages().unwrap();
    let (_, eve_pub) = eve.aggregate_key_packages().unwrap();

    assert_eq!(alice_pub, bob_pub);
    assert_eq!(alice_pub, eve_pub);
    assert_eq!(bob_pub, eve_pub);

    (alice, bob, eve)
}

#[test]
fn test_complete_stage_three() {
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup();

    let now = Instant::now();

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
