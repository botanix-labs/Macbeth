use super::*;

fn forward_to_timeout(
    alice: &mut DkgStateMachine,
    bob: &mut DkgStateMachine,
    eve: &mut DkgStateMachine,
    now: &mut Instant,
) -> Duration {
    let t = alice.timeout(*now).unwrap();
    let bt = bob.timeout(*now);
    let et = eve.timeout(*now);

    match (bt, et) {
        (Some(bt), Some(et)) => {
            assert_eq!(t, bt);
            assert_eq!(t, et);
        }
        (Some(bt), None) => {
            assert_eq!(t, bt);
        }
        (None, Some(et)) => {
            assert_eq!(t, et);
        }
        (None, None) => {}
    }

    // Forward time.
    *now += t;

    alice.on_timeout(*now);
    bob.on_timeout(*now);
    eve.on_timeout(*now);

    t
}

#[test]
fn stage_one_resend_round1_packages_on_timeout() {
    let config = test_config();
    let (alice_addr, bob_addr, eve_addr, mut alice, mut bob, mut eve) = setup(config);

    let mut now = Instant::now();

    // *** Alice sends her initial round1 package to Bob and Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            // round1(Alice) -> Bob
            .round1(alice_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob sends his round1 package and an ack to Alice
    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            // round1(Bob) -> Alice
            .round1(bob_addr, alice_addr)
            .msgs();

        // All messages dropped.
        let _ = (b1, b2);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round1_package_timeout);

    // *** Alice resends her round1 packages to Bob and Eve.
    // TODO: Check that it produced the same messages as before.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            // round1(Alice) -> Bob
            .round1(alice_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob resends his round1 package and an ack to Alice
    {
        // TODO: Shouldn't this be flipped?
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // round1(Bob) -> Alice
            .round1(bob_addr, alice_addr)
            // ack(Alice) -> Alice
            .ack_round1(alice_addr, alice_addr)
            .msgs();

        // Alice processes the messages.
        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round1_package_timeout);

    // ** Alice sends an ack to Bob, resends her round1 package, and sends
    // Bobs' forwarded round1 package to Eve.
    {
        let [a1, a2, a3] = CheckedSend::new(&mut alice, now)
            // ack1(Bob) -> Bob
            .ack_round1(bob_addr, bob_addr)
            // round1(Bob) -> Eve (forwarded)
            .round1(bob_addr, eve_addr)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            .msgs();

        // Bob processes the message, Eve drops them.
        bob.recv(a1).unwrap();
        let _ = (a2, a3);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round1_package_timeout);

    // ** Alice resends her and Bobs' forwarded round1 packages to Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round1(Bob) -> Eve (forwarded)
            .round1(bob_addr, eve_addr)
            // round1(Alice) -> Eve
            .round1(alice_addr, eve_addr)
            .msgs();

        let _ = (a1, a2);
    }
}

