use crate::types::{CreateDobjInput, CreateDobjResult};
use std::hint::black_box;
use std::time::{Duration, Instant};

fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

fn burn_cpu_for(duration: Duration) {
    let start = Instant::now();
    let mut acc: u64 = 0x9e37_79b9_7f4a_7c15;

    // Intentional tight loop to generate measurable app CPU load for mock runs.
    while start.elapsed() < duration {
        for i in 0..250_000_u64 {
            acc = acc.rotate_left(13) ^ (i.wrapping_mul(0xbf58_476d_1ce4_e5b9));
            acc = acc.wrapping_mul(0x94d0_49bb_1331_11eb);
        }
    }

    let _ = black_box(acc);
}

#[tauri::command]
pub async fn create_dobj(input: CreateDobjInput) -> Result<CreateDobjResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        burn_cpu_for(Duration::from_secs(10));

        let seed = format!("{}:{}", input.dobj_id, input.input_files.join(","));
        let old_root = fake_hash(&format!("{seed}:old"));
        let new_root = fake_hash(&format!("{seed}:new"));

        CreateDobjResult {
            ok: true,
            old_root,
            new_root,
            output_file: format!("{}.dobj", input.dobj_id),
        }
    })
    .await
    .map_err(|err| format!("failed to join create_dobj worker: {err}"))
}
