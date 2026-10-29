use plonky2::{
    field::types::Field,
    hash::{hash_types::HashOutTarget, poseidon::PoseidonHash},
    iop::{
        target::{BoolTarget, Target},
        witness::{PartialWitness, WitnessWrite},
    },
    plonk::{circuit_data::VerifierCircuitTarget, proof::ProofWithPublicInputsTarget},
};
use pod2::{
    backends::plonky2::{
        Result,
        basetypes::{
            C, CircuitBuilder, CommonCircuitData, D, HashOut, ProofWithPublicInputs,
            VerifierOnlyCircuitData,
        },
        circuits::common::{Flattenable, MerkleTreeStateTransitionClaimTarget},
        primitives::merkletree::{
            MerkleTreeOp, MerkleTreeStateTransitionProof, MerkleTreeStateTransitionProofTarget,
            verify_merkle_state_transition_circuit,
        },
        recursion::{InnerCircuit, VerifiedProofTarget, circuit::hash_verifier_data_gadget},
    },
    middleware::{EMPTY_HASH, F, HASH_SIZE},
};

use crate::{AGG_ROOT_RANGE, AGG_TREE_DEPTH, AGG_VERIFIER_DATA_HASH_RANGE};

#[derive(Clone, Debug)]
pub struct AggregatorParams {
    pub num_input_pods: usize,
    pub shrunk_common_data: CommonCircuitData,
    pub shrunk_verifier_data_hash: HashOut,
}

/// One shrunk proof verified by an Aggregator step. A disabled slot leaves the tree unchanged,
/// but its proof must still verify.
pub struct AggregatorSlot {
    pub proof: ProofWithPublicInputs,
    pub enabled: bool,
    pub transition: MerkleTreeStateTransitionProof,
}

pub struct AggregatorInput {
    /// False on the base case, where the verified Aggregator proof is a dummy.
    pub continuing: bool,
    pub agg_verifier_data_hash: HashOut,
    pub shrunk_verifier_data: VerifierOnlyCircuitData,
    pub slots: Vec<AggregatorSlot>,
}

struct AggregatorSlotTarget {
    proof: ProofWithPublicInputsTarget<D>,
    enabled: BoolTarget,
    transition: MerkleTreeStateTransitionProofTarget,
}

/// The fields of a state transition that the Aggregator constrains.
struct TransitionClaim {
    old_root: HashOutTarget,
    new_root: HashOutTarget,
    op_value: [Target; HASH_SIZE],
    op: Target,
}

impl TransitionClaim {
    /// Reads the fields through the flattened claim, laid out as
    /// `old_root | new_root | op_key | op_value | op`, since pod2 keeps them private.
    fn new(transition: &MerkleTreeStateTransitionProofTarget) -> Self {
        let flattened = MerkleTreeStateTransitionClaimTarget::from(transition.clone()).flatten();
        assert_eq!(flattened.len(), 4 * HASH_SIZE + 1);
        let hash_at = |index: usize| {
            HashOutTarget::from_vec(flattened[index * HASH_SIZE..(index + 1) * HASH_SIZE].to_vec())
        };
        Self {
            old_root: hash_at(0),
            new_root: hash_at(1),
            op_value: hash_at(3).elements,
            op: flattened[4 * HASH_SIZE],
        }
    }
}

pub struct AggregatorInnerCircuit {
    continuing: BoolTarget,
    agg_verifier_data_hash: HashOutTarget,
    shrunk_verifier_data: VerifierCircuitTarget,
    slots: Vec<AggregatorSlotTarget>,
}

fn select_hash(
    builder: &mut CircuitBuilder,
    condition: BoolTarget,
    if_true: HashOutTarget,
    if_false: HashOutTarget,
) -> HashOutTarget {
    HashOutTarget {
        elements: std::array::from_fn(|i| {
            builder.select(condition, if_true.elements[i], if_false.elements[i])
        }),
    }
}

fn conditional_assert_eq_hash(
    builder: &mut CircuitBuilder,
    condition: BoolTarget,
    left: [Target; HASH_SIZE],
    right: [Target; HASH_SIZE],
) {
    for (left, right) in left.into_iter().zip(right) {
        builder.conditional_assert_eq(condition.target, left, right);
    }
}

