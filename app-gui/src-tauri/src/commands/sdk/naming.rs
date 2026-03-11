use craft_sdk::SpendableObject;
use pod2::middleware::{hash_values, Key, Value};

use super::synchronizer_client::encode_hash_hex;

pub(super) fn normalize_component_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn format_output_file_name(class_name: &str, object_id: &str) -> String {
    format!(
        "{}_{}.dobj",
        normalize_component_name(class_name),
        normalize_component_name(object_id)
    )
}

pub(super) fn object_id_from_spendable(spendable: &SpendableObject) -> String {
    format!("{:#}", spendable.obj.commitment())
}

pub(super) fn object_state_hash_from_spendable(spendable: &SpendableObject) -> String {
    format!("{:#}", spendable.obj.commitment())
}

pub(super) fn object_nullifier_from_spendable(
    spendable: &SpendableObject,
) -> Result<String, String> {
    let object_key = spendable
        .obj
        .get(&Key::from("key"))
        .cloned()
        .map_err(|err| {
            format!(
                "input object missing required key field for {}: {err}",
                object_id_from_spendable(spendable),
            )
        })?;
    let object_key_hash = hash_values(&[Value::from(spendable.obj.commitment()), object_key]);
    let object_nullifier = hash_values(&[
        Value::from(object_key_hash),
        Value::from("txlib-nullifier-v1"),
    ]);
    Ok(encode_hash_hex(&object_nullifier))
}

#[cfg(test)]
mod tests {
    use super::{format_output_file_name, normalize_component_name};

    #[test]
    fn normalize_component_name_replaces_non_alnum_and_lowercases() {
        assert_eq!(normalize_component_name("Stone Pick+1"), "stone_pick_1");
        assert_eq!(normalize_component_name("0xAbC-123"), "0xabc_123");
    }

    #[test]
    fn format_output_file_name_uses_class_and_object_id() {
        let file_name = format_output_file_name("StonePick", "0xAbC-123");
        assert_eq!(file_name, "stonepick_0xabc_123.dobj");
    }
}
