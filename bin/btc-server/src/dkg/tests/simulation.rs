use super::*;
use crate::dkg::Stage;
use bitcoin::secp256k1;
use frost::keys::PublicKeyPackage;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use std::collections::BTreeMap;
use tokio::{sync::mpsc, time::timeout};

#[tokio::test]
#[ignore]
async fn stage_simulate_random_drops_with_3_members() {
    const NUM_MEMBERS: u16 = 3;

    let config = Config {
        max_signers: 3,
        min_signers: 3,
        round1_package_timeout: Duration::from_millis(100),
        round2_package_timeout: Duration::from_millis(100),
        round3_package_timeout: Duration::from_millis(100),
        pending_session_timeout: None,
    };

    setup_tasks(NUM_MEMBERS, config, &[]).await;
}

#[tokio::test]
#[ignore]
async fn stage_simulate_random_drops_with_one_absent_member() {
    const NUM_MEMBERS: u16 = 3;

    let config = Config {
        max_signers: 3,
        min_signers: 3,
        round1_package_timeout: Duration::from_millis(100),
        round2_package_timeout: Duration::from_millis(100),
        round3_package_timeout: Duration::from_millis(100),
        // Session resets after 10 seconds.
        pending_session_timeout: Some(Duration::from_secs(10)),
    };

    // Id number 2 is absent, the DKG process never completes.
    setup_tasks(NUM_MEMBERS, config, &[2]).await;
}

#[tokio::test]
#[ignore]
async fn stage_simulate_random_drops_with_16_members() {
    const NUM_MEMBERS: u16 = 16;

    let config = Config {
        max_signers: 16,
        min_signers: 16,
        round1_package_timeout: Duration::from_millis(500),
        round2_package_timeout: Duration::from_millis(500),
        round3_package_timeout: Duration::from_millis(500),
        pending_session_timeout: None,
    };

    setup_tasks(NUM_MEMBERS, config, &[]).await;
}

async fn setup_tasks(num_members: u16, config: Config, absent_nodes: &[u16]) {
    debug_assert!(config.min_signers <= config.max_signers);
    debug_assert!(config.min_signers <= num_members);
    debug_assert!(config.max_signers == num_members);

    let coordinator = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();

    // List of all Ids.
    let mut members = BTreeMap::new();
    // List of all sender channels (lookups).
    let mut channels = BTreeMap::new();
    // List of all receiver channels.
    let mut queues = BTreeMap::new();

    let absent_nodes: Vec<frost::Identifier> = absent_nodes
        .iter()
        .map(|id| frost::Identifier::derive(id.to_le_bytes().as_slice()).unwrap())
        .collect();

    for num in 0..num_members {
        let id = frost::Identifier::derive(num.to_le_bytes().as_slice()).unwrap();
        let (tx, rx) = mpsc::channel(100);

        let secp = secp256k1::Secp256k1::new();
        let static_sec = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let static_pub = secp256k1::PublicKey::from_secret_key(&secp, &static_sec);

        members.insert(id, static_pub);
        channels.insert(id, tx);
        queues.insert(id, (static_sec, rx));
    }

    let (callback_tx, mut callback_rx) = mpsc::channel(10);

    // Spawn a task for each member.
    for (id, (static_sec, rx)) in queues {
        if absent_nodes.contains(&id) {
            println!("WARNING: Member {} is absent", name_addr(&id));
            continue;
        }

        let machine =
            DkgStateMachine::new(id, static_sec, coordinator, members.clone(), config, Some(0))
                .unwrap();

        tokio::spawn(run_dkg(machine, rx, channels.clone(), callback_tx.clone()));
    }

    // Wait for all members to finish with their aggregated public key packages.
    let mut pkps = vec![];
    for _ in 0..num_members {
        let pkp = callback_rx.recv().await.unwrap();
        pkps.push(pkp);
    }

    assert_eq!(pkps.len(), num_members as usize);

    // Assert that all final public key packages are the same.
    let first = pkps[0].clone();
    for pkp in &pkps[1..] {
        assert_eq!(&first, pkp);
    }
}

