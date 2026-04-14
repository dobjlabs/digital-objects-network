//! VdfPod: Introduction Pod that used as a "Verifiable Delay Function".
//! - takes as input a custom value, which will be bounded into the recursive chain
//! - counts how many recursions have been performed
//!
//! The 'delay' comes from the proof computation cost at the each recursive step.
//!
//! An other option would be to prove the traditional Vdf (hash output within a
//! range / certain amount of zeroes) inside a circuit, which is easier to
//! parallelize to gain advantatge.
//!
//! Circuits structure:
//! 1. RecursiveCircuit<VdfInneCircuit>, where for each recursive step:
//!
//!   VdfInnerCircuit contains the logic of:
//!     - output = hash(input)
//!     - count+1
//!
//!   And the RecursiveCircuit does the logic of:
//!     - verify previous proof of itself
//!
//! 2. VdfPod:
//!     - satisfies in the pod2's Pod trait interface
//!     - verifies the proof from RecursiveCircuit<VdfInnerCircuit>
//!
//!
//! Usage:
//! ```rust,no_run
//!   use pod2::{backends::plonky2::basetypes::DEFAULT_VD_SET, middleware::{Params, RawValue, hash_str}};
//!   use vdfpod::VdfPod;
//!
//!   let params = Params::default();
//!   let vd_set = &*DEFAULT_VD_SET;
//!   let n_iters: usize = 2;
//!   let input = RawValue::from(hash_str("starting input"));
//!   let vdf_pod = VdfPod::new(&params, vd_set.clone(), n_iters, input).unwrap();
//! ```
//! An complete example of usage can be found at the test `test_vdf_pod` (bottom
//! of this file).

use anyhow::{Result, anyhow};
use itertools::Itertools;
use plonky2::{
    field::types::Field,
    hash::{
        hash_types::{HashOut, HashOutTarget},
        poseidon::PoseidonHash,
    },
    iop::{
        target::Target,
        witness::{PartialWitness, WitnessWrite},
    },
    plonk::{
        circuit_builder::CircuitBuilder,
        circuit_data::{CircuitData, VerifierOnlyCircuitData},
        proof::{ProofWithPublicInputs, ProofWithPublicInputsTarget},
    },
};
use pod2::{
    backends::plonky2::{
        Error, Result as BResult,
        circuits::{
            common::{
                CircuitBuilderPod, PredicateTarget, StatementArgTarget, StatementTarget,
                ValueTarget,
            },
            mainpod::calculate_statements_hash_circuit,
        },
        deserialize_proof, mainpod,
        mainpod::calculate_statements_hash,
        recursion::{
            InnerCircuit, RecursiveCircuit, RecursiveParams, VerifiedProofTarget,
            circuit::{dummy as dummy_recursive, hash_verifier_data_gadget},
            new_params as new_recursive_params,
        },
        serialization::CircuitDataSerializer,
        serialize_proof,
    },
    cache::{self, CacheEntry},
    measure_gates_begin, measure_gates_end, middleware,
    middleware::{
        C, D, EMPTY_HASH, F, HASH_SIZE, Hash, IntroPredicateRef, Params, Pod, Proof, RawValue,
        ToFields, VDSet, Value,
    },
    timed,
};
use pod2utils::mockintro::MockIntroPod;
use serde::{Deserialize, Serialize};

// ARITY is assumed to be one, this also assumed at the VdfInnerCircuit.
const ARITY: usize = 1;
const NUM_PUBLIC_INPUTS: usize = 13; // 13: count + input + output + verified_data_hash
const VDF_POD_TYPE: (usize, &str) = (2001, "Vdf");

pub static STANDARD_VDF_VD_HASH: std::sync::LazyLock<Hash> = std::sync::LazyLock::new(|| {
    let (_, data) = &**STANDARD_VDF_POD_DATA;
    let hash_out =
        pod2::backends::plonky2::recursion::circuit::hash_verifier_data(&data.verifier_only);
    Hash(hash_out.elements.map(|e| e))
});

