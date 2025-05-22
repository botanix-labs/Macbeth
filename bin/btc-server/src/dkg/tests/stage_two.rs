use super::*;
use crate::dkg::Stage;

pub fn complete_stage_two(
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
    assert_eq!(alice.stage(), Stage::RoundTwo);
    assert_eq!(bob.stage(), Stage::RoundTwo);
    assert_eq!(eve.stage(), Stage::RoundTwo);

    // Bob and Eve are waiting for Alice to send the initial message.
    assert!(bob.send(now).is_none());
    assert!(eve.send(now).is_none());

    assert!(alice.timeout(now).is_some());
    assert!(bob.timeout(now).is_none());
    assert!(eve.timeout(now).is_none());

    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round2(Alice.Eve) -> Eve
            .round2(alice_addr, eve_addr, eve_addr)
            // round2(Alice.Bob) -> Bob
            .round2(alice_addr, bob_addr, bob_addr)
            .msgs();

        eve.recv(a1).unwrap();
        bob.recv(a2).unwrap();
    }

    assert!(alice.timeout(now).is_some());
    assert!(bob.timeout(now).is_none());
    assert!(eve.timeout(now).is_none());

    {
        let [b1, b2, b3] = CheckedSend::new(&mut bob, now)
            // ack2(Alice.Bob) -> Alice
            .ack_round2(alice_addr, bob_addr, alice_addr)
            // round2(Bob.Eve) -> Alice
            .round2(bob_addr, eve_addr, alice_addr)
            // round2(Bob.Alice) -> Alice
            .round2(bob_addr, alice_addr, alice_addr)
            .msgs();

        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
        alice.recv(b3).unwrap();
    }

    {
        let [e1, e2, e3] = CheckedSend::new(&mut eve, now)
            // ack2(Alice.Eve) -> Alice
            .ack_round2(alice_addr, eve_addr, alice_addr)
            // round2(Eve.Bob) -> Alice
            .round2(eve_addr, bob_addr, alice_addr)
            // round2(Eve.Alice) -> Alice
            .round2(eve_addr, alice_addr, alice_addr)
            .msgs();

        alice.recv(e1).unwrap();
        alice.recv(e2).unwrap();
        alice.recv(e3).unwrap();
    }

    assert!(alice.timeout(now).is_some());
    assert!(bob.timeout(now).is_some());
    assert!(eve.timeout(now).is_some());

    {
        let [a1, a2, a3, a4, a5, a6] = CheckedSend::new(&mut alice, now)
            // ack2(Bob.Eve) -> Bob
            .ack_round2(bob_addr, eve_addr, bob_addr)
            // round2(Bob.Eve) -> Eve (forwarded)
            .round2(bob_addr, eve_addr, eve_addr)
            // ack2 (Bob.Alice) -> Bob
            .ack_round2(bob_addr, alice_addr, bob_addr)
            // ack2 (Eve.Bob) -> Eve
            .ack_round2(eve_addr, bob_addr, eve_addr)
            // round2 (Eve.Bob) -> Bob (forwarded)
            .round2(eve_addr, bob_addr, bob_addr)
            // ack2 (Eve.Alice) -> Eve
            .ack_round2(eve_addr, alice_addr, eve_addr)
            .msgs();

        bob.recv(a1).unwrap();
        bob.recv(a3).unwrap();
        bob.recv(a5).unwrap();

        eve.recv(a2).unwrap();
        eve.recv(a4).unwrap();
        eve.recv(a6).unwrap();
    }

    {
        let [b1] = CheckedSend::new(&mut bob, now)
            // ack2(Eve.Bob) -> Alice
            .ack_round2(eve_addr, bob_addr, alice_addr)
            .msgs();

        alice.recv(b1).unwrap();
    }

    {
        let [e1] = CheckedSend::new(&mut eve, now)
            // ack2(Bob.Eve) -> Alice
            .ack_round2(bob_addr, eve_addr, alice_addr)
            .msgs();

        alice.recv(e1).unwrap();
    }

    assert_eq!(alice.stage(), Stage::RoundThree);
    assert_eq!(bob.stage(), Stage::RoundThree);
    assert_eq!(eve.stage(), Stage::RoundThree);

    (alice, bob, eve)
}

#[test]
fn dkg_complete_stage_two() {
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(test_config());

    let now = Instant::now();

    let (alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    let (_alice, _bob, _eve) =
        complete_stage_two(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);
}
