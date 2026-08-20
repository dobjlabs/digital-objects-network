//! `dobj-trade`: a two-user swap over iroh, driven by the joint
//! transaction machinery. One user runs `offer`, hands the printed
//! invitation to a counterparty out-of-band, and the counterparty runs
//! `accept`. The initiator executes: it assembles both legs, finalizes,
//! and posts; both sides watch the synchronizer for the landing.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use driver::{
    HttpSynchronizerClient, ObjectRecord, SYNCHRONIZER_POLL_INTERVAL_MS,
    SYNCHRONIZER_POLL_TIMEOUT_SECS, SynchronizerClient,
};
use joint_tx::graph::{NodeKind, StatementGraph, StatementNode};
use pod2::middleware::Hash;
use trade::engine::{Accepter, Initiator, SwapDeps, SwapExpectation};
use trade::protocol::{Invitation, WireMsg};
use trade::{local::Local, net, post, ui};

#[derive(Parser)]
#[command(name = "dobj-trade", about = "swap digital objects with another user")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Offer a trade: prints an invitation to hand to the counterparty,
    /// then waits for them to connect.
    Offer {
        /// Class you are giving (e.g. Wood or craft-basics::Wood).
        #[arg(long)]
        give: String,
        /// Class you want in return.
        #[arg(long)]
        want: String,
        /// Commitment prefix selecting which of your objects to give.
        #[arg(long)]
        object: Option<String>,
        /// Prove with the mock prover and skip posting (local dry run).
        #[arg(long)]
        mock: bool,
    },
    /// Accept a trade from a pasted invitation.
    Accept {
        /// The invitation blob; prompted for if omitted.
        blob: Option<String>,
        /// Commitment prefix selecting which of your objects to give.
        #[arg(long)]
        object: Option<String>,
        /// Prove with the mock prover and skip watching (local dry run).
        #[arg(long)]
        mock: bool,
        /// Answer yes to every prompt (for scripted runs).
        #[arg(long)]
        yes: bool,
    },
    /// List your live objects.
    Objects,
    /// Fabricate a live object for mock runs (dev scaffolding).
    #[command(hide = true)]
    DevSpawn {
        /// Class to fabricate (e.g. Wood).
        class: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    match Cli::parse().cmd {
        Cmd::Offer {
            give,
            want,
            object,
            mock,
        } => run_offer(give, want, object, mock).await,
        Cmd::Accept {
            blob,
            object,
            mock,
            yes,
        } => run_accept(blob, object, mock, yes).await,
        Cmd::Objects => run_objects(),
        Cmd::DevSpawn { class } => run_dev_spawn(class),
    }
}

fn blocking<T>(f: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(f)
}

fn run_dev_spawn(class: String) -> Result<()> {
    let local = Local::open()?;
    let class_hash = local.catalog.resolve_class(&class)?;
    let record = trade::local::dev_spawn(&local, class_hash)?;
    println!(
        "spawned {} {}",
        local.catalog.class_label(class_hash),
        ui::short(&record.content_hash)
    );
    Ok(())
}

fn run_objects() -> Result<()> {
    let local = Local::open()?;
    let objects = local.live_objects()?;
    if objects.is_empty() {
        println!("no live objects");
        return Ok(());
    }
    for record in objects {
        let class_label = local.catalog.class_label(class_hash_of(&record)?);
        println!(
            "  {}  {}  {}",
            ui::short(&record.obj.commitment()),
            class_label,
            ui::describe_object(&record.obj)
        );
    }
    Ok(())
}

fn class_hash_of(record: &ObjectRecord) -> Result<Hash> {
    let value = record
        .obj
        .get(&pod2::middleware::StrKey::from("type"))
        .ok()
        .flatten()
        .ok_or_else(|| anyhow!("object {} has no type field", record.content_hash))?;
    Ok(Hash(value.raw().0))
}

/// Pick which of the user's live objects of `class_hash` to give.
fn pick_object(local: &Local, class_hash: Hash, prefix: Option<&str>) -> Result<ObjectRecord> {
    let mut candidates = local.live_objects_of_class(class_hash)?;
    if let Some(prefix) = prefix {
        let prefix = prefix.trim_start_matches("0x").to_ascii_lowercase();
        candidates.retain(|record| {
            format!("{:#}", record.obj.commitment())
                .trim_start_matches("0x")
                .to_ascii_lowercase()
                .starts_with(&prefix)
        });
    }
    match candidates.len() {
        0 => anyhow::bail!(
            "you hold no live object of class {}",
            local.catalog.class_label(class_hash)
        ),
        1 => Ok(candidates.remove(0)),
        _ => {
            println!("  you hold several; pick one with --object <commitment prefix>:");
            for record in &candidates {
                ui::object_line("candidate", &record.obj);
            }
            anyhow::bail!("ambiguous object choice");
        }
    }
}

