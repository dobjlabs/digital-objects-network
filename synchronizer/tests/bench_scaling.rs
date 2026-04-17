//! Scalability benchmarks for the synchronizer state machine.
//!
//! Measures the four workloads listed in `docs/synchronizer-scalability.md`:
//!   1. Proof verification (shrunk MainPod) per blob
//!   2. Merkle insert throughput at various tree sizes
//!   3. Full slot derivation (via `StateMachine::derive_slot_head`)
//!   4. Grounding witness generation latency (via `AppDb::prove_tx`)
//!
//! Run with:
//!   cargo test --release -p synchronizer --test bench_scaling \
//!     -- --ignored --nocapture --test-threads=1
//!
//! The first run will populate `~/.cache/pod2/` for the shrunk wrapper circuit
//! (a one-time cost, typically 1–3 minutes). Subsequent runs start quickly.

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use common::{
    payload::{Payload, PayloadProof},
    proof::MockBlobParser,
    shrink::{cache_get_shrunk_main_pod_circuit_data, shrink_compress_pod, ShrunkMainPodSetup},
};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{
        basetypes::DEFAULT_VD_SET,
        mainpod::{calculate_statements_hash, Prover},
    },
    frontend::{MainPodBuilder, Operation},
    middleware::{
        containers::Set, hash_values, Hash, Params, Statement, Value, EMPTY_HASH,
    },
};
use synchronizer::{
    app_db::AppDb,
    head::{CanonicalHead, CanonicalRoots},
    state_machine::StateMachine,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn unique_hash(seed: i64) -> Hash {
    hash_values(&[Value::from(seed)])
}

fn mock_txn_bytes(tx_final: Hash, nullifiers: &[Hash], state_root: Hash) -> Vec<u8> {
    let nullifiers_json = nullifiers
        .iter()
        .map(|h| format!("\"{:#}\"", h))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"tx_final":"{:#}","nullifiers":[{}],"state_root_hash":"{:#}"}}"#,
        tx_final, nullifiers_json, state_root
    )
    .into_bytes()
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn fmt_us(d: Duration) -> String {
    format!("{:.2} µs", d.as_secs_f64() * 1_000_000.0)
}

// ---------------------------------------------------------------------------
// Benchmark: proof verification
// ---------------------------------------------------------------------------

struct ProofVerificationFixture {
    payload_bytes: Vec<u8>,
    blob_size: usize,
    expected_statement: Statement,
    common_circuit_data: pod2::middleware::CommonCircuitData,
    verifier_circuit_data: pod2::middleware::VerifierCircuitData,
    vds_root: Hash,
}

