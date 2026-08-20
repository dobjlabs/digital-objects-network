//! Chatty terminal output. The app is partly an explainer, so the
//! narration makes explicit who is saying what to whom and surfaces
//! the technical artifacts (hashes, statements, pods) as they cross.

use std::io::Write as _;

use pod2::middleware::{Hash, StrKey, Value, containers::Dictionary};

pub fn banner() {
    println!();
    println!("  +----------------------------------------------+");
    println!("  |              ⚠️ TRADE OFFER ⚠️               |");
    println!("  +----------------------------------------------+");
}

/// A narration line: who is speaking, over which channel.
pub fn say(who: &str, text: &str) {
    println!("  [{who}] {text}");
}

pub fn note(text: &str) {
    println!("      {text}");
}

pub fn short(hash: &Hash) -> String {
    let full = format!("{hash:#}");
    if full.len() > 14 {
        format!("{}..{}", &full[..8], &full[full.len() - 4..])
    } else {
        full
    }
}

/// Render an object's non-key fields compactly, key redacted.
pub fn describe_object(obj: &Dictionary) -> String {
    let mut parts = Vec::new();
    let mut entries: Vec<(String, Value)> = obj.iter().filter_map(|kv| kv.ok()).collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in entries {
        match name.as_str() {
            "key" => parts.push("key: (private)".to_string()),
            "type" | "stable_identifier" => {}
            _ => parts.push(format!("{name}: {value}")),
        }
    }
    format!("{{ {} }}", parts.join(", "))
}

pub fn object_line(label: &str, obj: &Dictionary) {
    println!(
        "      {label}: {}  {}",
        short(&obj.commitment()),
        describe_object(obj)
    );
}

/// Blocking yes/no prompt.
pub fn confirm(question: &str) -> bool {
    print!("  {question} [y/N] ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "YES")
}

pub fn field(label: &str, value: &str) {
    println!("      {label:<18} {value}");
}

pub fn heading(text: &str) {
    println!();
    println!("  == {text} ==");
}

pub fn key_moment(text: &str) {
    println!();
    println!("  *** {text} ***");
    println!();
}

pub fn redact_key(obj: &Dictionary) -> String {
    match obj.get(&StrKey::from("key")) {
        Ok(Some(_)) => "held".to_string(),
        _ => "missing".to_string(),
    }
}