/// The swap's statement graph, labeled from one side's point of view:
/// "mine" is the object this user gives. Rendering its schedule is the
/// narration: the schedule IS the protocol both processes follow.
fn narrate_schedule(my_object: Hash, their_object: Hash, i_finalize: bool) {
    let nodes = if i_finalize {
        vec![
            StatementNode::new(
                "offer:theirs",
                "their side",
                NodeKind::TransferOffer {
                    object: their_object,
                },
                &[],
            ),
            StatementNode::new(
                "offer:mine",
                "this side",
                NodeKind::TransferOffer { object: my_object },
                &[],
            ),
            StatementNode::new(
                "accept:mine",
                "their side",
                NodeKind::TransferAcceptance { object: my_object },
                &["offer:mine"],
            ),
            StatementNode::new(
                "finalize",
                "this side",
                NodeKind::Finalize,
                &["offer:theirs", "accept:mine"],
            ),
        ]
    } else {
        vec![
            StatementNode::new(
                "offer:mine",
                "this side",
                NodeKind::TransferOffer { object: my_object },
                &[],
            ),
            StatementNode::new(
                "offer:theirs",
                "their side",
                NodeKind::TransferOffer {
                    object: their_object,
                },
                &[],
            ),
            StatementNode::new(
                "accept:theirs",
                "this side",
                NodeKind::TransferAcceptance {
                    object: their_object,
                },
                &["offer:theirs"],
            ),
            StatementNode::new(
                "finalize",
                "their side",
                NodeKind::Finalize,
                &["offer:mine", "accept:theirs"],
            ),
        ]
    };
    let graph = StatementGraph::new(nodes).expect("the swap graph is well-formed");
    ui::heading("the schedule (this is the whole protocol)");
    for line in graph.schedule().to_string().lines() {
        ui::note(line);
    }
}

fn confirm(question: &str, auto_yes: bool) -> bool {
    if auto_yes {
        println!("  {question} [y/N] y (auto)");
        return true;
    }
    ui::confirm(question)
}

