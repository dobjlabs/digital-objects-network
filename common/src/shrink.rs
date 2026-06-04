//! Check out common/src/lib.rs documentation for context.
//!

use std::time::Instant;

use anyhow::Result;
use plonky2::{
    iop::witness::{PartialWitness, WitnessWrite},
    plonk::{
        circuit_data::CircuitConfig,
        proof::{CompressedProof, ProofWithPublicInputsTarget},
    },
};
use pod2::{
    backends::plonky2::{
        basetypes::{CircuitBuilder, CircuitData, Proof, ProofWithPublicInputs},
        mainpod::{
            cache_get_rec_main_pod_common_circuit_data,
            cache_get_rec_main_pod_verifier_circuit_data, public_inputs,
        },
        serialization::{CommonCircuitDataSerializer, VerifierCircuitDataSerializer},
    },
    cache,
    cache::CacheEntry,
    middleware::{C, CommonCircuitData, D, F, Params, VerifierCircuitData},
};
use tracing::info;

pub struct ShrunkMainPodSetup {
    params: Params,
    main_pod_common_circuit_data: CommonCircuitData,
    main_pod_verifier_circuit_data: VerifierCircuitData,
}

pub struct ShrunkMainPodBuild {
    pub params: Params,
    pub main_pod_verifier_circuit_data: VerifierCircuitData,
    pub shrunk_main_pod: ShrunkMainPodTarget,
    pub circuit_data: CircuitData,
}

impl ShrunkMainPodSetup {
    pub fn new(params: &Params) -> Self {
        let common_circuit_data = cache_get_rec_main_pod_common_circuit_data(params);
        let verifier_circuit_data = cache_get_rec_main_pod_verifier_circuit_data(params);
        Self {
            params: params.clone(),
            main_pod_common_circuit_data: (**common_circuit_data).clone(),
            main_pod_verifier_circuit_data: (**verifier_circuit_data).clone(),
        }
    }
}

pub struct ShrunkMainPodTarget {
    proof_with_pis_target: ProofWithPublicInputsTarget<D>,
}

impl ShrunkMainPodTarget {
    pub fn set_targets(
        &self,
        pw: &mut PartialWitness<F>,
        proof_with_public_inputs: &ProofWithPublicInputs,
    ) -> Result<()> {
        pw.set_proof_with_pis_target(&self.proof_with_pis_target, proof_with_public_inputs)?;
        Ok(())
    }
}

impl ShrunkMainPodSetup {
    pub fn new_virtual(&self, builder: &mut CircuitBuilder) -> ShrunkMainPodTarget {
        let proof_with_pis_target =
            builder.add_virtual_proof_with_pis(&self.main_pod_common_circuit_data);
        ShrunkMainPodTarget {
            proof_with_pis_target,
        }
    }

    pub fn verify_shrunk_mainpod_circuit(
        &self,
        builder: &mut CircuitBuilder,
        shrunk_main_pod: &ShrunkMainPodTarget,
    ) -> Result<()> {
        // create circuit logic
        let verifier_circuit_target =
            builder.constant_verifier_data(&self.main_pod_verifier_circuit_data.verifier_only);
        builder.verify_proof::<C>(
            &shrunk_main_pod.proof_with_pis_target,
            &verifier_circuit_target,
            &self.main_pod_common_circuit_data,
        );

        builder.register_public_inputs(&shrunk_main_pod.proof_with_pis_target.public_inputs);

        Ok(())
    }

    pub fn build(&self) -> Result<ShrunkMainPodBuild> {
        let config = CircuitConfig::standard_recursion_config();
        let mut builder = CircuitBuilder::new(config);
        let shrunk_main_pod = self.new_virtual(&mut builder);
        self.verify_shrunk_mainpod_circuit(&mut builder, &shrunk_main_pod)?;
        let circuit_data = builder.build::<C>();
        Ok(ShrunkMainPodBuild {
            params: self.params.clone(),
            main_pod_verifier_circuit_data: self.main_pod_verifier_circuit_data.clone(),
            shrunk_main_pod,
            circuit_data,
        })
    }
}

impl ShrunkMainPodBuild {
    /// computes the one extra recirsive proof from the given MainPod's proof in
    /// order to shrink it, returns the shrank proof with the public inputs
    pub fn prove(&self, pod: pod2::frontend::MainPod) -> Result<ProofWithPublicInputs> {
        assert_eq!(pod.pod.params(), &self.params);
        assert_eq!(
            pod.pod.verifier_data(),
            self.main_pod_verifier_circuit_data.verifier_only
        );

        // generate first MainPod's proof
        let pod_proof: Proof = pod.pod.proof();
        let pod_proof_with_pis = ProofWithPublicInputs {
            proof: pod_proof.clone(),
            public_inputs: public_inputs(
                pod.pod.statements_root(),
                pod.pod.vd_set().root(),
                pod.pod.is_main(),
            ),
        };

        // shrink the MainPod's proof, obtaining a smaller plonky2 proof
        let start = Instant::now();
        let mut pw = PartialWitness::new();
        self.shrunk_main_pod
            .set_targets(&mut pw, &pod_proof_with_pis)?;
        let proof = self.circuit_data.prove(pw)?;
        info!("[TIME] shrunk MainPod proof took: {:?}", start.elapsed());

        // sanity check: verify proof
        self.circuit_data.verify(proof.clone())?;

        Ok(proof)
    }
}

/// first it shrinks the given MainPod's proof, and then compresses it,
/// returning the compressed proof (without public inputs)
pub fn shrink_compress_pod(
    shrunk_main_pod_build: &ShrunkMainPodBuild,
    pod: pod2::frontend::MainPod,
) -> Result<CompressedProof<F, C, D>> {
    // generate new plonky2 proof from POD's proof. This is 1 extra recursion in
    // order to shrink the proof size, together with removing extra custom gates
    let start = Instant::now();
    let proof_with_pis = shrunk_main_pod_build.prove(pod)?;
    // let (verifier_data, common_circuit_data, proof_with_pis) = prove_pod(pod)?;
    info!("[TIME] plonky2 (wrapper) proof took: {:?}", start.elapsed());

    // this next line performs the method `fri_query_indices`, which is not exposed
    let indices = proof_with_pis
        .get_challenges(
            proof_with_pis.get_public_inputs_hash(),
            &shrunk_main_pod_build
                .circuit_data
                .verifier_only
                .circuit_digest,
            &shrunk_main_pod_build.circuit_data.common,
        )?
        .fri_challenges
        .fri_query_indices;
    let compressed_proof = proof_with_pis.proof.compress(
        &indices,
        &shrunk_main_pod_build.circuit_data.common.fri_params,
    );
    Ok(compressed_proof)
}

/// Returns the shrunk MainPod circuit data from the pod2 disk cache, building it on first use.
/// The result contains the `CommonCircuitData` (for proof deserialization) and
/// `VerifierCircuitData` (for proof verification) of the shrunk wrapper circuit.
pub fn cache_get_shrunk_main_pod_circuit_data(
    params: &Params,
) -> CacheEntry<(CommonCircuitDataSerializer, VerifierCircuitDataSerializer)> {
    cache::get("shrunk_main_pod_circuit_data", params, |params| {
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(params)
            .build()
            .expect("successful build");
        let verifier = shrunk_main_pod_build.circuit_data.verifier_data();
        let common = shrunk_main_pod_build.circuit_data.common;
        (
            CommonCircuitDataSerializer(common),
            VerifierCircuitDataSerializer(verifier),
        )
    })
    .expect("cache ok")
}