static STANDARD_VDF_POD_DATA: std::sync::LazyLock<
    CacheEntry<(VdfPodTarget, CircuitDataSerializer)>,
> = std::sync::LazyLock::new(|| {
    cache::get("standard_vdf_pod_circuit_data", &(), |_| {
        let (target, circuit_data) = build().expect("successful build");
        (target, CircuitDataSerializer(circuit_data))
    })
    .expect("cache ok")
});
fn build() -> Result<(VdfPodTarget, CircuitData<F, C, D>)> {
    let params = Params::default();

    // use pod2's recursion config as config for the introduction pod; which if
    // the zk feature enabled, it will have the zk property enabled
    let rec_circuit_data =
        &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();

    let common_data = rec_circuit_data.0.clone();
    let config = common_data.config.clone();

    let mut builder = CircuitBuilder::<F, D>::new(config);
    let vdf_pod_verify_target = VdfPodTarget::add_targets(&mut builder, &params)?;
    pod2::backends::plonky2::recursion::pad_circuit(&mut builder, &common_data);

    let data = timed!("VdfPod build", builder.build::<C>());
    assert_eq!(common_data, data.common);
    Ok((vdf_pod_verify_target, data))
}
static VDF_RECURSIVE_CIRCUIT: std::sync::LazyLock<(
    RecursiveCircuit<VdfInnerCircuit>,
    RecursiveParams,
)> = std::sync::LazyLock::new(|| build_vdf_recursive_circuit().expect("successful build"));
fn build_vdf_recursive_circuit() -> Result<(RecursiveCircuit<VdfInnerCircuit>, RecursiveParams)> {
    let recursive_params: RecursiveParams =
        new_recursive_params::<VdfInnerCircuit>(ARITY, NUM_PUBLIC_INPUTS, &())?;

    let recursive_circuit = RecursiveCircuit::<VdfInnerCircuit>::build(&recursive_params, &())?;

    Ok((recursive_circuit, recursive_params))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VdfPod {
    pub params: Params,
    pub count: F,
    pub input: RawValue,
    pub output: RawValue, // output = H(H(H( ...H(input) ))) (count times)

    pub vd_set: VDSet,
    pub statements_hash: Hash,
    pub proof: Proof,

    pub common_hash: String,
}

#[allow(dead_code)]
impl VdfPod {
    pub fn new_boxed_mock(
        params: &Params,
        vd_set: VDSet,
        n_iters: usize,
        input: RawValue,
    ) -> Result<Box<dyn Pod>> {
        assert_eq!(params, &Params::default());
        let vd_hash = *STANDARD_VDF_VD_HASH;
        let mut output: Hash = Hash::from(input);
        for _ in 0..n_iters {
            output = pod2::middleware::hash_value(&RawValue::from(output))
        }
        let args = vec![
            Value::from(n_iters as i64),
            Value::from(input),
            Value::from(output),
        ];
        let name = VDF_POD_TYPE.1.to_string();
        Ok(Box::new(MockIntroPod::new(
            params, vd_set, name, vd_hash, args,
        )))
    }

    /// returns a VdfPod for the given n_iters and input.
    pub fn new(params: &Params, vd_set: VDSet, n_iters: usize, input: RawValue) -> Result<VdfPod> {
        assert_eq!(params, &Params::default());
        let (last_iteration_values, proof_with_pis): (
            VdfInnerCircuitInput,
            ProofWithPublicInputs<F, C, D>,
        ) = timed!(
            "VdfPod::gen_vdf_recursive_circuit_proof",
            VdfPod::get_vdf_recursive_circuit_proof(n_iters, input)?
        );

        // generate a new VdfPod from the given count, input, output
        let (count, input, output) = (
            last_iteration_values.count,
            last_iteration_values.input,
            last_iteration_values.output,
        );
        let vdf_pod = timed!(
            "VdfPod::construct",
            VdfPod::construct(params, vd_set, count, input, output, proof_with_pis)?
        );

        #[cfg(test)] // sanity check
        vdf_pod.verify()?;

        Ok(vdf_pod)
    }

    pub fn new_boxed(
        params: &Params,
        vd_set: VDSet,
        n_iters: usize,
        input: RawValue,
    ) -> Result<Box<dyn Pod>> {
        Ok(Box::new(Self::new(params, vd_set, n_iters, input)?))
    }

    /// given the proof from RecursiveCircuit<VdfInnerCircuit>, constructs the
    /// VdfPod which verifies it.
    fn construct(
        params: &Params,
        vd_set: VDSet,
        count: F,
        input: RawValue,
        output: RawValue,
        proof: ProofWithPublicInputs<F, C, D>,
    ) -> Result<VdfPod> {
        // verify the given proof in a VdfPodTarget circuit
        let (vdf_pod_target, circuit_data) = &**STANDARD_VDF_POD_DATA;
        let statements = pub_self_statements(count, input, output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements);
        // set targets
        let pod_vdf_input = VdfPodVerifyInput {
            vd_root: vd_set.root(),
            statements_hash,
            proof,
        };
        let mut pw = PartialWitness::<F>::new();
        vdf_pod_target.set_targets(&mut pw, &pod_vdf_input)?;
        let proof_with_pis = timed!(
            "prove the vdf-verification proof verification (VdfPod proof)",
            circuit_data.prove(pw)?
        );
        // sanity check
        circuit_data
            .verifier_data()
            .verify(proof_with_pis.clone())?;

        let common_hash: String =
            pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash(params).clone();

        Ok(VdfPod {
            params: params.clone(),
            statements_hash,
            count,
            input,
            output,
            proof: proof_with_pis.proof,
            vd_set: vd_set.clone(),
            common_hash,
        })
    }

    /// computes the Vdf proof out of the RecursiveCircuit<VdfInnerCircuit> circuit.
    fn get_vdf_recursive_circuit_proof(
        n_iters: usize,
        starting_input: RawValue,
    ) -> Result<(VdfInnerCircuitInput, ProofWithPublicInputs<F, C, D>)> {
        if n_iters < 2 {
            // this check is due the verifier_data_hash behaving differently for
            // the first 2 iterations:
            // - if n_iters=0, is [0,0,0,0]
            // - if n_iters=1, is the one of the dummy_verifier_data
            // in both cases, when verifying the proof out of the recursive
            // chain in the VdfPod circuit, the verifier_data_hash would not
            // match the one expected (hardcoded as constant) at the VdfPod
            // circuit.
            return Err(anyhow!("n_iters must be equal or greater than 2"));
        }

        let mut inner_inputs = VdfInnerCircuitInput {
            prev_count: F::ZERO,
            count: F::ONE,
            input: starting_input,
            midput: starting_input, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&starting_input)),
        };

        let (recursive_circuit, recursive_params) = &*VDF_RECURSIVE_CIRCUIT;

        let (dummy_verifier_only_data, dummy_proof) =
            dummy_recursive(recursive_params.common_data(), NUM_PUBLIC_INPUTS)?;
        let mut recursive_proof = dummy_proof;
        let mut recursive_verifier_only_data = dummy_verifier_only_data;
        for i in 0..n_iters {
            if i > 0 {
                inner_inputs.prev_count = inner_inputs.count;
                inner_inputs.count += F::ONE;
                inner_inputs.midput = inner_inputs.output;
                inner_inputs.output =
                    RawValue::from(pod2::middleware::hash_value(&inner_inputs.midput));

                recursive_verifier_only_data =
                    recursive_params.verifier_data().verifier_only.clone();
            }
            log::debug!("{inner_inputs:?}");
            log::debug!("{:?}", recursive_proof.public_inputs);

            recursive_proof = recursive_circuit.prove(
                &inner_inputs,
                vec![recursive_proof.clone()],
                vec![recursive_verifier_only_data.clone()],
            )?;
            recursive_params
                .verifier_data()
                .verify(recursive_proof.clone())?;
        }
        Ok((inner_inputs, recursive_proof))
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    count: F,
    input: RawValue,
    output: RawValue,
    proof: String,
    common_hash: String,
}

