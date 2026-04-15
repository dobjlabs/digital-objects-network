//! Check out common/src/lib.rs documentation for context.
//!

use std::time::Instant;

use anyhow::{Result, anyhow};
use tracing::info;

const CACHE_SUBDIR_INPUT: &str = "pod2-groth16/plonky2-proof";
const CACHE_SUBDIR_OUTPUT: &str = "pod2-groth16/groth-artifacts";

/// Returns the platform cache directory (~/.cache on Linux, ~/Library/Caches on
/// macOS) joined with the given subdirectory. Creates the path if it doesn't
/// exist.
fn cache_path(subdir: &str) -> Result<String> {
    let base =
        dirs::cache_dir().ok_or_else(|| anyhow!("could not determine user cache directory"))?;
    let path = base.join(subdir);
    std::fs::create_dir_all(&path)
        .map_err(|e| anyhow!("failed to create cache dir {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// initializes the groth16 prover memory, loading the artifacts. This method
/// must be called before the `prove` method.
pub fn init() -> Result<()> {
    let input = cache_path(CACHE_SUBDIR_INPUT)?;
    let output = cache_path(CACHE_SUBDIR_OUTPUT)?;
    pod2_onchain::init(&input, &output)?;
    Ok(())
}

pub fn load_vk() -> Result<()> {
    let output = cache_path(CACHE_SUBDIR_OUTPUT)?;
    pod2_onchain::load_vk(&output)?;
    Ok(())
}

/// computes the one extra recursive proof from the given MainPod's proof in
/// order to shrink it, together with using the bn254's poseidon variant in the
/// configuration of the plonky2 prover, in order to make it compatible with the
/// Groth16 circuit.
/// Returns the Groth16 proof, and the Public Inputs, both in their byte-array
/// representation.
pub fn prove(pod: pod2::frontend::MainPod) -> Result<(Vec<u8>, Vec<u8>)> {
    let start = Instant::now();
    // generate new plonky2 proof (groth16's friendly kind) from POD's proof
    let (_, _, proof_with_pis) = pod2_onchain::prove_pod(pod)?;
    info!(
        "[TIME] plonky2 proof (groth16-friendly) took: {:?}",
        start.elapsed()
    );

    // assuming that the trusted setup & r1cs are in place, generate the Groth16
    // proof
    let (g16_proof, g16_pub_inp) = pod2_onchain::groth16_prove(proof_with_pis)?;

    Ok((g16_proof, g16_pub_inp))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;

    use super::*;

    fn gen_trusted_setup() -> Result<()> {
        let input_path = cache_path(CACHE_SUBDIR_INPUT)?;
        let output_path = cache_path(CACHE_SUBDIR_OUTPUT)?;

        // if plonky2 groth16-friendly proof does not exist yet, generate it
        let proof_marker = Path::new(&input_path).join("proof_with_public_inputs.bin");
        if !proof_marker.exists() {
            println!("generating plonky2 groth16-friendly proof at {input_path}");
            pod2_onchain::pod::sample_plonky2_g16_friendly_proof(&input_path)?;
        } else {
            println!("plonky2 groth16-friendly proof already exists at {input_path}, skipping");
        }

        // if trusted setup does not exist yet, generate it
        let vk_marker = Path::new(&output_path).join("verifying.key");
        if !vk_marker.exists() {
            println!("generating groth16's trusted setup at {output_path}");
            let result = pod2_onchain::trusted_setup(&input_path, &output_path);
            println!("trusted_setup result: {result}");
        } else {
            println!("trusted setup already exists at {output_path}, skipping");
        }

        Ok(())
    }

    // ignored by default since it takes long time to compute. To run it use:
    // cargo test --release -p common groth::tests::test_gen_trusted_setup -- --ignored
    #[ignore]
    #[test]
    fn test_gen_trusted_setup() -> Result<()> {
        gen_trusted_setup()?;

        Ok(())
    }

    // ignored by default since it takes long time to compute. To run it use:
    // cargo test --release -p common groth::tests::test_prove_method -- --ignored
    #[ignore]
    #[test]
    fn test_prove_method() -> Result<()> {
        // if trusted setup does not exist yet, generate it
        gen_trusted_setup()?;

        // obtain the pod to be proven
        let start = Instant::now();
        let pod = pod2_onchain::pod::sample_main_pod()?;
        println!(
            "[TIME] generate pod & compute pod proof took: {:?}",
            start.elapsed()
        );

        // initialize groth16 memory
        init()?;

        // compute its plonky2 & groth16 proof
        let (g16_proof, g16_pub_inp) = prove(pod.clone())?;
        pod2_onchain::groth16_verify(g16_proof.clone(), g16_pub_inp)?;

        // test the public_inputs parsing flow
        let (_, _, proof_with_pis) = pod2_onchain::prove_pod(pod)?;
        let pub_inp = proof_with_pis.public_inputs;

        // encode it as big-endian bytes compatible with Gnark
        let pub_inp_bytes = pod2_onchain::encode_public_inputs_gnark(pub_inp);
        // call groth16_verify again but now using the encoded pub_inp_bytes
        pod2_onchain::groth16_verify(g16_proof, pub_inp_bytes)?;

        Ok(())
    }
}