/// Build a real shrunk Plonky2 MainPod proof once, then return the serialized
/// payload bytes plus everything needed to verify again on each call.
///
/// We build a proof over a trivial dummy predicate (same approach as
/// `common::payload::tests::test_payload_roundtrip`) because we care about
/// proof verification *cost*, which is dominated by Plonky2 FRI decompression
/// + verification of the shrunk wrapper circuit — that cost is independent of
/// which underlying MainPod custom predicate the proof was built against.
fn build_proof_fixture() -> Result<ProofVerificationFixture> {
    println!("  [proof] building circuit data + real shrunk proof (one-time cost)...");
    let t0 = Instant::now();

    let params = Params::default();
    let vd_set = &*DEFAULT_VD_SET;
    let vds_root = vd_set.root();

    // Trivial predicate — just needs to produce a valid Statement::Custom.
    let input = r#"
    Bench(tx_final, nullifiers, state_root) = AND(
        Equal(0, 0)
    )
    "#;
    let module = pod2::lang::load_module(input, "bench_txn", &params, &[]).unwrap();
    let pred = module.predicate_ref_by_name("Bench").unwrap();

    // Build the shrunk wrapper circuit (cached on disk after first use).
    let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build()?;

    // Build a MainPod that emits a single custom statement with 2 nullifiers.
    let tx_final = Value::from("bench_tx_final");
    let nullifiers = vec![unique_hash(9001), unique_hash(9002)];
    let nullifiers_set_value = Value::from(Set::new(
        nullifiers
            .iter()
            .map(|h| Value::from(*h))
            .collect::<HashSet<_>>(),
    ));
    let state_root_value = Value::from("bench_state_root");

    let mut builder = MainPodBuilder::new(&params, vd_set);
    let st0 = builder.priv_op(Operation::eq(0, 0))?;
    let _st_final = builder.op(
        true,
        vec![
            (0, tx_final.clone()),
            (1, nullifiers_set_value.clone()),
            (2, state_root_value.clone()),
        ],
        Operation::custom(pred.clone(), [st0]),
    )?;

    let prover = Prover {};
    let pod = builder.prove(&prover)?;
    pod.pod.verify()?;

    let shrunk_compressed = shrink_compress_pod(&shrunk_main_pod_build, pod)?;

    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(shrunk_compressed)),
        tx_final: Hash(tx_final.raw().0),
        state_root_hash: Hash(state_root_value.raw().0),
        nullifiers: nullifiers.clone(),
    };
    let payload_bytes = payload.to_bytes();
    let blob_size = payload_bytes.len();

    // Use the cached shrunk circuit data (same path `ProofParser::new` uses).
    let (common_circuit_data, verifier_circuit_data) =
        &*cache_get_shrunk_main_pod_circuit_data(&params);
    let common_circuit_data = (**common_circuit_data).clone();
    let verifier_circuit_data = (**verifier_circuit_data).clone();

    let expected_statement = Statement::Custom(
        pred,
        vec![
            Value::from(payload.tx_final),
            Value::from(Set::new(
                payload
                    .nullifiers
                    .iter()
                    .map(|h| Value::from(*h))
                    .collect::<HashSet<_>>(),
            )),
            Value::from(payload.state_root_hash),
        ],
    );

    println!(
        "  [proof] fixture ready in {} (blob size = {} bytes)",
        fmt_ms(t0.elapsed()),
        blob_size
    );
    Ok(ProofVerificationFixture {
        payload_bytes,
        blob_size,
        expected_statement,
        common_circuit_data,
        verifier_circuit_data,
        vds_root,
    })
}

/// Equivalent to `common::proof::ProofParser::parse_blob` but against a
/// caller-supplied expected statement instead of txlib's `TxFinalized`.
fn verify_fixture_once(fx: &ProofVerificationFixture) -> Result<()> {
    let payload = Payload::from_bytes(&fx.payload_bytes, &fx.common_circuit_data)?;
    let sts_hash = calculate_statements_hash(&[fx.expected_statement.clone().into()]);
    let public_inputs = [sts_hash.0, fx.vds_root.0].concat();
    let compressed_proof = match payload.proof {
        PayloadProof::Plonky2(proof) => proof,
        PayloadProof::Groth16(_) => unreachable!(),
    };
    let proof_with_pis = CompressedProofWithPublicInputs {
        proof: *compressed_proof,
        public_inputs,
    };
    let proof = proof_with_pis
        .decompress(
            &fx.verifier_circuit_data.verifier_only.circuit_digest,
            &fx.common_circuit_data,
        )
        .map_err(|e| anyhow::anyhow!("decompress: {e}"))?;
    fx.verifier_circuit_data
        .verify(proof)
        .map_err(|e| anyhow::anyhow!("verify: {e}"))?;
    Ok(())
}

fn bench_proof_verification(fx: &ProofVerificationFixture) -> Result<Duration> {
    // Warm up (first verify JIT-warms FRI tables, etc.).
    for _ in 0..2 {
        verify_fixture_once(fx)?;
    }
    const N: u32 = 20;
    let t0 = Instant::now();
    for _ in 0..N {
        verify_fixture_once(fx)?;
    }
    Ok(t0.elapsed() / N)
}

// ---------------------------------------------------------------------------
// Benchmark: Merkle insert throughput at a given tree size
// ---------------------------------------------------------------------------

