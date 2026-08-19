//! Statement dependency graph and proving schedule for a
//! jointly-assembled transaction.
//!
//! Nodes are the private-data-dependent exported statements of one
//! transaction (the contributions parties exchange as pods) plus the
//! finalize itself. Statements depending only on public plan data are
//! not nodes: they are proven inside whichever pod needs them and
//! duplicate freely across pods. Each node names the one party able
//! to prove it and the foreign statements its proof consumes; the
//! schedule derived from those edges is the protocol the parties
//! follow.
//!
//! Graphs are hand-declared per transaction script. This module is
//! test-only for now: where the vocabulary finally lives (txlib
//! proper or a negotiation crate) is deliberately deferred until the
//! borrow scenario shows the shared shape.

use std::collections::HashMap;
use std::fmt;

use pod2::middleware::Hash;

/// What a node's exported statement is, identified by plan data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A `TransferOffer` for the state with this commitment: its
    /// owner's openings, key erasure, and spend endorsement.
    TransferOffer { object: Hash },
    /// A `TransferAcceptance` of the state with this commitment,
    /// proven at the plan's positions for its leg.
    TransferAcceptance { object: Hash },
    /// The assembling party's `TxFinalized`.
    Finalize,
}

/// One exported statement: who alone can prove it, and which foreign
/// statements its proof consumes.
#[derive(Clone, Debug)]
pub struct StatementNode {
    pub id: String,
    pub producer: String,
    pub kind: NodeKind,
    pub premises: Vec<String>,
}

impl StatementNode {
    pub fn new(id: &str, producer: &str, kind: NodeKind, premises: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            producer: producer.to_string(),
            kind,
            premises: premises.iter().map(|p| p.to_string()).collect(),
        }
    }
}

/// One party's proving session: the statements it proves together in
/// one builder, and the foreign statements that must have arrived as
/// pods before it can start.
#[derive(Clone, Debug)]
pub struct ProvingSession {
    pub party: String,
    pub statements: Vec<String>,
    pub imports: Vec<String>,
}

/// The proving schedule: rounds of sessions, where sessions within a
/// round run in parallel and pods cross between rounds. The number of
/// sequential exchanges is `rounds - 1`.
#[derive(Clone, Debug)]
pub struct Schedule {
    pub rounds: Vec<Vec<ProvingSession>>,
}

impl Schedule {
    pub fn exchange_count(&self) -> usize {
        self.rounds.len().saturating_sub(1)
    }
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (round, sessions) in self.rounds.iter().enumerate() {
            for session in sessions {
                write!(
                    f,
                    "round {round}: {} proves [{}]",
                    session.party,
                    session.statements.join(", ")
                )?;
                if !session.imports.is_empty() {
                    write!(f, " importing [{}]", session.imports.join(", "))?;
                }
                writeln!(f)?;
            }
        }
        Ok(())
    }
}

/// A transaction's exported statements and their premise edges.
#[derive(Debug)]
pub struct StatementGraph {
    nodes: Vec<StatementNode>,
    /// Per node, its premises resolved to node indices at validation
    /// time, so scheduling is pure integer indexing.
    premise_indices: Vec<Vec<usize>>,
}

