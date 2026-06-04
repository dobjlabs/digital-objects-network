//! Startup warm-up of the proving circuits.
//!
//! Generating a proof lazily builds (or loads) several circuits on the first
//! action. This module forces those circuit builds/loads up front so `dobjd`
//! does them at boot instead -- touching circuit data only, never proving.

use common::shrink::ShrunkMainPodSetup;
use lt_eq_u256_pod::STANDARD_LT_EQ_U256_VD_HASH;
use pod2::backends::plonky2::emptypod::cache_get_standard_empty_pod_circuit_data;
use pod2::middleware::Params;
use vdfpod::STANDARD_VDF_VD_HASH;

/// Load (building on a cold cache) the proving circuits the first action would
/// otherwise build lazily, so the first `execute` doesn't pay for them. This
/// only touches circuit data -- it does not generate proofs:
/// - the recursive MainPod circuit (pod2's disk cache, the dominant artifact),
/// - pod2's empty pod circuit,
/// - the VDF intro pod circuit (forcing `STANDARD_VDF_VD_HASH` loads it), and
/// - the lt_eq_u256 intro pod circuit (via `STANDARD_LT_EQ_U256_VD_HASH`).
///
/// Every step panics internally on a build/cache failure; the caller treats that
/// panic as fatal, since a circuit that can't build now would fail every action.
/// Idempotent: a warm disk cache makes each step a fast read.
pub fn warm_proving_circuits() {
    let start = std::time::Instant::now();
    let params = Params::default();
    log::info!("warming proving circuits at startup...");

    // Constructing ShrunkMainPodSetup reads pod2's rec-main-pod common+verifier
    // data, forcing that shared circuit (the one `Prover::prove` uses) to build.
    log::info!("warming recursive MainPod circuit (first cold build can take minutes)...");
    let _ = ShrunkMainPodSetup::new(&params);

    log::info!("warming empty pod circuit...");
    let _ = cache_get_standard_empty_pod_circuit_data();

    // Forcing each intro pod's verifier-data hash drives its circuit data to
    // load/build, without constructing a pod (which would prove).
    log::info!("warming VDF intro pod circuit...");
    let _ = *STANDARD_VDF_VD_HASH;

    log::info!("warming lt_eq_u256 intro pod circuit...");
    let _ = *STANDARD_LT_EQ_U256_VD_HASH;

    log::info!("proving circuits ready (warmed in {:?})", start.elapsed());
}