/// Seeds a transactions Set to `target_size` entries, then measures the per-
/// insert time over `measure_n` additional insertions. Returns (mean per-insert).
fn bench_merkle_insert(target_size: usize, measure_n: usize) -> Result<Duration> {
    let dir = TempDir::new()?;
    let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
    let mut txs = app_db.open_transactions(EMPTY_HASH)?;

    println!("  [merkle @ {}] seeding...", target_size);
    let seed_start = Instant::now();
    let report_every = (target_size / 10).max(1);
    for i in 0..target_size {
        txs.insert(&Value::from(unique_hash(i as i64)))?;
        if (i + 1) % report_every == 0 {
            println!(
                "    seeded {}/{} ({})",
                i + 1,
                target_size,
                fmt_ms(seed_start.elapsed())
            );
        }
    }
    println!(
        "  [merkle @ {}] seeded {} entries in {}",
        target_size,
        target_size,
        fmt_ms(seed_start.elapsed())
    );

    let base = target_size as i64;
    let measure_start = Instant::now();
    for i in 0..measure_n {
        txs.insert(&Value::from(unique_hash(base + i as i64)))?;
    }
    let elapsed = measure_start.elapsed();
    Ok(elapsed / measure_n as u32)
}

// ---------------------------------------------------------------------------
// Benchmark: full slot derivation (merkle-only; proof verification excluded)
// ---------------------------------------------------------------------------

/// Measures `StateMachine::derive_slot_head` using `MockBlobParser` (which skips
/// real Plonky2 proof verification). The returned time is the Merkle-I/O
/// portion of per-slot cost; to estimate full per-slot time, add
/// `proof_verification_time × blobs_per_slot`.
///
/// Each iteration uses a fresh slot number and fresh blob contents to avoid
/// duplicate-rejection inside the state machine.
fn bench_slot_derivation(
    blobs_per_slot: usize,
    nullifiers_per_blob: usize,
    iters: u32,
) -> Result<Duration> {
    let dir = TempDir::new()?;
    let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
    let sm = StateMachine::new(app_db, Arc::new(MockBlobParser));

    // Seed an initial slot so there's a valid GSR to ground against.
    let head0 = sm.derive_slot_head(CanonicalHead::empty(), [], 0, 0, &[])?;
    let grounding_gsr = head0
        .metadata
        .current_gsr
        .expect("seed slot should publish a gsr");

    // Warm-up: one iteration so file-system / RocksDB paths are hot.
    let mut head = head0;
    let warmup_blobs = build_slot_blobs(0, blobs_per_slot, nullifiers_per_blob, grounding_gsr);
    head = sm.derive_slot_head(
        head,
        [(grounding_gsr, 0)],
        100,
        100,
        &warmup_blobs,
    )?;

    let mut total = Duration::ZERO;
    for iter in 0..iters {
        let blobs =
            build_slot_blobs(iter + 1, blobs_per_slot, nullifiers_per_blob, grounding_gsr);
        let t0 = Instant::now();
        head = sm.derive_slot_head(
            head,
            [(grounding_gsr, 0)],
            200 + iter,
            200 + iter,
            &blobs,
        )?;
        total += t0.elapsed();
    }
    Ok(total / iters)
}

fn build_slot_blobs(
    iter: u32,
    blobs_per_slot: usize,
    nullifiers_per_blob: usize,
    grounding_gsr: Hash,
) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::with_capacity(blobs_per_slot);
    let iter_base = (iter as i64) * 1_000_000;
    for b in 0..blobs_per_slot {
        let blob_base = iter_base + (b as i64) * 1_000;
        let tx_final = unique_hash(blob_base);
        let nullifiers: Vec<_> = (0..nullifiers_per_blob)
            .map(|n| unique_hash(blob_base + (n as i64) + 1))
            .collect();
        let bytes = mock_txn_bytes(tx_final, &nullifiers, grounding_gsr);
        out.push((b as u32, bytes));
    }
    out
}

// ---------------------------------------------------------------------------
// Benchmark: grounding witness latency (prove_tx at a given tree size)
// ---------------------------------------------------------------------------