async fn run_dkg(
    mut machine: DkgStateMachine,
    // Payload receiver.
    mut rx: mpsc::Receiver<DkgPayload>,
    // Channel lookup table.
    channels: BTreeMap<frost::Identifier, mpsc::Sender<DkgPayload>>,
    // Callback to send the final public key package.
    callback: mpsc::Sender<PublicKeyPackage>,
) {
    // Set current name with the current stage.
    let mut my_name = format!(
        "{name}({stage})",
        name = name_addr(&machine.frost_id()),
        stage = name_stage(machine.stage()),
    );

    // Trigger send events immediately; only the coordinator have something to
    // send.
    let mut timer = Duration::from_secs(0);
    let mut dkg_completed = false;

    loop {
        // Listen for incoming messages...
        match timeout(timer, rx.recv()).await {
            Ok(Some(payload)) => {
                assert_eq!(machine.frost_id(), payload.recipient);

                // 33% chance that the payload is dropped.
                let rng = SmallRng::from_entropy().gen_range(0..3);
                if rng == 0 {
                    println!(
                        "{my_name}: Drop message {ty} <- {sender}!",
                        ty = name_payload(&payload.msg),
                        sender = name_addr(&payload.sender),
                    );

                    continue;
                }

                println!(
                    "{my_name}: Recv message {ty} <- {sender}",
                    ty = name_payload(&payload.msg),
                    sender = name_addr(&payload.sender),
                );

                // MACHINE: Receive the payload.
                machine.recv(payload).unwrap();

                // Update the name with the current stage.
                my_name = format!(
                    "{name}({stage})",
                    name = name_addr(&machine.frost_id()),
                    stage = name_stage(machine.stage()),
                );
            }
            Ok(None) => {
                panic!("Channel closed unexpectedly");
            }
            Err(_) => {
                // MACHINE: Trigger timeout events.
                let now = Instant::now();
                machine.on_timeout(now);
            }
        };

        let now = Instant::now();
        while let Some(payload) = machine.send(now) {
            println!(
                "{my_name}: Send message {ty} -> {recipient}",
                recipient = name_addr(&payload.recipient),
                ty = name_payload(&payload.msg)
            );

            let tx = channels.get(&payload.recipient).unwrap();
            let _ = tx.try_send(payload);
        }

        // MACHINE: Retrieve next timeout event.
        let now = Instant::now();
        if let Some(trigger) = machine.timeout(now) {
            timer = trigger;
        } else {
            timer = Duration::from_secs(u64::MAX);
        }

        // MACHINE: Retrieve final, aggregated public key.
        if !dkg_completed {
            if let Some((_, pkp)) = machine.aggregate_key_packages() {
                println!("{my_name}: DKG FINALIZED!");
                callback.send(pkp.clone()).await.unwrap();
                dkg_completed = true;
            }
        }
    }
}

fn name_addr(id: &frost::Identifier) -> String {
    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let charlie_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();
    let dave_addr = frost::Identifier::derive(3u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(4u16.to_le_bytes().as_slice()).unwrap();
    let frank_addr = frost::Identifier::derive(5u16.to_le_bytes().as_slice()).unwrap();
    let grace_addr = frost::Identifier::derive(6u16.to_le_bytes().as_slice()).unwrap();
    let heidi_addr = frost::Identifier::derive(7u16.to_le_bytes().as_slice()).unwrap();
    let ivan_addr = frost::Identifier::derive(8u16.to_le_bytes().as_slice()).unwrap();
    let jane_addr = frost::Identifier::derive(9u16.to_le_bytes().as_slice()).unwrap();
    let kevin_addr = frost::Identifier::derive(10u16.to_le_bytes().as_slice()).unwrap();
    let lisa_addr = frost::Identifier::derive(11u16.to_le_bytes().as_slice()).unwrap();
    let mike_addr = frost::Identifier::derive(12u16.to_le_bytes().as_slice()).unwrap();
    let nancy_addr = frost::Identifier::derive(13u16.to_le_bytes().as_slice()).unwrap();
    let oscar_addr = frost::Identifier::derive(14u16.to_le_bytes().as_slice()).unwrap();
    let peggy_addr = frost::Identifier::derive(15u16.to_le_bytes().as_slice()).unwrap();

    match *id {
        id if id == alice_addr => "Alice".to_string(),
        id if id == bob_addr => "Bob".to_string(),
        id if id == charlie_addr => "Charlie".to_string(),
        id if id == dave_addr => "Dave".to_string(),
        id if id == eve_addr => "Eve".to_string(),
        id if id == frank_addr => "Frank".to_string(),
        id if id == grace_addr => "Grace".to_string(),
        id if id == heidi_addr => "Heidi".to_string(),
        id if id == ivan_addr => "Ivan".to_string(),
        id if id == jane_addr => "Jane".to_string(),
        id if id == kevin_addr => "Kevin".to_string(),
        id if id == lisa_addr => "Lisa".to_string(),
        id if id == mike_addr => "Mike".to_string(),
        id if id == nancy_addr => "Nancy".to_string(),
        id if id == oscar_addr => "Oscar".to_string(),
        id if id == peggy_addr => "Peggy".to_string(),
        _ => panic!("unsupported Id: {:?}", id),
    }
}

fn name_payload(msg: &DkgMessage) -> String {
    match msg {
        DkgMessage::Round1 { initiator, .. } => {
            format!("Round1({})", name_addr(&initiator.0))
        }
        DkgMessage::AckRound1 { initiator, .. } => {
            format!("AckRound1({})", name_addr(&initiator.0))
        }
        DkgMessage::Round2 { initiator, target, .. } => {
            format!("Round2({}.{})", name_addr(&initiator.0), name_addr(&target.0))
        }
        DkgMessage::AckRound2 { initiator, target, .. } => {
            format!("AckRound2({}.{})", name_addr(&initiator.0), name_addr(&target.0))
        }
        DkgMessage::Round3 { initiator, .. } => {
            format!("Round3({})", name_addr(&initiator.0))
        }
        DkgMessage::AckRound3 { initiator, .. } => {
            format!("AckRound3({})", name_addr(&initiator.0))
        }
    }
}

fn name_stage(stage: Stage) -> String {
    match stage {
        Stage::AwaitingInit => String::from("AA"),
        Stage::RoundOne => String::from("R1"),
        Stage::RoundTwo => String::from("R2"),
        Stage::RoundThree => String::from("R3"),
        Stage::Finalized => String::from("FF"),
        Stage::Aborted => String::from("!!"),
    }
}