impl Pod for VdfPod {
    fn params(&self) -> &Params {
        &self.params
    }
    fn verify(&self) -> pod2::backends::plonky2::Result<()> {
        let statements = pub_self_statements(self.count, self.input, self.output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements);
        if statements_hash != self.statements_hash {
            return Err(Error::statements_hash_not_equal(
                self.statements_hash,
                statements_hash,
            ));
        }

        let (_, circuit_data) = &**STANDARD_VDF_POD_DATA;

        let public_inputs = statements_hash
            .to_fields()
            .iter()
            .chain(self.vd_set().root().0.iter())
            .cloned()
            .collect_vec();

        circuit_data
            .verify(ProofWithPublicInputs {
                proof: self.proof.clone(),
                public_inputs,
            })
            .map_err(|e| Error::custom(format!("VdfPod proof verification failure: {e:?}")))
    }

    fn statements_hash(&self) -> Hash {
        self.statements_hash
    }

    fn pod_type(&self) -> (usize, &'static str) {
        VDF_POD_TYPE
    }

    fn pub_self_statements(&self) -> Vec<middleware::Statement> {
        // exposed as a separate function for easier isolated testing
        pub_self_statements(self.count, self.input, self.output)
    }

    fn serialize_data(&self) -> serde_json::Value {
        serde_json::to_value(Data {
            count: self.count,
            input: self.input,
            output: self.output,
            proof: serialize_proof(&self.proof),
            common_hash: self.common_hash.clone(),
        })
        .expect("serialization to json")
    }
    fn deserialize_data(
        params: Params,
        data: serde_json::Value,
        vd_set: VDSet,
        statements_hash: Hash,
    ) -> BResult<Self> {
        let data: Data = serde_json::from_value(data)?;
        let common =
            &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();
        let proof = deserialize_proof(common, &data.proof)?;
        Ok(Self {
            params,
            count: data.count,
            input: data.input,
            output: data.output,
            vd_set,
            statements_hash,
            proof,
            common_hash: data.common_hash,
        })
    }

