use std::collections::HashMap;

use anyhow::{Result, anyhow};
use pod2::{
    backends::plonky2::{
        basetypes::{
            CommonCircuitData, HashOut, ProofWithPublicInputs, VerifierCircuitData,
            VerifierOnlyCircuitData,
        },
        primitives::merkletree::{MerkleTree, MerkleTreeStateTransitionProof},
        recursion::{
            RecursiveCircuit, RecursiveParams, circuit::dummy, hash_verifier_data, new_params,
        },
    },
    middleware::{F, Hash, RawValue},
};

use crate::{
    AGG_NUM_PUBLIC_INPUTS, AGG_ROOT_RANGE, AGG_VERIFIER_DATA_HASH_RANGE, MAX_AGG_SIZE,
    aggregation_root,
    circuit::{AggregatorInnerCircuit, AggregatorInput, AggregatorParams, AggregatorSlot},
    leaf_hash, leaf_key,
};

/// Builds and proves the Aggregator circuit for a fixed number of shrunk proofs per step.
pub struct Aggregator {
    num_input_pods: usize,
    circuit: RecursiveCircuit<AggregatorInnerCircuit>,
    recursive_params: RecursiveParams,
    shrunk_verifier_data: VerifierOnlyCircuitData,
    agg_verifier_data_hash: HashOut,
    /// Stands in for the previous Aggregator proof on the base case.
    dummy_verifier_data: VerifierOnlyCircuitData,
    dummy_proof: ProofWithPublicInputs,
}

/// The latest Aggregator proof of a chain, with the public inputs of every shrunk proof it
/// aggregates, in tree order.
#[derive(Clone)]
pub struct AggregationState {
    pub proof: ProofWithPublicInputs,
    pub shrunk_public_inputs: Vec<Vec<F>>,
    tree: MerkleTree,
}

impl Aggregator {
    pub fn new(
        num_input_pods: usize,
        shrunk_common_data: &CommonCircuitData,
        shrunk_verifier_data: &VerifierOnlyCircuitData,
    ) -> Result<Self> {
        assert!(num_input_pods > 0);
        let inner_params = AggregatorParams {
            num_input_pods,
            shrunk_common_data: shrunk_common_data.clone(),
            shrunk_verifier_data_hash: hash_verifier_data(shrunk_verifier_data),
        };
        let recursive_params =
            new_params::<AggregatorInnerCircuit>(1, AGG_NUM_PUBLIC_INPUTS, &inner_params)?;
        let circuit = RecursiveCircuit::build(&recursive_params, &inner_params)?;
        let (dummy_verifier_data, dummy_proof) =
            dummy(recursive_params.common_data(), AGG_NUM_PUBLIC_INPUTS)?;
        let agg_verifier_data_hash =
            hash_verifier_data(&recursive_params.verifier_data().verifier_only);
        Ok(Self {
            num_input_pods,
            circuit,
            recursive_params,
            shrunk_verifier_data: shrunk_verifier_data.clone(),
            agg_verifier_data_hash,
            dummy_verifier_data,
            dummy_proof,
        })
    }

    pub fn verifier_data(&self) -> &VerifierCircuitData {
        self.recursive_params.verifier_data()
    }

    pub fn common_data(&self) -> &CommonCircuitData {
        self.recursive_params.common_data()
    }

    /// Starts a chain with up to `num_input_pods` shrunk proofs.
    pub fn prove_base(
        &self,
        shrunk_proofs: Vec<ProofWithPublicInputs>,
    ) -> Result<AggregationState> {
        self.prove_step(None, shrunk_proofs)
    }

    /// Extends `previous` with up to `num_input_pods` shrunk proofs.
    pub fn prove_continue(
        &self,
        previous: AggregationState,
        shrunk_proofs: Vec<ProofWithPublicInputs>,
    ) -> Result<AggregationState> {
        self.prove_step(Some(previous), shrunk_proofs)
    }

    fn prove_step(
        &self,
        previous: Option<AggregationState>,
        shrunk_proofs: Vec<ProofWithPublicInputs>,
    ) -> Result<AggregationState> {
        if shrunk_proofs.is_empty() || shrunk_proofs.len() > self.num_input_pods {
            return Err(anyhow!(
                "expected 1 to {} shrunk proofs, got {}",
                self.num_input_pods,
                shrunk_proofs.len()
            ));
        }
        let continuing = previous.is_some();
        let (previous_proof, previous_verifier_data, mut tree, mut shrunk_public_inputs) =
            match previous {
                Some(state) => (
                    state.proof,
                    self.verifier_data().verifier_only.clone(),
                    state.tree,
                    state.shrunk_public_inputs,
                ),
                None => (
                    self.dummy_proof.clone(),
                    self.dummy_verifier_data.clone(),
                    MerkleTree::new(&HashMap::new()),
                    Vec::new(),
                ),
            };
        if shrunk_public_inputs.len() + shrunk_proofs.len() > MAX_AGG_SIZE {
            return Err(anyhow!(
                "aggregating {} more shrunk proofs would exceed {MAX_AGG_SIZE}",
                shrunk_proofs.len()
            ));
        }

        let filler_proof = shrunk_proofs[0].clone();
        let mut slots = Vec::with_capacity(self.num_input_pods);
        for proof in shrunk_proofs {
            let leaf = leaf_hash(&proof.public_inputs);
            let transition =
                tree.insert(&leaf_key(shrunk_public_inputs.len()), &RawValue(leaf.0))?;
            shrunk_public_inputs.push(proof.public_inputs.clone());
            slots.push(AggregatorSlot {
                proof,
                enabled: true,
                transition,
            });
        }
        while slots.len() < self.num_input_pods {
            slots.push(AggregatorSlot {
                proof: filler_proof.clone(),
                enabled: false,
                transition: MerkleTreeStateTransitionProof::pad(),
            });
        }

        let input = AggregatorInput {
            continuing,
            agg_verifier_data_hash: self.agg_verifier_data_hash,
            shrunk_verifier_data: self.shrunk_verifier_data.clone(),
            slots,
        };
        let proof =
            self.circuit
                .prove(&input, vec![previous_proof], vec![previous_verifier_data])?;
        Ok(AggregationState {
            proof,
            shrunk_public_inputs,
            tree,
        })
    }
}