impl InnerCircuit for AggregatorInnerCircuit {
    type Input = AggregatorInput;
    type Params = AggregatorParams;

    fn build(
        builder: &mut CircuitBuilder,
        params: &Self::Params,
        verified_proofs: &[VerifiedProofTarget],
    ) -> Result<Self> {
        assert_eq!(verified_proofs.len(), 1);
        let previous = &verified_proofs[0];

        let continuing = builder.add_virtual_bool_target_safe();
        let agg_verifier_data_hash = builder.add_virtual_hash();
        let previous_root =
            HashOutTarget::from_vec(previous.public_inputs[AGG_ROOT_RANGE].to_vec());
        let previous_agg_verifier_data_hash =
            HashOutTarget::from_vec(previous.public_inputs[AGG_VERIFIER_DATA_HASH_RANGE].to_vec());
        conditional_assert_eq_hash(
            builder,
            continuing,
            previous.verifier_data_hash.elements,
            agg_verifier_data_hash.elements,
        );
        conditional_assert_eq_hash(
            builder,
            continuing,
            previous_agg_verifier_data_hash.elements,
            agg_verifier_data_hash.elements,
        );
        let empty_root = builder.constant_hash(EMPTY_HASH.into());
        let mut root = select_hash(builder, continuing, previous_root, empty_root);

        let shrunk_verifier_data = builder
            .add_virtual_verifier_data(params.shrunk_common_data.config.fri_config.cap_height);
        let shrunk_verifier_data_hash = hash_verifier_data_gadget(builder, &shrunk_verifier_data);
        let expected_shrunk_verifier_data_hash =
            builder.constant_hash(params.shrunk_verifier_data_hash);
        builder.connect_hashes(
            shrunk_verifier_data_hash,
            expected_shrunk_verifier_data_hash,
        );

        let insert_op = builder.constant(F::from_canonical_u8(MerkleTreeOp::Insert as u8));
        let mut slots = Vec::with_capacity(params.num_input_pods);
        for _ in 0..params.num_input_pods {
            let proof = builder.add_virtual_proof_with_pis(&params.shrunk_common_data);
            builder.verify_proof::<C>(&proof, &shrunk_verifier_data, &params.shrunk_common_data);
            let enabled = builder.add_virtual_bool_target_safe();
            let leaf = builder.hash_n_to_hash_no_pad::<PoseidonHash>(proof.public_inputs.clone());

            let transition =
                MerkleTreeStateTransitionProofTarget::new_virtual(AGG_TREE_DEPTH, builder);
            verify_merkle_state_transition_circuit(builder, &transition);
            let claim = TransitionClaim::new(&transition);
            builder.conditional_assert_eq(enabled.target, claim.op, insert_op);
            conditional_assert_eq_hash(builder, enabled, claim.old_root.elements, root.elements);
            conditional_assert_eq_hash(builder, enabled, claim.op_value, leaf.elements);
            root = select_hash(builder, enabled, claim.new_root, root);

            slots.push(AggregatorSlotTarget {
                proof,
                enabled,
                transition,
            });
        }

        builder.register_public_inputs(&root.elements);
        builder.register_public_inputs(&agg_verifier_data_hash.elements);

        Ok(Self {
            continuing,
            agg_verifier_data_hash,
            shrunk_verifier_data,
            slots,
        })
    }

    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &Self::Input) -> Result<()> {
        assert_eq!(input.slots.len(), self.slots.len());
        pw.set_bool_target(self.continuing, input.continuing)?;
        pw.set_hash_target(self.agg_verifier_data_hash, input.agg_verifier_data_hash)?;
        pw.set_verifier_data_target(&self.shrunk_verifier_data, &input.shrunk_verifier_data)?;
        for (target, slot) in self.slots.iter().zip(&input.slots) {
            pw.set_proof_with_pis_target(&target.proof, &slot.proof)?;
            pw.set_bool_target(target.enabled, slot.enabled)?;
            target.transition.set_targets(pw, &slot.transition)?;
        }
        Ok(())
    }
}