fn bench_grounding_witness(tree_size: usize, measure_n: usize) -> Result<Duration> {
    let dir = TempDir::new()?;
    let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
    let mut txs = app_db.open_transactions(EMPTY_HASH)?;

    println!("  [grounding @ {}] seeding...", tree_size);
    let seed_start = Instant::now();
    let sample_stride = (tree_size / measure_n).max(1);
    let mut sampled_hashes = Vec::with_capacity(measure_n);
    for i in 0..tree_size {
        let h = unique_hash(i as i64);
        txs.insert(&Value::from(h))?;
        if sampled_hashes.len() < measure_n && i % sample_stride == 0 {
            sampled_hashes.push(h);
        }
    }
    println!(
        "  [grounding @ {}] seeded in {}",
        tree_size,
        fmt_ms(seed_start.elapsed())
    );

    let roots = CanonicalRoots {
        transactions: txs.commitment(),
        ..CanonicalRoots::empty()
    };

    // Warm-up — first call pulls pages into the OS cache.
    for h in sampled_hashes.iter().take(3) {
        let _ = app_db.prove_tx(&roots, *h)?;
    }

    let t0 = Instant::now();
    for h in &sampled_hashes {
        let _ = app_db.prove_tx(&roots, *h)?;
    }
    let elapsed = t0.elapsed();
    Ok(elapsed / sampled_hashes.len() as u32)
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn bench_scaling_all() -> Result<()> {
    println!("\n=== synchronizer scalability benchmark ===\n");

    // 1. Proof verification
    println!("[1/6] Proof verification per blob");
    let fx = build_proof_fixture()?;
    let proof_time = bench_proof_verification(&fx)?;
    println!("  => {} / blob (blob size {} bytes)\n", fmt_ms(proof_time), fx.blob_size);

    // 2. Merkle insert at 10K
    println!("[2/6] Merkle insert per entry at tree size 10K");
    let insert_10k = bench_merkle_insert(10_000, 200)?;
    println!("  => {} / insert\n", fmt_us(insert_10k));

    // 3. Merkle insert at 1M
    println!("[3/6] Merkle insert per entry at tree size 1M");
    let insert_1m = bench_merkle_insert(1_000_000, 200)?;
    println!("  => {} / insert\n", fmt_us(insert_1m));

    // 4. Full slot derivation: 1 blob, 2 nullifiers
    println!("[4/6] Slot derivation: 1 blob, 2 nullifiers (merkle-only)");
    let slot_1_2_merkle = bench_slot_derivation(1, 2, 5)?;
    let slot_1_2_full = slot_1_2_merkle + proof_time;
    println!(
        "  => merkle-only {}; full (+1×proof verify) {}\n",
        fmt_ms(slot_1_2_merkle),
        fmt_ms(slot_1_2_full)
    );

    // 5. Full slot derivation: 6 blobs, 12 nullifiers (2 per blob)
    println!("[5/6] Slot derivation: 6 blobs × 2 nullifiers = 12 nullifiers (merkle-only)");
    let slot_6_12_merkle = bench_slot_derivation(6, 2, 5)?;
    let slot_6_12_full = slot_6_12_merkle + proof_time * 6;
    println!(
        "  => merkle-only {}; full (+6×proof verify) {}\n",
        fmt_ms(slot_6_12_merkle),
        fmt_ms(slot_6_12_full)
    );

    // 6. Grounding witness at 100K
    println!("[6/6] Grounding witness latency (1 source TX, 100K tree)");
    let grounding = bench_grounding_witness(100_000, 50)?;
    println!("  => {} / prove_tx call\n", fmt_ms(grounding));

    // Summary
    println!("\n=== Benchmark summary (paste into docs/synchronizer-scalability.md) ===\n");
    println!("- Proof verification time per blob: **{}**", fmt_ms(proof_time));
    println!(
        "- Merkle insert time per entry at 10K total: **{}**",
        fmt_us(insert_10k)
    );
    println!(
        "- Merkle insert time per entry at 1M total: **{}**",
        fmt_us(insert_1m)
    );
    println!(
        "- Full slot derivation (1 blob, 2 nullifiers): **{}** (merkle {} + 1× proof verify {})",
        fmt_ms(slot_1_2_full),
        fmt_ms(slot_1_2_merkle),
        fmt_ms(proof_time)
    );
    println!(
        "- Full slot derivation (6 blobs, 12 nullifiers): **{}** (merkle {} + 6× proof verify {} each)",
        fmt_ms(slot_6_12_full),
        fmt_ms(slot_6_12_merkle),
        fmt_ms(proof_time)
    );
    println!(
        "- Grounding witness latency (1 source TX, 100K tree): **{}**",
        fmt_ms(grounding)
    );

    Ok(())
}
