use craft_sdk::SpendableObject;

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
