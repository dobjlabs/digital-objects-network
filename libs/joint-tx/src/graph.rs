//! Statement dependencies and proving schedule for a joint transaction.
//!
//! Nodes represent exported statements that depend on private data, plus
//! finalization. Each node names its producer and imported premises.
//! Statements derived only from public plan data are omitted.
//!
//! [`crate::JointTransaction`] derives the graph from labeled events.

use std::collections::HashMap;
use std::fmt;

use pod2::middleware::Hash;

/// What a node's exported statement is, identified by plan data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Openings, key erasure, and spend authorization for this state.
    TransferOffer { object: Hash },
    /// A `TransferAcceptance` of the state with this commitment,
    /// proven at the plan's positions for its leg.
    TransferAcceptance { object: Hash },
    /// The assembling party's `TxFinalized`.
    Finalize,
}

/// One exported statement, its producer, and its imported premises.
#[derive(Clone, Debug)]
pub struct StatementNode {
    /// `offer:{index}`, `accept:{index}`, or `finalize`.
    /// Transfer ids refer to top-level action positions.
    pub(crate) id: String,
    /// Participant that proves this node.
    pub(crate) producer: String,
    pub(crate) kind: NodeKind,
    pub(crate) premises: Vec<String>,
}

impl StatementNode {
    pub(crate) fn new(id: &str, producer: &str, kind: NodeKind, premises: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            producer: producer.to_string(),
            kind,
            premises: premises.iter().map(|p| p.to_string()).collect(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn producer(&self) -> &str {
        &self.producer
    }

    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// Ids of the foreign nodes this node's proof consumes.
    pub fn premises(&self) -> &[String] {
        &self.premises
    }
}

/// Statements one party proves together and the node ids it must import.
#[derive(Clone, Debug)]
pub struct ProvingSession {
    pub party: String,
    pub statements: Vec<String>,
    pub imports: Vec<String>,
}

/// Parallel proving rounds. Pods cross between rounds.
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
#[derive(Clone, Debug)]
pub struct StatementGraph {
    nodes: Vec<StatementNode>,
    /// Premises resolved to node indices during validation.
    premise_indices: Vec<Vec<usize>>,
}

impl StatementGraph {
    /// Validate nodes and resolve premise ids.
    ///
    /// Premises must be declared earlier, making the graph acyclic. A
    /// transfer acceptance must consume the matching offer.
    pub(crate) fn new(nodes: Vec<StatementNode>) -> anyhow::Result<Self> {
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

    /// The declared nodes, in declaration order.
    pub fn nodes(&self) -> &[StatementNode] {
        &self.nodes
    }

    /// Schedule nodes around cross-party premises, delaying each node as far
    /// as its consumers allow to batch work by party.
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

        // A reverse pass finds the latest round that does not delay a consumer.
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

#[cfg(test)]
fn test_hash(byte: u8) -> Hash {
    Hash([pod2::middleware::F(byte as u64); 4])
}

// A receiver-finalized transfer needs one exchange.
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
