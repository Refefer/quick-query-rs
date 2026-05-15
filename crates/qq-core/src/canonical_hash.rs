//! Canonical hashing for `(tool_name, arguments)` pairs.
//!
//! Two layers consume this: the in-loop `RepetitionDetector` (which hard-blocks
//! identical calls after a threshold) and the `RepetitionWarningHook` (which
//! prepends an informational note after the first repeat). Both must agree on
//! what counts as "identical" — otherwise an arg-key-order difference could let
//! a loop slip past one layer but not the other. Keeping the function in a
//! single module enforces that.
//!
//! Canonicalisation: JSON object keys are sorted recursively before hashing,
//! so `{a:1, b:2}` and `{b:2, a:1}` hash to the same value.

use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Compute a deterministic hash of `(tool_name, arguments)` with sorted JSON keys.
pub(crate) fn canonical_hash(tool_name: &str, arguments: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);
    hash_value(arguments, &mut hasher);
    hasher.finish()
}

fn hash_value(value: &Value, hasher: &mut impl Hasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        Value::Number(n) => {
            2u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for v in arr {
                hash_value(v, hasher);
            }
        }
        Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                key.hash(hasher);
                hash_value(&map[key], hasher);
            }
        }
    }
}