/// Checks that `proof` is an Aggregator proof whose tree holds exactly `shrunk_public_inputs`,
/// in order.
pub fn verify_aggregation(
    proof: &ProofWithPublicInputs,
    verifier_data: &VerifierCircuitData,
    shrunk_public_inputs: &[Vec<F>],
) -> Result<()> {
    if proof.public_inputs.len() != AGG_NUM_PUBLIC_INPUTS {
        return Err(anyhow!(
            "expected {AGG_NUM_PUBLIC_INPUTS} public inputs, got {}",
            proof.public_inputs.len()
        ));
    }
    verifier_data.verify(proof.clone())?;

    let agg_verifier_data_hash = hash_verifier_data(&verifier_data.verifier_only);
    if proof.public_inputs[AGG_VERIFIER_DATA_HASH_RANGE] != agg_verifier_data_hash.elements {
        return Err(anyhow!(
            "the proof's verifier data hash is not the Aggregator's"
        ));
    }

    let leaves: Vec<Hash> = shrunk_public_inputs
        .iter()
        .map(|public_inputs| leaf_hash(public_inputs))
        .collect();
    if proof.public_inputs[AGG_ROOT_RANGE] != aggregation_root(&leaves).0 {
        return Err(anyhow!(
            "the proof's root does not match the shrunk public inputs"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use payload::shrink::{ShrunkMainPodBuild, ShrunkMainPodSetup};
    use plonky2::field::types::Field;
    use pod2::{
        backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover},
        frontend::{MainPodBuilder, Operation},
        middleware::Params,
    };

    use super::*;

    fn shrunk_proofs(
        params: &Params,
        shrunk_build: &ShrunkMainPodBuild,
        count: usize,
    ) -> Vec<ProofWithPublicInputs> {
        (0..count)
            .map(|index| {
                let mut builder = MainPodBuilder::new(params, &DEFAULT_VD_SET);
                builder
                    .pub_op(Operation::eq(index as i64, index as i64))
                    .unwrap();
                let pod = builder.prove(&Prover {}).unwrap();
                shrunk_build.prove(pod).unwrap()
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn test_aggregate_base_then_continue() {
        let params = Params::default();
        let shrunk_build = ShrunkMainPodSetup::new(&params).build().unwrap();
        let proofs = shrunk_proofs(&params, &shrunk_build, 3);
        let aggregator = Aggregator::new(
            2,
            &shrunk_build.circuit_data.common,
            &shrunk_build.circuit_data.verifier_only,
        )
        .unwrap();
        let verifier_data = aggregator.verifier_data();

        let base = aggregator.prove_base(proofs[0..2].to_vec()).unwrap();
        verify_aggregation(&base.proof, verifier_data, &base.shrunk_public_inputs).unwrap();

        let state = aggregator
            .prove_continue(base, proofs[2..3].to_vec())
            .unwrap();
        let public_inputs: Vec<Vec<F>> = proofs
            .iter()
            .map(|proof| proof.public_inputs.clone())
            .collect();
        assert_eq!(state.shrunk_public_inputs, public_inputs);
        verify_aggregation(&state.proof, verifier_data, &public_inputs).unwrap();

        let reordered = vec![
            public_inputs[1].clone(),
            public_inputs[0].clone(),
            public_inputs[2].clone(),
        ];
        assert!(verify_aggregation(&state.proof, verifier_data, &reordered).is_err());
        assert!(verify_aggregation(&state.proof, verifier_data, &public_inputs[0..2]).is_err());

        let mut tampered = state.proof.clone();
        tampered.public_inputs[AGG_VERIFIER_DATA_HASH_RANGE.start] += F::ONE;
        assert!(verify_aggregation(&tampered, verifier_data, &public_inputs).is_err());
    }

    #[test]
    #[ignore]
    fn test_aggregator_size() {
        let params = Params::default();
        let shrunk_build = ShrunkMainPodSetup::new(&params).build().unwrap();
        for num_input_pods in [1, 2, 4] {
            let aggregator = Aggregator::new(
                num_input_pods,
                &shrunk_build.circuit_data.common,
                &shrunk_build.circuit_data.verifier_only,
            )
            .unwrap();
            println!(
                "num_input_pods={num_input_pods} degree_bits={}",
                aggregator.common_data().degree_bits()
            );
        }
    }
}