#[test]
fn stage_two_resend_round2_packages_on_timeout() {
    let config = test_config();
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(config);

    let mut now = Instant::now();

    // Complete stage one.
    let (mut alice, mut bob, mut eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    // *** Alice sends her round2 packages to Bob and Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round2(Alice.Eve) -> Eve
            .round2(alice_addr, eve_addr, eve_addr)
            // round2(Alice.Bob) -> Bob
            .round2(alice_addr, bob_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob sends his round2 packages and an ack to Alice
    {
        let [b1, b2, b3] = CheckedSend::new(&mut bob, now)
            // ack2(Alice.Bob) -> Alice
            .ack_round2(alice_addr, bob_addr, alice_addr)
            // round2(Bob.Eve) -> Alice
            .round2(bob_addr, eve_addr, alice_addr)
            // round2(Bob.Alice) -> Alice
            .round2(bob_addr, alice_addr, alice_addr)
            .msgs();

        // All messages dropped.
        let _ = (b1, b2, b3);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round2_package_timeout);

    // *** Alice resends her round2 packages to Bob and Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round2(Alice.Eve) -> Eve
            .round2(alice_addr, eve_addr, eve_addr)
            // round2(Alice.Bob) -> Bob
            .round2(alice_addr, bob_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob resends his round2 packages and an ack to Alice
    {
        let [b1, b2, b3] = CheckedSend::new(&mut bob, now)
            // round2(Bob.Eve) -> Alice
            .round2(bob_addr, eve_addr, alice_addr)
            // round2(Bob.Alice) -> Alice
            .round2(bob_addr, alice_addr, alice_addr)
            // ack2(Alice.Bob) -> Alice
            .ack_round2(alice_addr, bob_addr, alice_addr)
            .msgs();

        // Alice processes the messages.
        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
        alice.recv(b3).unwrap();
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round2_package_timeout);

    // *** Alice sends two acks to Bob, resends her round2 package to Eve,
    // and forwards Bobs' round2 package to Eve.
    {
        let [a1, a2, a3, a4] = CheckedSend::new(&mut alice, now)
            // ack2(Bob.Eve) -> Bob
            .ack_round2(bob_addr, eve_addr, bob_addr)
            // round2(Bob.Eve) -> Eve (forwarded)
            .round2(bob_addr, eve_addr, eve_addr)
            // ack2 (Bob.Alice) -> Bob
            .ack_round2(bob_addr, alice_addr, bob_addr)
            // round2(Alice.Eve) -> Eve
            .round2(alice_addr, eve_addr, eve_addr)
            .msgs();

        // Bob processes the message, Eve drops them.
        bob.recv(a1).unwrap();
        bob.recv(a3).unwrap();
        let _ = (a2, a4);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round2_package_timeout);

    // *** Alice resends her and Bobs' forwarded round2 packages to Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round2(Bob.Eve) -> Eve (forwarded)
            .round2(bob_addr, eve_addr, eve_addr)
            // round2(Alice.Eve) -> Eve
            .round2(alice_addr, eve_addr, eve_addr)
            .msgs();

        let _ = (a1, a2);
    }
}

#[test]
fn stage_three_resend_round3_packages_on_timeout() {
    let config = test_config();
    let (alice_addr, bob_addr, eve_addr, alice, bob, eve) = setup(config);

    let mut now = Instant::now();

    // Complete stage one.
    let (alice, bob, eve) =
        complete_stage_one(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    // Complete stage two.
    let (mut alice, mut bob, mut eve) =
        complete_stage_two(alice_addr, bob_addr, eve_addr, alice, bob, eve, now);

    // *** Alice sends her initial round3 packages to Bob and Eve.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round3(Alice) -> Eve
            .round3(alice_addr, eve_addr)
            // round3(Alice) -> Bob
            .round3(alice_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob sends his round3 package and an ack to Alice
    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // ack3(Alice) -> Alice
            .ack_round3(alice_addr, alice_addr)
            // round3(Bob) -> Alice
            .round3(bob_addr, alice_addr)
            .msgs();

        // All messages dropped.
        let _ = (b1, b2);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round3_package_timeout);

    // *** Alice resends her round3 packages to Bob and Eve.
    // TODO: Check that it produced the same messages as before.
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // TODO: Why is this reversed?
            // round3(Alice) -> Eve
            .round3(alice_addr, eve_addr)
            // round3(Alice) -> Bob
            .round3(alice_addr, bob_addr)
            .msgs();

        // Bob processes the message, Eve drops it.
        bob.recv(a2).unwrap();
        let _ = a1;
    }

    // *** Bob resends his round3 package and an ack to Alice
    {
        let [b1, b2] = CheckedSend::new(&mut bob, now)
            // round3(Bob) -> Alice
            .round3(bob_addr, alice_addr)
            // ack3(Alice) -> Alice
            .ack_round3(alice_addr, alice_addr)
            .msgs();

        // Alice processes the messages.
        alice.recv(b1).unwrap();
        alice.recv(b2).unwrap();
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round3_package_timeout);

    // *** Alice sends an ack to Bob, resends her round3 package, and sends
    // Bobs' forwarded round3 package to Eve
    {
        let [a1, a2, a3] = CheckedSend::new(&mut alice, now)
            // ack3(Bob) -> Bob
            .ack_round3(bob_addr, bob_addr)
            // round3(Bob) -> Eve (forwarded)
            .round3(bob_addr, eve_addr)
            // round3(Alice) -> Eve
            .round3(alice_addr, eve_addr)
            .msgs();

        // Bob processes the message, Eve drops them.
        bob.recv(a1).unwrap();
        let _ = (a2, a3);
    }

    // Trigger timeout.
    let t = forward_to_timeout(&mut alice, &mut bob, &mut eve, &mut now);
    assert_eq!(t, config.round3_package_timeout);

    // *** Alice resends her and Bobs' forwarded round3 package to Eve
    {
        let [a1, a2] = CheckedSend::new(&mut alice, now)
            // round3(Bob) -> Eve (forwarded)
            .round3(bob_addr, eve_addr)
            // round3(Alice) -> Eve
            .round3(alice_addr, eve_addr)
            .msgs();

        let _ = (a1, a2);
    }
}
