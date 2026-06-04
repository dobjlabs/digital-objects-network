//! Startup warm-up of the proving circuits.
//!
//! Building proofs lazily builds several circuits on the first action. This
//! module forces those builds up front so `dobjd` can do them at boot instead.

use common::shrink::ShrunkMainPodSetup;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::backends::plonky2::basetypes::DEFAULT_VD_SET;
use pod2::backends::plonky2::emptypod::cache_get_standard_empty_pod_circuit_data;
use pod2::middleware::{Params, RawValue};
use vdfpod::VdfPod;

/// Build every proving circuit the first action would otherwise build lazily,
/// so the first `execute` is fast. On a cold start the first proof builds:
/// - the recursive MainPod circuit (pod2's disk cache, the dominant artifact),
/// - pod2's standard empty pod circuit (recursion padding the prover inserts),
/// - the VDF intro pod circuit, including its in-memory `VDF_RECURSIVE_CIRCUIT`
///   -- only generating a proof forces that one to build, so we prove a minimal
///   input (the VDF requires at least 2 iterations), and
/// - the lt_eq_u256 intro pod circuit.
///
/// Each step is best-effort: a failure is logged and warming continues, so a
/// warm-up problem never blocks startup -- the first action just builds
/// whatever is missing, as before. Idempotent: already-built circuits load from
/// the disk cache (the VDF recursive circuit is in-memory, so it is rebuilt once
/// per process regardless).
pub fn warm_proving_circuits() {
    let start = std::time::Instant::now();
    let params = Params::default();
    let vd_set = DEFAULT_VD_SET.clone();
    log::info!("warming proving circuits at startup...");

    // Recursive MainPod circuit. Constructing ShrunkMainPodSetup reads pod2's
    // rec-main-pod common+verifier data, which forces that shared circuit data
    // (the one `Prover::prove` uses) to be built and cached; the value is
    // dropped -- the populated cache is the point.
    log::info!("warming recursive MainPod circuit (first cold build can take minutes)...");
    let _ = ShrunkMainPodSetup::new(&params);

    // Empty pod: the prover pads recursive slots with it, so proving builds it
    // on a cold cache even though no action references it directly.
    log::info!("warming empty pod circuit...");
    let _ = cache_get_standard_empty_pod_circuit_data();

    log::info!("warming VDF intro pod circuit...");
    if let Err(err) = VdfPod::new_boxed(&params, vd_set.clone(), 2, RawValue::from(0_i64)) {
        log::warn!("VDF circuit warm-up failed (first action will build it): {err:#}");
    }

    // lt_eq_u256 requires lhs <= rhs; equal inputs satisfy the precheck.
    log::info!("warming lt_eq_u256 intro pod circuit...");
    if let Err(err) = LtEqU256Pod::new_boxed(
        &params,
        vd_set,
        RawValue::from(0_i64),
        RawValue::from(0_i64),
    ) {
        log::warn!("lt_eq_u256 circuit warm-up failed (first action will build it): {err:#}");
    }

    log::info!("proving circuits ready (warmed in {:?})", start.elapsed());
}
