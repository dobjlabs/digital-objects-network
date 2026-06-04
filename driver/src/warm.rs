//! Startup warm-up of the proving circuits.
//!
//! Generating a proof lazily builds (or loads) several artifacts on the first
//! action. This module forces those builds/loads up front so `dobjd` does them
//! at boot instead.

use common::shrink::ShrunkMainPodSetup;
use lt_eq_u256_pod::STANDARD_LT_EQ_U256_VD_HASH;
use pod2::backends::plonky2::basetypes::DEFAULT_VD_SET;
use pod2::backends::plonky2::emptypod::EmptyPod;
use pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash;
use pod2::middleware::Params;
use vdfpod::STANDARD_VDF_VD_HASH;

/// Load (building on a cold cache) the proving artifacts the first action would
/// otherwise build lazily, so the first `execute` doesn't pay for them:
/// - the recursive MainPod circuit and its common hash,
/// - the VDF and lt_eq_u256 intro pod circuits (via their verifier-data hashes),
/// - the empty pod: both its circuit and the proved *instance* the prover
///   inserts as recursion padding.
///
/// Every step panics internally on a build/cache failure; the caller treats that
/// as fatal, since an artifact that can't build now would fail every action.
/// Idempotent: a warm disk cache makes each step a fast read.
pub fn warm_proving_circuits() {
    let start = std::time::Instant::now();
    let params = Params::default();
    log::info!("warming proving circuits at startup...");

    // Constructing ShrunkMainPodSetup reads pod2's rec-main-pod common+verifier
    // data, forcing that shared circuit (the one `Prover::prove` uses) to build.
    log::info!("warming recursive MainPod circuit (first cold build can take minutes)...");
    let _ = ShrunkMainPodSetup::new(&params);

    // The intro pods stamp this onto every pod they build (VdfPod::new /
    // LtEqU256Pod::new); a cheap hash of the rec-main-pod common circuit, but a
    // disk cache the first action would otherwise write.
    log::info!("warming rec MainPod common hash...");
    let _ = cache_get_rec_main_pod_common_hash(&params);

    // Empty pod: new_boxed proves+caches the instance the prover pads recursive
    // slots with (keyed by vd_set), and building it also builds the empty pod
    // circuit -- so this covers both empty pod caches the first proof would hit.
    log::info!("warming empty pod (circuit + instance)...");
    let _ = EmptyPod::new_boxed(DEFAULT_VD_SET.clone());

    // Forcing each intro pod's verifier-data hash drives its circuit data to
    // load/build, without constructing a pod (which would prove).
    log::info!("warming VDF intro pod circuit...");
    let _ = *STANDARD_VDF_VD_HASH;

    log::info!("warming lt_eq_u256 intro pod circuit...");
    let _ = *STANDARD_LT_EQ_U256_VD_HASH;

    log::info!("proving circuits ready (warmed in {:?})", start.elapsed());
}