    fn verifier_data(&self) -> VerifierOnlyCircuitData<C, D> {
        let (_, circuit_data) = &**STANDARD_VDF_POD_DATA;
        circuit_data.verifier_data().verifier_only.clone()
    }

    fn common_hash(&self) -> String {
        self.common_hash.clone()
    }
    fn proof(&self) -> Proof {
        self.proof.clone()
    }
    fn vd_set(&self) -> &VDSet {
        &self.vd_set
    }
}

fn pub_self_statements(count: F, input: RawValue, output: RawValue) -> Vec<middleware::Statement> {
    vec![middleware::Statement::Intro(
        IntroPredicateRef {
            name: VDF_POD_TYPE.1.to_string(),
            args_len: 3,
            verifier_data_hash: EMPTY_HASH,
        },
        vec![
            RawValue([count, F::ZERO, F::ZERO, F::ZERO]).into(),
            input.into(),
            output.into(),
        ],
    )]
}
fn pub_self_statements_target(
    builder: &mut CircuitBuilder<F, D>,
    params: &Params,
    count: Target,
    input: &[Target],
    output: &[Target],
) -> Vec<StatementTarget> {
    let zero = builder.zero();
    let st_arg_0 = StatementArgTarget::literal(
        builder,
        &ValueTarget::from_slice(&[count, zero, zero, zero]),
    );
    let st_arg_1 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(input));
    let st_arg_2 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(output));
    let args = [st_arg_0, st_arg_1, st_arg_2]
        .into_iter()
        .chain(core::iter::repeat_with(|| {
            StatementArgTarget::none(builder)
        }))
        .take(Params::max_statement_args())
        .collect_vec();

    let verifier_data_hash = builder.constant_hash(HashOut {
        elements: EMPTY_HASH.0,
    });
    let predicate = PredicateTarget::new_intro(builder, verifier_data_hash);
    vec![StatementTarget::new_with_pred(
        builder, params, predicate, &args,
    )]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VdfPodTarget {
    vd_root: HashOutTarget,
    statements_hash: HashOutTarget,
    proof: ProofWithPublicInputsTarget<D>,
}
struct VdfPodVerifyInput {
    vd_root: Hash,
    statements_hash: Hash,
    proof: ProofWithPublicInputs<F, C, D>,
}
impl VdfPodTarget {
    fn add_targets(builder: &mut CircuitBuilder<F, D>, params: &Params) -> Result<Self> {
        let measure = measure_gates_begin!(builder, "VdfPodTarget");

        // Verify RecursiveCircuit<VdfInnerCircuit>'s proof (with verifier_data hardcoded as constant)
        let (_, recursive_params) = &*VDF_RECURSIVE_CIRCUIT;
        let verifier_data_targ =
            builder.constant_verifier_data(&recursive_params.verifier_data().verifier_only);
        let proof = builder.add_virtual_proof_with_pis(recursive_params.common_data());
        builder.verify_proof::<C>(&proof, &verifier_data_targ, recursive_params.common_data());

        // ensure that the verifier_data_hash that appears at the public inputs
        // of the proof being verified matches the one that is constant
        let pi_verifier_data_hash = &proof.public_inputs[9..13];
        let constant_verifier_data_hash = hash_verifier_data_gadget(builder, &verifier_data_targ);
        #[allow(clippy::needless_range_loop)] // to use same syntax as in other similar circuits
        for i in 0..HASH_SIZE {
            builder.connect(
                pi_verifier_data_hash[i],
                constant_verifier_data_hash.elements[i],
            );
        }

        // calculate statements_hash
        let count = proof.public_inputs[0];
        let input = &proof.public_inputs[1..5];
        let output = &proof.public_inputs[5..9];
        let statements = pub_self_statements_target(builder, params, count, input, output);
        let statements_hash = calculate_statements_hash_circuit(builder, &statements);

        // register the public inputs
        let vd_root = builder.add_virtual_hash();
        builder.register_public_inputs(&statements_hash.elements);
        builder.register_public_inputs(&vd_root.elements);

        measure_gates_end!(builder, measure);
        Ok(VdfPodTarget {
            vd_root,
            statements_hash,
            proof,
        })
    }

    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &VdfPodVerifyInput) -> Result<()> {
        pw.set_proof_with_pis_target(&self.proof, &input.proof)?;
        pw.set_hash_target(
            self.statements_hash,
            HashOut::from_vec(input.statements_hash.0.to_vec()),
        )?;
        pw.set_target_arr(&self.vd_root.elements, &input.vd_root.0)?;

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct VdfInnerCircuit {
    prev_count: Target,
    count: Target,       // count contains the amount of recursive steps done
    input: ValueTarget,  // input that is bounded into the recursive chain
    midput: ValueTarget, // midput is the 'input' used for the last step of the recursion
    output: ValueTarget, // output of the recursive chain
}
#[derive(Debug)]
struct VdfInnerCircuitInput {
    prev_count: F,
    count: F,
    input: RawValue,
    midput: RawValue,
    output: RawValue,
}
impl InnerCircuit for VdfInnerCircuit {
    type Input = VdfInnerCircuitInput;
    type Params = ();
    fn build(
        builder: &mut CircuitBuilder<F, D>,
        _params: &Self::Params,
        verified_proofs: &[VerifiedProofTarget],
    ) -> BResult<Self> {
        let prev_count = builder.add_virtual_target();
        let input = builder.add_virtual_value();
        let midput = builder.add_virtual_value();

        let output_h = builder.hash_n_to_hash_no_pad::<PoseidonHash>(midput.elements.to_vec());
        let output = ValueTarget::from_slice(output_h.elements.as_ref());

        let zero = builder.zero();
        let one = builder.one();

        let is_basecase = builder.is_equal(prev_count, zero); // case 0
        let is_not_basecase = builder.not(is_basecase);
        let is_case_1 = builder.is_equal(prev_count, one); // case 1
        let case_0_or_1 = builder.or(is_basecase, is_case_1);
        let after_case_1 = builder.not(case_0_or_1);

        // if we're at the prev_count==0, ensure that
        // input==midput
        for i in 0..HASH_SIZE {
            builder.conditional_assert_eq(
                is_basecase.target,
                input.elements[i],
                midput.elements[i],
            );
        }

        // if we're at case prev_count>0, assert that the public_inputs of the
        // proof being verified match with the prev_count, input and midput.
        // For prev_count>1, we also check that the verifier_data_hash being
        // used matches the one at the public_inputs of the previous proof.
        builder.connect(verified_proofs[0].public_inputs[0], prev_count);
        for i in 0..HASH_SIZE {
            // if prev_count>0:
            builder.conditional_assert_eq(
                is_not_basecase.target,
                verified_proofs[0].public_inputs[1 + i],
                input.elements[i],
            );
            builder.conditional_assert_eq(
                is_not_basecase.target,
                verified_proofs[0].public_inputs[5 + i],
                midput.elements[i],
            );

            // if we're at case prev_count>1:
            // check that the verifier_data's hash used to verify the current
            // proof is the same as in the public_inputs. Notice that at case 0,
            // this verifier_data_hash is [0,0,0,0], and at case 1 is the hash
            // of the dummy_verifier_data; hence we do this check when
            // prev_count>1.
            builder.conditional_assert_eq(
                after_case_1.target,
                verified_proofs[0].public_inputs[9 + i],
                verified_proofs[0].verifier_data_hash.elements[i],
            );
        }

        // increment count
        let count = builder.add(prev_count, one);

        // register public inputs: count, input, output
        builder.register_public_input(count);
        builder.register_public_inputs(&input.elements);
        builder.register_public_inputs(&output.elements);
        builder.register_public_inputs(&verified_proofs[0].verifier_data_hash.elements);

        Ok(Self {
            prev_count,
            count,
            input,
            midput,
            output,
        })
    }
    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &Self::Input) -> BResult<()> {
        pw.set_target(self.prev_count, input.prev_count)?;
        pw.set_target(self.count, input.count)?;
        pw.set_target_arr(&self.input.elements, &input.input.0)?;
        pw.set_target_arr(&self.midput.elements, &input.midput.0)?;
        pw.set_target_arr(&self.output.elements, &input.output.0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use plonky2::plonk::circuit_data::CircuitConfig;
    use pod2::{
        backends::plonky2::{basetypes::DEFAULT_VD_SET, recursion::circuit::hash_verifier_data},
        frontend, measure_gates_print,
        middleware::{Value, hash_str},
    };

    use super::*;

    // For tests only. Returns a valid VerifiedProofTarget filled with the
    // public_inputs from the given VdfInnerCircuitInput, in order to run some
    // tests.
    fn empty_verified_proof_target(
        builder: &mut CircuitBuilder<F, D>,
        inp: &VdfInnerCircuitInput,
    ) -> VerifiedProofTarget {
        let count = builder.constant(inp.prev_count);
        let input = builder.constants(&inp.input.0);
        let midput = if inp.prev_count.is_zero() {
            builder.constants(&inp.output.0)
        } else {
            builder.constants(&inp.midput.0)
        };
        let verifier_data_hash = HashOutTarget::from_partial(&[builder.zero()], builder.zero());
        VerifiedProofTarget {
            public_inputs: [
                vec![count],
                input,
                midput,
                verifier_data_hash.elements.to_vec(),
            ]
            .concat(),
            verifier_data_hash,
        }
    }

    #[ignore]
    #[test]
    fn test_inner_circuit() -> Result<()> {
        let inner_params = ();

        let starting_input = RawValue::from(hash_str("starting input"));

        // circuit
        let config = CircuitConfig::standard_recursion_zk_config();
        let mut builder = CircuitBuilder::<F, D>::new(config.clone());

        let inner_inputs = VdfInnerCircuitInput {
            prev_count: F::ZERO,
            count: F::ONE,
            input: starting_input,
            midput: starting_input, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&starting_input)),
        };

        // build circuit
        let measure = measure_gates_begin!(&builder, format!("VdfInnerCircuit gates"));
        let verified_proof_target = empty_verified_proof_target(&mut builder, &inner_inputs);
        let targets =
            VdfInnerCircuit::build(&mut builder, &inner_params, &[verified_proof_target])?;
        measure_gates_end!(&builder, measure);
        measure_gates_print!();
        let data = builder.build::<C>();

        // set witness
        let mut pw = PartialWitness::<F>::new();
        targets.set_targets(&mut pw, &inner_inputs)?;

        // generate & verify proof
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        // Second iteration
        let inner_inputs = VdfInnerCircuitInput {
            prev_count: F::ONE,
            count: F::from_canonical_u64(2u64),
            input: starting_input,
            midput: inner_inputs.output, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&inner_inputs.output)),
        };
        let mut builder = CircuitBuilder::<F, D>::new(config);
        let mut pw = PartialWitness::<F>::new();
        let verified_proof_target = empty_verified_proof_target(&mut builder, &inner_inputs);
        let targets =
            VdfInnerCircuit::build(&mut builder, &inner_params, &[verified_proof_target])?;
        targets.set_targets(&mut pw, &inner_inputs)?;
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }

    #[ignore]
    #[test]
    fn test_recursion_on_inner_circuit() -> Result<()> {
        let starting_input = RawValue::from(hash_str("starting input"));
        let _ = VdfPod::get_vdf_recursive_circuit_proof(3, starting_input)?;
        Ok(())
    }

    /// test to ensure that the pub_self_statements methods match between the
    /// in-circuit and the out-circuit implementations
    #[ignore]
    #[test]
    fn test_pub_self_statements_target() -> Result<()> {
        // first generate all the circuits data so that it does not need to be
        // computed at further stages of the test (affecting the time reports)
        timed!(
            "generate VDF_RECURSIVE_CIRCUIT, STANDARD_VDF_POD_DATA, STANDARD_REC_MAIN_POD_CIRCUIT",
            {
                let (_, _) = &*VDF_RECURSIVE_CIRCUIT;
                let (_, _) = &**STANDARD_VDF_POD_DATA;
                let _ =
                    &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data(
                    );
            }
        );

        let params = &Default::default();

        let count = F::ONE;
        let input = RawValue::from(hash_str("starting input"));
        let output = RawValue::from(pod2::middleware::hash_value(&input));

        let st = pub_self_statements(count, input, output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: HashOut<F> =
            HashOut::<F>::from_vec(calculate_statements_hash(&st).0.to_vec());

        // circuit
        let config = CircuitConfig::standard_recursion_config();
        let mut builder = CircuitBuilder::<F, D>::new(config);
        let mut pw = PartialWitness::<F>::new();

        // add targets
        let count_targ = builder.add_virtual_target();
        let input_targ = builder.add_virtual_value();
        let output_targ = builder.add_virtual_value();
        let expected_statements_hash_targ = builder.add_virtual_hash();

        // set values to targets
        pw.set_target(count_targ, count)?;
        pw.set_target_arr(&input_targ.elements, &input.0)?;
        pw.set_target_arr(&output_targ.elements, &output.0)?;
        pw.set_hash_target(expected_statements_hash_targ, statements_hash)?;

        let st_targ = pub_self_statements_target(
            &mut builder,
            params,
            count_targ,
            &input_targ.elements,
            &output_targ.elements,
        );
        let statements_hash_targ = calculate_statements_hash_circuit(&mut builder, &st_targ);

        builder.connect_hashes(expected_statements_hash_targ, statements_hash_targ);

        // generate & verify proof
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }

    #[ignore]
    #[test]
    fn test_vdf_pod() -> Result<()> {
        // for this test, first generate all the circuits data so that it does
        // not need to be computed at further stages of the test (affecting the
        // time reports)
        timed!(
            "generate VDF_RECURSIVE_CIRCUIT, STANDARD_VDF_POD_DATA, standard_rec_main_pod_common_circuit_data",
            {
                let (_, _) = &*VDF_RECURSIVE_CIRCUIT;
                let (_, _) = &**STANDARD_VDF_POD_DATA;
                let _ =
                    &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data(
                    );
            }
        );

        let params = Params::default();
        let n_iters: usize = 2;
        let input = RawValue::from(hash_str("starting input"));

        let vd_set = &*DEFAULT_VD_SET;
        let vdf_pod = timed!(
            "VdfPod::new",
            VdfPod::new(&params, vd_set.clone(), n_iters, input)?
        );
        vdf_pod.verify()?;

        println!(
            "vdf_pod.verifier_data_hash(): {:#} . To be used when importing the VdfPod as introduction pod to define new predicates.",
            vdf_pod.verifier_data_hash()
        );

        // wrap the vdf_pod in a 'MainPod'
        let main_vdf_pod = frontend::MainPod {
            pod: Box::new(vdf_pod.clone()),
            public_statements: vdf_pod.pub_statements(),
            params: params.clone(),
        };

        let expected_count = Value::from(n_iters as i64);
        let expected_input = input;

        // now generate a new MainPod from the vdf_pod
        let mut main_pod_builder = frontend::MainPodBuilder::new(&params, vd_set);
        main_pod_builder.add_pod(main_vdf_pod.clone())?;

        let _ = main_pod_builder.reveal(&main_vdf_pod.public_statements[0]);

        let prover = pod2::backends::plonky2::mock::mainpod::MockProver {};
        let pod = main_pod_builder.prove(&prover)?;
        assert!(pod.pod.verify().is_ok());

        println!("going to prove the main_pod");
        let prover = mainpod::Prover {};
        let main_pod = timed!("main_pod_builder.prove", main_pod_builder.prove(&prover)?);
        let pod: Box<mainpod::MainPod> = (main_pod.pod as Box<dyn std::any::Any>)
            .downcast::<mainpod::MainPod>()
            .unwrap();
        pod.verify()?;

        let st_vdf = pod.pub_statements()[0].clone();
        let count = st_vdf.args()[0].literal()?;
        let input = st_vdf.args()[1].literal()?;
        assert_eq!(count, expected_count);
        assert_eq!(input, Value::from(expected_input));

        Ok(())
    }

    #[test]
    fn test_vdf_vd_hash() {
        let (_, circuit_data) = &**STANDARD_VDF_POD_DATA;
        let expected_vd_hash = hash_verifier_data(&circuit_data.verifier_data().verifier_only);
        assert_eq!(expected_vd_hash, HashOut::from(*STANDARD_VDF_VD_HASH));
    }

    #[test]
    fn test_mock_vdf() {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let n_iters: usize = 2;
        let input = RawValue::from(hash_str("starting input"));

        let pod = VdfPod::new_boxed(&params, vd_set.clone(), n_iters, input).unwrap();
        pod.verify().unwrap();
        let mock_pod = VdfPod::new_boxed_mock(&params, vd_set.clone(), n_iters, input).unwrap();
        mock_pod.verify().unwrap();

        assert_eq!(pod.verifier_data_hash(), mock_pod.verifier_data_hash());
        assert_eq!(pod.pub_statements(), mock_pod.pub_statements());
    }
}