async fn run_offer(give: String, want: String, object: Option<String>, mock: bool) -> Result<()> {
    let local = blocking(Local::open)?;
    let give_class = local.catalog.resolve_class(&give)?;
    let want_class = local.catalog.resolve_class(&want)?;
    let record = pick_object(&local, give_class, object.as_deref())?;
    let give_label = local.catalog.class_label(give_class);
    let want_label = local.catalog.class_label(want_class);

    ui::say("you", &format!("offering {give_label} for {want_label}"));
    ui::object_line("your object", &record.obj);

    ui::heading("opening the door");
    ui::say("you", "binding an iroh endpoint and coming online...");
    let (endpoint, addr) = net::listen().await?;
    let invitation = Invitation {
        node: addr,
        offers: give_class,
        wants: want_class,
        mock,
    };
    ui::key_moment("hand this invitation to your counterparty (any channel works)");
    ui::note("whoever holds this invitation can claim the trade; hand it only to your");
    ui::note("intended counterparty.");
    println!("{}", invitation.encode());
    println!();
    ui::say("you", "waiting for them to paste it and connect...");

    let (connection, mut channel) = net::accept_peer(&endpoint).await?;
    ui::say("them", "connected over iroh");

    // Data round, their half.
    let accept = match channel.recv().await? {
        WireMsg::Accept(msg) => msg,
        other => anyhow::bail!("expected their disclosure, got {other:?}"),
    };
    ui::heading("data round");
    ui::say(
        "them",
        "disclosing the object they give (fields, commitment, nullifier; never the key):",
    );
    ui::object_line("their object", &accept.accepter_object.mid);
    ui::note(&format!(
        "nullifier {} -- spendable only with their key",
        ui::short(&accept.accepter_object.nullifier)
    ));
    ui::note("disclosing your side is irrevocable: they will recognize this object's");
    ui::note("later spend even if the deal dies here.");

    let commitments = [
        accept.accepter_object.old_commitment,
        record.obj.commitment(),
    ];
    let witness = if mock {
        ui::say(
            "you",
            "mock run: grounding both inputs in a synthetic local state",
        );
        trade::local::mock_grounding_witness(&commitments)
    } else {
        ui::say(
            "you",
            "fetching one grounding witness for both inputs from the synchronizer...",
        );
        let sync_url = local.settings.synchronizer_api_url.clone();
        Arc::new(blocking(|| {
            HttpSynchronizerClient::new().fetch_grounding_witness(&sync_url, &commitments)
        })?)
    };
    ui::note(&format!(
        "grounded against state root {} (block {})",
        ui::short(&witness.state_header.hash()),
        witness.state_header.block_number
    ));

    let deps = SwapDeps {
        modules: local.catalog.modules.clone(),
        classes: local.catalog.classes.clone(),
        mock,
    };
    let initiator = Initiator::new(deps, record.obj.clone(), want_class);
    let (initiator, plan_data) = blocking(|| initiator.on_accept(&accept, witness.clone()))?;
    ui::say(
        "you -> them",
        "your disclosure plus the grounding header and your projected new state",
    );
    channel.send(&WireMsg::PlanData(plan_data)).await?;

    let plan_ack = match channel.recv().await? {
        WireMsg::PlanAck(msg) => msg,
        other => anyhow::bail!("expected their plan ack, got {other:?}"),
    };
    ui::say(
        "them",
        &format!(
            "agreed; both sides derived tx_final {}",
            ui::short(&plan_ack.tx_final)
        ),
    );
    ui::note("endorsements bind this exact effect: it lands exactly as read, or not at all.");

    narrate_schedule(
        record.obj.commitment(),
        accept.accepter_object.old_commitment,
        true,
    );

    if !mock {
        file_projection(
            &local,
            initiator.projected_received(),
            want_class,
            plan_ack.tx_final,
            &mut channel,
        )
        .await?;
    }

    ui::heading("round 0: your offer");
    ui::say(
        "you",
        "proving the offer of your object (openings, key erasure, spend endorsement)...",
    );
    let (initiator, offer) = blocking(|| initiator.on_plan_ack(&plan_ack))?;
    ui::say("you -> them", "offer pod");
    channel.send(&WireMsg::Offer(Box::new(offer))).await?;

    ui::say(
        "you",
        "waiting for their combined offer-plus-acceptance session...",
    );
    let acceptance = match channel.recv().await? {
        WireMsg::Acceptance(msg) => *msg,
        other => anyhow::bail!("expected their acceptance, got {other:?}"),
    };
    ui::say(
        "them",
        "their offer, their acceptance of your object, and its class guard, in one pod",
    );

    ui::heading("round 2: assembly and finalize");
    channel
        .send(&WireMsg::Progress {
            note: "validating your pod, assembling both legs, finalizing, proving".to_string(),
        })
        .await?;
    ui::say(
        "you",
        "validating their artifacts against the pod, then assembling...",
    );
    if !mock {
        ui::note("real proving now; expect roughly half a minute of silence.");
    }
    let outcome = blocking(|| initiator.on_acceptance(acceptance))?;
    ui::say(
        "you",
        &format!(
            "TxFinalized proven; tx_final {}",
            ui::short(&outcome.expectation.tx_final)
        ),
    );

    if mock {
        channel
            .send(&WireMsg::Posted {
                tx_hash: None,
                block_number: None,
            })
            .await?;
        ui::key_moment("mock run complete: nothing was posted");
        summarize(&outcome.expectation, &local);
    } else {
        channel
            .send(&WireMsg::Progress {
                note: "shrinking the proof into a blob payload and posting via the relayer"
                    .to_string(),
            })
            .await?;
        ui::say("you", "shrinking the proof and posting via the relayer...");
        let state_root = witness.state_header.hash();
        let relayer_url = local.settings.relayer_api_url.clone();
        let pod = outcome.pod.clone();
        let confirmation = blocking(move || -> Result<_> {
            let bytes = post::build_payload_bytes(state_root, &outcome.tx, pod)?;
            post::post_and_confirm(&relayer_url, &bytes)
        })?;
        ui::say(
            "you",
            &format!(
                "posted: eth tx {}",
                confirmation.tx_hash.as_deref().unwrap_or("(pending)")
            ),
        );
        channel
            .send(&WireMsg::Posted {
                tx_hash: confirmation.tx_hash.clone(),
                block_number: confirmation.block_number,
            })
            .await?;
        watch_and_reconcile(&local, &outcome.expectation, record.obj.commitment()).await?;
    }

    // The final message is ours; let it deliver, then wait for the
    // counterparty to hang up before closing.
    channel.finish();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), connection.closed()).await;
    endpoint.close().await;
    Ok(())
}

