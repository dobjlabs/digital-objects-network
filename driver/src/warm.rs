//! Startup warm-up of the proving circuits.
//!
//! Building proofs lazily builds several circuits on the first action. This
//! module forces those builds up front so `dobjd` can do them at boot instead.

use anyhow::{Context, Result};
use common::shrink::ShrunkMainPodSetup;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::backends::plonky2::basetypes::DEFAULT_VD_SET;
use pod2::backends::plonky2::emptypod::EmptyPod;
use pod2::middleware::{Params, RawValue};
use vdfpod::VdfPod;

/// Build every proving circuit the first action would otherwise build lazily,
/// so the first `execute` is fast. On a cold start the first proof builds:
/// - the recursive MainPod circuit (pod2's disk cache, the dominant artifact),
/// - pod2's empty pod (the proved instance the prover inserts as recursion
///   padding, plus its circuit),
/// - the VDF intro pod circuit, including its in-memory `VDF_RECURSIVE_CIRCUIT`
///   -- only generating a proof forces that one to build, so we prove a minimal
///   input (the VDF requires at least 2 iterations), and
/// - the lt_eq_u256 intro pod circuit.
///
/// A failure propagates rather than being swallowed: a circuit that cannot build
/// at startup would fail every action too, so the caller should refuse to start
/// rather than serve a daemon that cannot prove. (`ShrunkMainPodSetup::new` and
/// `EmptyPod::new_boxed` return their value directly and panic internally on a
/// build/cache failure; that panic is as fatal as the `?`s below.) Idempotent:
/// already-built circuits load from the disk cache -- though the VDF recursive
/// circuit is in-memory, so it is rebuilt once per process regardless.
pub fn warm_proving_circuits() -> Result<()> {
    let start = std::time::Instant::now();
    let params = Params::default();
    let vd_set = DEFAULT_VD_SET.clone();
    log::info!("warming proving circuits at startup...");

    // Recursive MainPod circuit. Constructing ShrunkMainPodSetup reads pod2's
    // rec-main-pod common+verifier data, which forces that shared circuit data
    // (the one `Prover::prove` uses) to be built and cached.
    log::info!("warming recursive MainPod circuit (first cold build can take minutes)...");
    let _ = ShrunkMainPodSetup::new(&params);

    // Empty pod: the prover pads recursive slots with a proved empty pod keyed
    // by vd_set. `new_boxed` caches that instance ("empty_pod") and, building
    // it, the empty pod circuit ("standard_empty_pod_circuit_data").
    log::info!("warming empty pod...");
    let _ = EmptyPod::new_boxed(vd_set.clone());

    log::info!("warming VDF intro pod circuit...");
    VdfPod::new_boxed(&params, vd_set.clone(), 2, RawValue::from(0_i64))
        .context("warming VDF intro pod circuit")?;

    // lt_eq_u256 requires lhs <= rhs; equal inputs satisfy the precheck.
    log::info!("warming lt_eq_u256 intro pod circuit...");
    LtEqU256Pod::new_boxed(
        &params,
        vd_set,
        RawValue::from(0_i64),
        RawValue::from(0_i64),
    )
    .context("warming lt_eq_u256 intro pod circuit")?;

    log::info!("proving circuits ready (warmed in {:?})", start.elapsed());
    Ok(())
}
