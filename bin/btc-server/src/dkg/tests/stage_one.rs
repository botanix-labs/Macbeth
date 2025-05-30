use super::*;
use crate::dkg::Stage;

pub fn complete_stage_one(
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
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            // round1(Alice) -> Bob
            .round1(alice_addr, bob_addr)
            .msgs();

        eve.recv(a1).unwrap();
        bob.recv(a2).unwrap();
    }

    assert_eq!(alice.stage(), Stage::RoundOne);
    assert_eq!(bob.stage(), Stage::RoundOne);
    assert_eq!(eve.stage(), Stage::RoundOne);

    assert!(alice.timeout(now).is_some());
    assert!(bob.timeout(now).is_none());
    assert!(eve.timeout(now).is_none());

    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Bob) -> Alice
            .round1(bob_addr, alice_addr)
            .msgs();

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

        alice.recv(e1).unwrap();
        alice.recv(e2).unwrap();
    }

    assert!(alice.timeout(now).is_some());
    assert!(bob.timeout(now).is_some());
    assert!(eve.timeout(now).is_some());

    {
        let [a1, a2, a3, a4] = CheckedSend::new(&mut alice, now)
            // ack(Bob) -> Bob
            .ack_round1(bob_addr, bob_addr)
            // round1(Bob) -> Eve (forwarded)
            .round1(bob_addr, eve_addr)
            // ack(Eve) -> Eve
            .ack_round1(eve_addr, eve_addr)
            // round1(Eve) -> Bob (forwarded)
            .round1(eve_addr, bob_addr)
            .msgs();

        bob.recv(a1).unwrap();
        eve.recv(a2).unwrap();
        eve.recv(a3).unwrap();
        bob.recv(a4).unwrap();
    }

    {
        let [b1] = CheckedSend::new(&mut bob, now)
            // ack(Eve) -> Alice
            .ack_round1(eve_addr, alice_addr)
            .msgs();

        alice.recv(b1).unwrap();
    }

    {
        let [e1] = CheckedSend::new(&mut eve, now)
            // ack(Bob) -> Alice
            .ack_round1(bob_addr, alice_addr)
            .msgs();

        alice.recv(e1).unwrap();
    }

    assert_eq!(alice.stage(), Stage::RoundTwo);
    assert_eq!(bob.stage(), Stage::RoundTwo);
    assert_eq!(eve.stage(), Stage::RoundTwo);

    (alice, bob, eve)
}

#[test]
fn dkg_complete_stage_one() {
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(test_config());

    assert_eq!(alice.stage(), Stage::RoundOne);
    assert_eq!(bob.stage(), Stage::AwaitingInit);
    assert_eq!(eve.stage(), Stage::AwaitingInit);

    let now = Instant::now();

    let (_alice, _bob, _eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);
}
