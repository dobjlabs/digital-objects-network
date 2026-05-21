//! LtEqU256Pod: Introduction Pod that proves lower than for U256 values.
//! - takes as input two RawValues (lhs and rhs) that represent a U256 split into 4 limbs each
//! - proves that lhs <= rhs
//!
//! This can be used to prove that mining work was done to find a valid nonce/seed.
//!
//! Circuit structure:
//! 1. LtEqU256Circuit:
//!     - lhs: RawValue (4 field elements interpreted as 4 limbs of U256.  For PoW this is a hash/commitment)
//!     - rhs: RawValue (4 field elements interpreted as 4 limbs of U256.  For PoW this is the
//!       inverse of the difficulty)
//!     - proves: lhs <= rhs
//!
//! 2. LtEqU256Pod:
//!     - satisfies the pod2's Pod trait interface
//!     - verifies the proof from LtEqU256Circuit
//!
//! Usage:
//! ```rust,no_run
//!   use pod2::{backends::plonky2::basetypes::DEFAULT_VD_SET, middleware::{Params, RawValue, hash_str, F}};
//!   use lt_eq_u256_pod::LtEqU256Pod;
//!
//!   let params = Params::default();
//!   let vd_set = &*DEFAULT_VD_SET;
//!   let candidate_hash = RawValue::from(hash_str("block header + nonce"));
//!   let max_value = RawValue([F(0), F(0), F(0), F(0x0020_0000_0000_0000u64)]);
//!   let lt_eq_u256_pod = LtEqU256Pod::new(&params, vd_set.clone(), candidate_hash, max_value).unwrap();
//! ```

use anyhow::Result;
use plonky2::{
    field::types::{Field, PrimeField64},
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
        proof::ProofWithPublicInputs,
    },
};
use pod2::{
    backends::plonky2::{
        Error, Result as BResult,
        circuits::common::{
            CircuitBuilderPod, Flattenable, PredicateTarget, StatementArgTarget, StatementTarget,
            ValueTarget,
        },
        deserialize_proof,
        mainpod::public_inputs,
        primitives::merkletree::kv_hash_target,
        serialization::CircuitDataSerializer,
        serialize_proof,
    },
    cache::{self, CacheEntry},
    measure_gates_begin, measure_gates_end, middleware,
    middleware::{
        C, D, EMPTY_HASH, F, Hash, IntroPredicateRef, Params, Pod, Proof, RawValue, VDSet, Value,
        containers::Array,
    },
    timed,
};
use pod2utils::mockintro::MockIntroPod;
use serde::{Deserialize, Serialize};

const LT_EQ_U256_POD_TYPE: (usize, &str) = (2002, "LtEqU256");

pub static STANDARD_LT_EQ_U256_VD_HASH: std::sync::LazyLock<Hash> =
    std::sync::LazyLock::new(|| {
        let (_, data) = &**STANDARD_LT_EQ_U256_POD_DATA;
        let hash_out =
            pod2::backends::plonky2::recursion::circuit::hash_verifier_data(&data.verifier_only);
        Hash(hash_out.elements.map(|e| e))
    });

static STANDARD_LT_EQ_U256_POD_DATA: std::sync::LazyLock<
    CacheEntry<(LtEqU256PodTarget, CircuitDataSerializer)>,