impl StatementGraph {
    /// Validate and build. Premises must name previously declared
    /// nodes, which is what makes hand-declared graphs acyclic by
    /// construction. An acceptance must consume its own object's
    /// offer, since only the sender's key-erasing statement lets the
    /// receiver prove `Rekey`.
    pub fn new(nodes: Vec<StatementNode>) -> anyhow::Result<Self> {
        let mut declared: HashMap<&str, usize> = HashMap::new();
        let mut premise_indices: Vec<Vec<usize>> = Vec::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            anyhow::ensure!(
                !declared.contains_key(node.id.as_str()),
                "duplicate node id: {}",
                node.id
            );
            let mut resolved = Vec::with_capacity(node.premises.len());
            for premise in &node.premises {
                let Some(&premise_index) = declared.get(premise.as_str()) else {
                    anyhow::bail!(
                        "node {} names premise {} before it is declared",
                        node.id,
                        premise
                    );
                };
                resolved.push(premise_index);
            }
            if let NodeKind::TransferAcceptance { object } = &node.kind {
                let consumes_offer = resolved.iter().any(|&premise_index| {
                    matches!(
                        &nodes[premise_index].kind,
                        NodeKind::TransferOffer { object: offered } if offered == object
                    )
                });
                anyhow::ensure!(
                    consumes_offer,
                    "acceptance {} does not consume its object's offer",
                    node.id
                );
            }
            declared.insert(node.id.as_str(), index);
            premise_indices.push(resolved);
        }
        Ok(Self {
            nodes,
            premise_indices,
        })
    }

    /// Derive the proving schedule. A node's earliest round is forced
    /// by its foreign premises (a pod must cross for each producer
    /// alternation); it is then deferred as late as its consumers
    /// allow, which batches a party's statements into few sessions.
    pub fn schedule(&self) -> Schedule {
        let crosses = |consumer: usize, premise: usize| {
            self.nodes[premise].producer != self.nodes[consumer].producer
        };

        let count = self.nodes.len();
        let mut earliest = vec![0usize; count];
        for i in 0..count {
            for &j in &self.premise_indices[i] {
                earliest[i] = earliest[i].max(earliest[j] + usize::from(crosses(i, j)));
            }
        }

        // Latest round each node can run without delaying a consumer;
        // consumers only appear later in declaration order, so one
        // reverse pass sees every consumer's bound before placing a
        // node.
        let mut bound: Vec<Option<usize>> = vec![None; count];
        let mut round_of = vec![0usize; count];
        for i in (0..count).rev() {
            round_of[i] = bound[i].unwrap_or(earliest[i]);
            for &j in &self.premise_indices[i] {
                let limit = round_of[i] - usize::from(crosses(i, j));
                bound[j] = Some(bound[j].map_or(limit, |b| b.min(limit)));
            }
        }

        let round_count = round_of.iter().max().map_or(0, |last| last + 1);
        let mut rounds: Vec<Vec<ProvingSession>> = vec![Vec::new(); round_count];
        for (i, node) in self.nodes.iter().enumerate() {
            let sessions = &mut rounds[round_of[i]];
            let position = match sessions.iter().position(|s| s.party == node.producer) {
                Some(position) => position,
                None => {
                    sessions.push(ProvingSession {
                        party: node.producer.clone(),
                        statements: Vec::new(),
                        imports: Vec::new(),
                    });
                    sessions.len() - 1
                }
            };
            let session = &mut sessions[position];
            session.statements.push(node.id.clone());
            for (&j, premise) in self.premise_indices[i].iter().zip(&node.premises) {
                if crosses(i, j) && !session.imports.contains(premise) {
                    session.imports.push(premise.clone());
                }
            }
        }
        Schedule { rounds }
    }
}

fn test_hash(byte: u8) -> Hash {
    Hash([pod2::middleware::F(byte as u64); 4])
}

// A transfer whose receiver finalizes is the parallel optimum: one
// exchange, straight to the finalizer.
#[test]
fn receiver_assembled_transfer_schedules_one_exchange() {
    let graph = StatementGraph::new(vec![
        StatementNode::new(
            "offer",
            "alice",
            NodeKind::TransferOffer {
                object: test_hash(1),
            },
            &[],
        ),
        StatementNode::new("finalize", "bob", NodeKind::Finalize, &["offer"]),
    ])
    .unwrap();
    let schedule = graph.schedule();
    assert_eq!(schedule.exchange_count(), 1);
    assert_eq!(schedule.rounds[0][0].party, "alice");
    assert_eq!(schedule.rounds[1][0].imports, vec!["offer"]);
}

#[test]
fn graph_rejects_malformed_declarations() {
    let offer = StatementNode::new(
        "offer",
        "alice",
        NodeKind::TransferOffer {
            object: test_hash(1),
        },
        &[],
    );

    let err =
        StatementGraph::new(vec![offer.clone(), offer.clone()]).expect_err("duplicate node id");
    assert!(format!("{err}").contains("duplicate node id"));

    let err = StatementGraph::new(vec![StatementNode::new(
        "finalize",
        "bob",
        NodeKind::Finalize,
        &["offer"],
    )])
    .expect_err("premise declared nowhere");
    assert!(format!("{err}").contains("before it is declared"));

    let err = StatementGraph::new(vec![
        offer,
        StatementNode::new(
            "accept",
            "bob",
            NodeKind::TransferAcceptance {
                object: test_hash(2),
            },
            &["offer"],
        ),
    ])
    .expect_err("acceptance consuming a different object's offer");
    assert!(format!("{err}").contains("does not consume its object's offer"));
}