async fn run_accept(
    blob: Option<String>,
    object: Option<String>,
    mock: bool,
    yes: bool,
) -> Result<()> {
    let local = blocking(Local::open)?;
    let blob = match blob {
        Some(blob) => blob,
        None => blocking(|| -> Result<String> {
            println!("paste the invitation blob:");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            Ok(line)
        })?,
    };
    let invitation = Invitation::decode(&blob)?;
    if mock && !invitation.mock {
        anyhow::bail!(
            "this invitation is for a real trade; drop --mock or ask for a mock invitation"
        );
    }
    let mock = invitation.mock;
    let offers_label = local.catalog.class_label(invitation.offers);
    let wants_label = local.catalog.class_label(invitation.wants);
    ui::banner();
    println!();
    ui::field("their iroh id", &invitation.node.id.to_string());
    ui::field(
        "they give",
        &format!("{offers_label}  (guard {})", ui::short(&invitation.offers)),
    );
    ui::field(
        "they want",
        &format!("{wants_label}  (guard {})", ui::short(&invitation.wants)),
    );
    if mock {
        ui::field("mode", "MOCK DRY RUN: mock proofs, nothing will be posted");
    }

    let record = pick_object(&local, invitation.wants, object.as_deref())?;
    ui::object_line("you would give", &record.obj);
    ui::note("accepting discloses this object's fields, commitment, and nullifier to them,");
    ui::note("irrevocably even if the deal dies. this is your decision point: everything");
    ui::note("after runs to completion.");
    if !blocking(|| confirm("accept the trade?", yes)) {
        anyhow::bail!("trade declined");
    }

    ui::say("you", "dialing their endpoint over iroh...");
    let (endpoint, connection, mut channel) = net::connect(invitation.node.clone()).await?;
    ui::say("you", "connected");

    let deps = SwapDeps {
        modules: local.catalog.modules.clone(),
        classes: local.catalog.classes.clone(),
        mock,
    };
    let accepter = Accepter::new(deps, record.obj.clone(), invitation.offers);
    let (accepter, accept_msg) = accepter.accept();
    ui::heading("data round");
    ui::say(
        "you -> them",
        "your disclosure (fields, commitment, nullifier; never the key)",
    );
    let my_commitment = record.obj.commitment();
    channel.send(&WireMsg::Accept(accept_msg)).await?;

    let plan_data = match channel.recv().await? {
        WireMsg::PlanData(msg) => msg,
        other => anyhow::bail!("expected their plan data, got {other:?}"),
    };
    ui::say(
        "them",
        "their disclosure, the grounding header, and their projected new state:",
    );
    ui::object_line("their object", &plan_data.initiator_object.mid);
    ui::note(&format!(
        "grounding block {}, state root {}",
        plan_data.header.block_number,
        ui::short(&plan_data.header.hash())
    ));
    let (accepter, plan_ack) = blocking(|| accepter.on_plan_data(&plan_data))?;
    let their_object = plan_data.initiator_object.old_commitment;
    if !mock {
        file_projection(
            &local,
            accepter.projected_received(),
            invitation.offers,
            plan_ack.tx_final,
            &mut channel,
        )
        .await?;
    }
    ui::say(
        "you -> them",
        &format!("plan agreed; tx_final {}", ui::short(&plan_ack.tx_final)),
    );
    ui::note(
        "your endorsement will bind this exact effect: it lands exactly as read, or not at all.",
    );
    channel.send(&WireMsg::PlanAck(plan_ack)).await?;

    narrate_schedule(my_commitment, their_object, false);

    ui::say("you", "waiting for their offer pod (round 0)...");
    let offer = match channel.recv().await? {
        WireMsg::Offer(msg) => *msg,
        other => anyhow::bail!("expected their offer, got {other:?}"),
    };
    ui::say("them", "offer pod for the object they give");

    ui::heading("round 1: your combined session");
    ui::say(
        "you",
        "validating their offer, then proving your offer, your acceptance, and the class guard...",
    );
    if !mock {
        ui::note("real proving now; this side takes a while too.");
    }
    let (acceptance, expectation) = blocking(|| accepter.on_offer(offer))?;
    ui::say("you -> them", "combined offer-plus-acceptance pod");
    channel
        .send(&WireMsg::Acceptance(Box::new(acceptance)))
        .await?;

    ui::heading("round 2 is theirs: they assemble, finalize, and post");
    loop {
        match channel.recv().await? {
            WireMsg::Progress { note } => ui::say("them", &note),
            WireMsg::Posted {
                tx_hash,
                block_number,
            } => {
                match (&tx_hash, mock) {
                    (Some(hash), _) => ui::say(
                        "them",
                        &format!(
                            "posted: eth tx {hash}{}",
                            block_number
                                .map(|block| format!(" (block {block})"))
                                .unwrap_or_default()
                        ),
                    ),
                    (None, true) => ui::say("them", "mock run complete: nothing was posted"),
                    (None, false) => ui::say("them", "posted (no tx hash reported)"),
                }
                break;
            }
            other => anyhow::bail!("unexpected message while watching: {other:?}"),
        }
    }

    if mock {
        summarize(&expectation, &local);
    } else {
        watch_and_reconcile(&local, &expectation, record.obj.commitment()).await?;
    }

    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(())
}