> = std::sync::LazyLock::new(|| {
    cache::get("standard_lt_eq_u256_pod_circuit_data", &(), |_| {
        let (target, circuit_data) = build().expect("successful build");
        (target, CircuitDataSerializer(circuit_data))
    })
    .expect("cache ok")
});
fn build() -> Result<(LtEqU256PodTarget, CircuitData<F, C, D>)> {
    let params = Params::default();

    // use pod2's recursion config as config for the introduction pod; which if
    // the zk feature enabled, it will have the zk property enabled
    let rec_circuit_data =
        &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();

    let common_data = rec_circuit_data.0.clone();
    let config = common_data.config.clone();

    let mut builder = CircuitBuilder::<F, D>::new(config);
    let lt_eq_u256_pod_target = LtEqU256PodTarget::add_targets(&mut builder, &params)?;
    pod2::backends::plonky2::recursion::pad_circuit(&mut builder, &common_data);

    let data = timed!("LtEqU256Pod build", builder.build::<C>());
    assert_eq!(common_data, data.common);
    Ok((lt_eq_u256_pod_target, data))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LtEqU256Pod {
    pub params: Params,
    // verify lhs <= rhs
    pub lhs: RawValue,
    pub rhs: RawValue,

    pub vd_set: VDSet,
    pub proof: Proof,

    pub common_hash: String,
}

#[allow(dead_code)]
impl LtEqU256Pod {
    pub fn new_boxed_mock(
        params: &Params,
        vd_set: VDSet,
        lhs: RawValue,
        rhs: RawValue,
    ) -> Result<Box<dyn Pod>> {
        assert_eq!(params, &Params::default());
        let vd_hash = *STANDARD_LT_EQ_U256_VD_HASH;
        let args = vec![Value::from(lhs), Value::from(rhs)];
        let name = LT_EQ_U256_POD_TYPE.1.to_string();
        Ok(Box::new(MockIntroPod::new(
            params, vd_set, name, vd_hash, args,
        )))
    }

    pub fn new_boxed(
        params: &Params,
        vd_set: VDSet,
        lhs: RawValue,
        rhs: RawValue,
    ) -> Result<Box<dyn Pod>> {
        Ok(Box::new(Self::new(params, vd_set, lhs, rhs)?))
    }

    /// Creates a LtEqU256Pod proving that hash[0] <= difficulty
    pub fn new(
        params: &Params,
        vd_set: VDSet,
        lhs: RawValue,
        rhs: RawValue,
    ) -> Result<LtEqU256Pod> {
        assert_eq!(params, &Params::default());

        // Pre-check difficulty
        for (lhs_limb, rhs_limb) in lhs.0.iter().zip(rhs.0.iter()).rev() {
            if lhs_limb.0 > rhs_limb.0 {
                anyhow::bail!("lhs > rhs in LtEqU256");
            } else if lhs_limb.0 < rhs_limb.0 {
                break;
            }
        }

        // Build the proof
        let (lt_eq_u256_pod_target, circuit_data) = &**STANDARD_LT_EQ_U256_POD_DATA;

        // set targets
        let lt_eq_u256_input = LtEqU256PodInput {
            vd_root: vd_set.root(),
            lhs,
            rhs,
        };

        let mut pw = PartialWitness::<F>::new();
        lt_eq_u256_pod_target.set_targets(&mut pw, &lt_eq_u256_input)?;

        let proof_with_pis = timed!(
            "prove LtEqU256 (LtEqU256Pod proof)",
            circuit_data.prove(pw)?
        );

        // sanity check
        circuit_data
            .verifier_data()
            .verify(proof_with_pis.clone())?;

        let common_hash: String =
            pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash(params).clone();

        Ok(LtEqU256Pod {
            params: params.clone(),
            lhs,
            rhs,
            proof: proof_with_pis.proof,
            vd_set: vd_set.clone(),
            common_hash,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    lhs: RawValue,
    rhs: RawValue,
    proof: String,
    common_hash: String,
}

impl Pod for LtEqU256Pod {
    fn params(&self) -> &Params {
        &self.params
    }

    fn verify(&self) -> pod2::backends::plonky2::Result<()> {
        let sts_root = self.pub_raw_statements_mt().commitment();

        let (_, circuit_data) = &**STANDARD_LT_EQ_U256_POD_DATA;

        circuit_data
            .verify(ProofWithPublicInputs {
                proof: self.proof.clone(),
                public_inputs: public_inputs(sts_root, self.vd_set().root(), false),
            })
            .map_err(|e| Error::custom(format!("LtEqU256Pod proof verification failure: {e:?}")))
    }

    fn pod_type(&self) -> (usize, &'static str) {
        LT_EQ_U256_POD_TYPE
    }

    fn pub_raw_statements_mt(&self) -> Array {
        single_statement_mt(&pub_raw_statements(self.lhs, self.rhs)[0])
    }

    fn pub_raw_statements(&self) -> Vec<middleware::Statement> {
        pub_raw_statements(self.lhs, self.rhs)
    }

    fn serialize_data(&self) -> serde_json::Value {
        serde_json::to_value(Data {
            lhs: self.lhs,
            rhs: self.rhs,
            proof: serialize_proof(&self.proof),
            common_hash: self.common_hash.clone(),
        })
        .expect("serialization to json")
    }

    fn deserialize_data(params: Params, data: serde_json::Value, vd_set: VDSet) -> BResult<Self> {
        let data: Data = serde_json::from_value(data)?;
        let common =
            &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();
        let proof = deserialize_proof(common, &data.proof)?;
        Ok(Self {
            params,
            lhs: data.lhs,
            rhs: data.rhs,
            vd_set,
            proof,
            common_hash: data.common_hash,
        })
    }

    fn verifier_data(&self) -> VerifierOnlyCircuitData<C, D> {
        let (_, circuit_data) = &**STANDARD_LT_EQ_U256_POD_DATA;
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

fn single_statement_mt(st: &middleware::Statement) -> Array {
    Array::new(vec![Value::from(st.hash())])
}

fn pub_raw_statements(lhs: RawValue, rhs: RawValue) -> Vec<middleware::Statement> {
    vec![middleware::Statement::Intro(
        IntroPredicateRef {
            name: LT_EQ_U256_POD_TYPE.1.to_string(),
            args_len: 2,
            verifier_data_hash: EMPTY_HASH,
        },
        vec![lhs.into(), rhs.into()],
    )]
}

fn pub_raw_statements_target(
    builder: &mut CircuitBuilder<F, D>,
    params: &Params,
    lhs: &[Target],
    rhs: &[Target],
) -> Vec<StatementTarget> {
    let st_arg_0 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(lhs));
    let st_arg_1 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(rhs));

    let args = [st_arg_0, st_arg_1]
        .into_iter()
        .chain(core::iter::repeat_with(|| {
            StatementArgTarget::none(builder)
        }))
        .take(Params::max_statement_args())
        .collect::<Vec<_>>();

    let verifier_data_hash = builder.constant_hash(HashOut {
        elements: EMPTY_HASH.0,
    });
    let predicate = PredicateTarget::new_intro(builder, verifier_data_hash);
    vec![StatementTarget::new_with_pred(
        builder, params, predicate, &args,
    )]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LtEqU256PodTarget {
    vd_root: HashOutTarget,
    lhs: ValueTarget,
    rhs: ValueTarget,
}

struct LtEqU256PodInput {
    vd_root: Hash,
    lhs: RawValue,
    rhs: RawValue,
}

// assert lhs <= rhs for little endian encoded values with 32 bit limbs
fn assert_lt_eq<const N: usize>(
    builder: &mut CircuitBuilder<F, D>,
    lhs: &[Target; N],
    rhs: &[Target; N],
) {
    // Find the highest limb pair that is not equal and store the limb diff for that pair.
    // We then make sure that the diff fits within 32 bits, which means there's no underflow and
    // thus limb_lhs <= limb_rhs.
    let mut diff = builder.constant(F::ZERO);
    let mut diff_found = builder.constant_bool(false);
    for (lhs_limb, rhs_limb) in lhs.iter().zip(rhs.iter()).rev() {
        let limbs_eq = builder.is_equal(*lhs_limb, *rhs_limb);
        let limbs_neq = builder.not(limbs_eq);
        let diff_not_found = builder.not(diff_found);
        let is_first_neq = builder.and(limbs_neq, diff_not_found);
        diff_found = builder.or(diff_found, is_first_neq);
        let limbs_diff = builder.sub(*rhs_limb, *lhs_limb);
        diff = builder.select(is_first_neq, limbs_diff, diff);
    }
    builder.range_check(diff, 32);
}

fn value_to_32b_limbs(builder: &mut CircuitBuilder<F, D>, v: ValueTarget) -> [Target; 8] {
    let field_max = [
        builder.constant(F(F::NEG_ONE.to_canonical_u64() & 0xffffffff)),
        builder.constant(F(F::NEG_ONE.to_canonical_u64() >> 32)),
    ];
    let v_64b_limbs = v.elements.map(|t| builder.split_le(t, 64));
    let v_32b_limbs = v_64b_limbs.map(|bits| {
        let pair = [
            builder.le_sum(bits[..32].iter()),
            builder.le_sum(bits[32..].iter()),
        ];
        // Assert that the 64 bit representation is canonical
        assert_lt_eq(builder, &pair, &field_max);
        pair
    });
    std::array::from_fn(|i| v_32b_limbs[i / 2][i % 2])
}

impl LtEqU256PodTarget {
    fn add_targets(builder: &mut CircuitBuilder<F, D>, params: &Params) -> Result<Self> {
        let measure = measure_gates_begin!(builder, "LtEqU256PodTarget");

        // Add virtual inputs
        let lhs = builder.add_virtual_value();
        let rhs = builder.add_virtual_value();

        let lhs_bits = value_to_32b_limbs(builder, lhs);
        let rhs_bits = value_to_32b_limbs(builder, rhs);
        assert_lt_eq(builder, &lhs_bits, &rhs_bits);

        // Build the public statements Merkle root: a 1-element sparse array with the single
        // intro statement's hash at index 0. The MT root for that shape equals
        // kv_hash(key=0, value=st_hash).
        let statements = pub_raw_statements_target(builder, params, &lhs.elements, &rhs.elements);
        let st_hash = builder.hash_n_to_hash_no_pad::<PoseidonHash>(statements[0].flatten());
        let zero_key = ValueTarget::zero(builder);
        let st_hash_value = ValueTarget {
            elements: st_hash.elements,
        };
        let statements_root = kv_hash_target(builder, &zero_key, &st_hash_value);

        // Register public inputs: statements_root (4), vd_root (4), is_main (1)
        let vd_root = builder.add_virtual_hash();
        builder.register_public_inputs(&statements_root.elements);
        builder.register_public_inputs(&vd_root.elements);
        let is_main = builder._false();
        builder.register_public_input(is_main.target);

        measure_gates_end!(builder, measure);

        Ok(LtEqU256PodTarget { vd_root, lhs, rhs })
    }

    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &LtEqU256PodInput) -> Result<()> {
        pw.set_target_arr(&self.lhs.elements, &input.lhs.0)?;
        pw.set_target_arr(&self.rhs.elements, &input.rhs.0)?;
        pw.set_target_arr(&self.vd_root.elements, &input.vd_root.0)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use plonky2::{
        hash::hash_types::HashOut,
        plonk::{
            circuit_builder::CircuitBuilder, circuit_data::CircuitConfig,
            config::PoseidonGoldilocksConfig,
        },
    };
    use pod2::{
        backends::plonky2::{basetypes::DEFAULT_VD_SET, recursion::circuit::hash_verifier_data},
        middleware::hash_str,
    };

    use super::*;

    fn test_lt_eq_circuit(lhs: RawValue, rhs: RawValue) -> Result<()> {
        let config = CircuitConfig::standard_recursion_config();
        let mut builder = CircuitBuilder::<F, D>::new(config);

        let lhs_target = builder.add_virtual_value();
        let rhs_target = builder.add_virtual_value();

        let lhs_bits = value_to_32b_limbs(&mut builder, lhs_target);
        let rhs_bits = value_to_32b_limbs(&mut builder, rhs_target);
        assert_lt_eq(&mut builder, &lhs_bits, &rhs_bits);

        let mut pw = PartialWitness::<F>::new();
        pw.set_target_arr(&lhs_target.elements, &lhs.0)?;
        pw.set_target_arr(&rhs_target.elements, &rhs.0)?;

        let circuit_data = builder.build::<PoseidonGoldilocksConfig>();
        let proof = circuit_data.prove(pw)?;
        circuit_data.verify(proof)?;
        Ok(())
    }

    #[test]
    fn test_lt_eq_pass() {
        test_lt_eq_circuit(
            RawValue([F(0), F(0), F(0), F(0)]),
            RawValue([F(0), F(0), F(0), F(0)]),
        )
        .unwrap();

        test_lt_eq_circuit(
            RawValue([F(0), F(0), F(0), F(0)]),
            RawValue([F(1), F(0), F(0), F(0)]),
        )
        .unwrap();
        test_lt_eq_circuit(
            RawValue([F(0), F(0), F(0), F(0)]),
            RawValue([F(0), F(1), F(0), F(0)]),
        )
        .unwrap();
        test_lt_eq_circuit(
            RawValue([F(1), F(0), F(8), F(0)]),
            RawValue([F(0), F(1), F(8), F(0)]),
        )
        .unwrap();
    }

    #[test]
    fn test_lt_eq_fail() {
        assert!(
            test_lt_eq_circuit(
                RawValue([F(1), F(0), F(0), F(0)]),
                RawValue([F(0), F(0), F(0), F(0)]),
            )
            .is_err()
        );
        assert!(
            test_lt_eq_circuit(
                RawValue([F(0), F(1), F(0), F(0)]),
                RawValue([F(0), F(0), F(0), F(0)]),
            )
            .is_err()
        );
        assert!(
            test_lt_eq_circuit(
                RawValue([F(0), F(1), F(8), F(0)]),
                RawValue([F(1), F(0), F(8), F(0)]),
            )
            .is_err()
        );
    }

    #[test]
    fn test_lteq256_pod() -> Result<()> {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;

        // Find a valid input by brute force (for testing)
        let rhs_3 = 0x0020_0000_0000_0000u64;
        let mut found_input = None;

        for i in 0..10000 {
            let test_input = RawValue::from(i as i64);
            let hash_output = RawValue::from(pod2::middleware::hash_value(&test_input));
            if hash_output.0[3].0 <= rhs_3 {
                found_input = Some(test_input);
                println!("Found valid input at i={}: hash={:#}", i, hash_output);
                break;
            }
        }

        let hash_output = RawValue::from(pod2::middleware::hash_value(
            &found_input.expect("Should find valid input"),
        ));

        // This should succeed
        let lt_eq_u256_pod = LtEqU256Pod::new(
            &params,
            vd_set.clone(),
            hash_output,
            RawValue([F::ZERO, F::ZERO, F::ZERO, F(rhs_3)]),
        )?;
        lt_eq_u256_pod.verify()?;

        println!(
            "lt_eq_u256_pod.verifier_data_hash(): {:#} . To be used in predicates.",
            lt_eq_u256_pod.verifier_data_hash()
        );

        // Verify that hash_output meets difficulty
        assert!(hash_output.0[3].0 <= rhs_3);

        Ok(())
    }

    #[test]
    fn test_mock_ltequ256() -> Result<()> {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let rhs = RawValue([F::ZERO, F::ZERO, F::ZERO, F(0x0020_0000_0000_0000u64)]);
        let hash = RawValue::from(0i64);

        let pod = LtEqU256Pod::new_boxed(&params, vd_set.clone(), hash, rhs)?;
        pod.verify()?;
        let mock_pod = LtEqU256Pod::new_boxed_mock(&params, vd_set.clone(), hash, rhs)?;
        mock_pod.verify()?;

        assert!(mock_pod.is_mock());
        assert_eq!(pod.verifier_data_hash(), mock_pod.verifier_data_hash());
        assert_eq!(pod.pub_statements(), mock_pod.pub_statements());
        Ok(())
    }

    #[test]
    fn test_ltequ256_vd_hash() {
        let (_, circuit_data) = &**STANDARD_LT_EQ_U256_POD_DATA;
        let expected_vd_hash = hash_verifier_data(&circuit_data.verifier_data().verifier_only);
        assert_eq!(
            expected_vd_hash,
            HashOut::from(*STANDARD_LT_EQ_U256_VD_HASH)
        );
    }

    #[test]
    fn test_ltequ256_pod_fails_above_difficulty() -> Result<()> {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;

        let input = RawValue::from(hash_str("definitely above difficulty"));
        let rhs = RawValue([F::ZERO, F::ZERO, F::ZERO, F(0x0000_0000_0000_1000u64)]);

        // This should fail
        let result = LtEqU256Pod::new(&params, vd_set.clone(), input, rhs);
        assert!(result.is_err());

        Ok(())
    }

    /// Reproduce the dobjd "wire set twice" failure path: build an intro pod, build a MainPod
    /// that opens its statement publicly, then build a SECOND MainPod that opens that statement
    /// from the first MainPod's chain.
    #[ignore]
    #[test]
    fn test_chained_main_pod_with_intro_statement() -> Result<()> {
        use pod2::{backends::plonky2::mainpod, frontend};

        let _ = env_logger::builder().is_test(true).try_init();
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let prover = mainpod::Prover {};

        // 1. Build an intro pod and wrap it as a frontend MainPod.
        let lhs = RawValue([F(1), F(0), F(0), F(0)]);
        let rhs = RawValue([F(2), F(0), F(0), F(0)]);
        let intro_pod = LtEqU256Pod::new(&params, vd_set.clone(), lhs, rhs)?;
        let intro_st = intro_pod.pub_statements()[0].clone();
        let intro_main_pod = frontend::MainPod {
            pod: Box::new(intro_pod.clone()),
            public_statements: intro_pod.pub_statements(),
            params: params.clone(),
        };

        // 2. Build MainPod #1 that opens the intro statement publicly.
        println!("Building MainPod #1 (opens intro statement)");
        let mut builder1 = frontend::MainPodBuilder::new(&params, vd_set);
        builder1.add_pod(intro_main_pod)?;
        builder1.open_input_st(true, 0, &intro_st)?;
        let main_pod_1 = builder1.prove(&prover)?;
        main_pod_1.pod.verify()?;
        println!("MainPod #1 verified OK");

        // 3. Build MainPod #2 that opens the intro statement from MainPod #1's chain.
        println!("Building MainPod #2 (chains from MainPod #1)");
        let main_pod_1_pub_st = main_pod_1.public_statements[0].clone();
        let mut builder2 = frontend::MainPodBuilder::new(&params, vd_set);
        builder2.add_pod(main_pod_1)?;
        builder2.open_input_st(true, 0, &main_pod_1_pub_st)?;
        let main_pod_2 = builder2.prove(&prover)?;
        main_pod_2.pod.verify()?;
        println!("MainPod #2 verified OK");

        Ok(())
    }
}
