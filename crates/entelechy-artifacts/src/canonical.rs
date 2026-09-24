//! Canonical serialization (PRD Q22, section 5.9).
//!
//! Artifact identity is a digest over a canonical byte encoding so that identity
//! "does not depend on map ordering, whitespace or platform-specific
//! formatting" (5.9). The 1.x contract is RFC 8785 JSON Canonicalization Scheme
//! (JCS): object members are sorted by the UTF-16 code units of their names and
//! the document is emitted with no insignificant whitespace.
//!
//! ## Known limitation (tracked to AC-2)
//! JCS also mandates ECMAScript `Number::toString` formatting for numbers. This
//! implementation relies on `serde_json`'s number formatting, which agrees with
//! JCS for integers and the values Entelechy artifacts currently use, but is not
//! yet a full `Number::toString`. Artifacts should therefore prefer integers and
//! strings until the golden JCS number fixtures (AC-2) are in place.

use serde_json::{Map, Value};

/// Serialize a JSON value to its canonical byte form (RFC 8785 subset).
pub fn to_canonical_bytes(value: &Value) -> Vec<u8> {
    let mut out = String::new();
    write_value(value, &mut out);
    out.into_bytes()
}

/// Serialize any `Serialize` type to canonical bytes via its JSON projection.
pub fn serialize_canonical<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let v = serde_json::to_value(value)?;
    Ok(to_canonical_bytes(&v))
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map, out),
    }
}

fn write_object(map: &Map<String, Value>, out: &mut String) {
    // RFC 8785: sort members by UTF-16 code units of the key.
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by(|a, b| utf16_cmp(a, b));
    out.push('{');
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(key, out);
        out.push(':');
        write_value(&map[*key], out);
    }
    out.push('}');
}

/// Compare two strings by their UTF-16 code units, per RFC 8785 §3.2.3.
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.encode_utf16();
    let mut ib = b.encode_utf16();
    loop {
        match (ia.next(), ib.next()) {
            (Some(x), Some(y)) => match x.cmp(&y) {
                std::cmp::Ordering::Equal => continue,
                non_eq => return non_eq,
            },
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

/// Emit a JSON string with the minimal escaping mandated by RFC 8785 §3.2.2.2.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{0009}' => out.push_str("\\t"),
            '\u{000A}' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\u{000D}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_key_order_is_normalized() {
        let a = json!({"b": 1, "a": 2});
        let b = json!({"a": 2, "b": 1});
        assert_eq!(to_canonical_bytes(&a), to_canonical_bytes(&b));
        assert_eq!(
            String::from_utf8(to_canonical_bytes(&a)).unwrap(),
            r#"{"a":2,"b":1}"#
        );
    }

    #[test]
    fn whitespace_is_insignificant() {
        let v: Value = serde_json::from_str("{\n  \"x\" : [1,   2]\n}").unwrap();
        assert_eq!(
            String::from_utf8(to_canonical_bytes(&v)).unwrap(),
            r#"{"x":[1,2]}"#
        );
    }

    #[test]
    fn strings_are_escaped() {
        let v = json!({"k": "line\nbreak\t\"q\""});
        assert_eq!(
            String::from_utf8(to_canonical_bytes(&v)).unwrap(),
            r#"{"k":"line\nbreak\t\"q\""}"#
        );
    }

    #[test]
    fn utf16_ordering() {
        // Nested keys must also be sorted.
        let v = json!({"z": {"y": 1, "x": 2}, "a": 0});
        assert_eq!(
            String::from_utf8(to_canonical_bytes(&v)).unwrap(),
            r#"{"a":0,"z":{"x":2,"y":1}}"#
        );
    }
}