/// File the projected received state (key included) in dobjd before
/// any endorsement leaves this machine. Until the transaction lands it
/// sits as an unknown object; the daemon's sync flips it live once the
/// commitment is on-chain. Failing here is safe, so it aborts the
/// deal: proceeding without a durable copy of the new key risks an
/// object nobody can ever spend.
async fn file_projection(
    local: &Local,
    projection: &pod2::middleware::containers::Dictionary,
    received_class: Hash,
    tx_final: Hash,
    channel: &mut trade::net::Channel,
) -> Result<()> {
    ui::say(
        "you",
        "filing your projected new object (with its key) in dobjd before endorsing...",
    );
    let result = blocking(|| local.import_received(projection, received_class, tx_final));
    if let Err(err) = result {
        net::abort(channel, "could not durably file the projected object").await;
        return Err(err.context(
            "dobjd must be reachable to file the projected object before endorsing; aborted safely",
        ));
    }
    ui::note("it shows as an unknown object until the transaction lands; if the deal");
    ui::note("dies instead, delete it from dobjd.");
    Ok(())
}

/// Both parties end the same way: watch their own synchronizer until
/// the transaction's whole effect is visible, then let their daemon
/// reconcile, which flips the received object live and moves the spent
/// one to the nullified store.
async fn watch_and_reconcile(
    local: &Local,
    expectation: &SwapExpectation,
    given: Hash,
) -> Result<()> {
    ui::heading("watching the synchronizer");
    ui::say(
        "you",
        &format!(
            "waiting until every new commitment and nullifier of tx_final {} is on-chain...",
            ui::short(&expectation.tx_final)
        ),
    );
    let sync_url = local.settings.synchronizer_api_url.clone();
    let created = expectation.new_commitments.clone();
    let nullifiers = expectation.nullifiers.clone();
    let head = blocking(move || {
        HttpSynchronizerClient::new().wait_for_tx(
            &sync_url,
            &created,
            &nullifiers,
            SYNCHRONIZER_POLL_TIMEOUT_SECS,
            SYNCHRONIZER_POLL_INTERVAL_MS,
        )
    })?;
    ui::say(
        "you",
        &format!(
            "landed; synchronizer head is now {}",
            ui::short(&head.current_state_root)
        ),
    );

    ui::say("you", "asking dobjd to reconcile object statuses...");
    match blocking(|| local.object_statuses()) {
        Ok(statuses) => {
            let received = expectation.received.commitment();
            match statuses.get(&received) {
                Some(wire_types::ObjectStatus::Live) => {
                    ui::note("your new object is live in the object store.")
                }
                Some(status) => ui::note(&format!(
                    "your new object is filed but still {}; the next sync should settle it.",
                    status.as_str()
                )),
                None => ui::note("your new object is not in dobjd's store; check the daemon."),
            }
            match statuses.get(&given) {
                Some(wire_types::ObjectStatus::Nullified) | None => {
                    ui::note("the object you gave is spent and moved to the nullified store.")
                }
                Some(status) => ui::note(&format!(
                    "the object you gave still shows as {}; the next sync should settle it.",
                    status.as_str()
                )),
            }
        }
        Err(err) => {
            ui::note(&format!(
                "dobjd is unreachable ({err:#}); your new object was filed before endorsing, so"
            ));
            ui::note("nothing is lost: statuses settle on the daemon's next sync.");
        }
    }
    summarize(expectation, local);
    Ok(())
}

fn summarize(expectation: &SwapExpectation, local: &Local) {
    let class = class_label_of_dict(local, &expectation.received);
    ui::key_moment("trade complete");
    ui::say("you", &format!("you now control {class}"));
    ui::object_line("received", &expectation.received);
    ui::note(&format!(
        "under tx_final {}",
        ui::short(&expectation.tx_final)
    ));
}

fn class_label_of_dict(local: &Local, obj: &pod2::middleware::containers::Dictionary) -> String {
    obj.get(&pod2::middleware::StrKey::from("type"))
        .ok()
        .flatten()
        .map(|value| local.catalog.class_label(Hash(value.raw().0)))
        .unwrap_or_else(|| "(unknown class)".to_string())
}
