//! Applies Chord delta op batches to a JSON value: a port of `apply` in
//! `@earendil-works/chord/delta` (the backend's `conversation.view` ops).
//!
//! Ops are decoded tuples (paths inline):
//!
//! | Tuple | Meaning |
//! | --- | --- |
//! | `["r", value]` | Replace the complete value. |
//! | `["s", path, value]` | Set an object property or array element. |
//! | `["d", path]` | Delete an object property or remove an array element. |
//! | `["a", path, text]` | Append to a string. |
//! | `["t", path, count]` | Remove `count` UTF-16 code units from a string's front. |
//! | `["p", path, index, remove, items]` | Splice an array. |
//! | `["m", path, permutation]` | Reorder an array: `new[i] = old[permutation[i]]`. |
//!
//! Like Chord, ops apply in order and an error leaves the value partially
//! updated: discard it and recover from a fresh full value.

use serde_json::Value;

/// Path segments that reach the JS prototype chain; Chord rejects them.
const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// Why a batch couldn't be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeltaError {
    /// The op has the wrong verb, arity, or payload shape (Chord's
    /// `TypeError`).
    #[error("invalid op: {0}")]
    InvalidOp(&'static str),
    /// A path segment is reserved, not a key or index, or an index is past the
    /// end of its array (Chord's `UnsafePathError`).
    #[error("unsafe path segment: {0}")]
    UnsafePath(Value),
    /// The path doesn't resolve to a value of the op's kind (Chord's
    /// `PathError`).
    #[error("unresolvable path: {0}")]
    Path(Value),
}

/// A validated path segment.
#[derive(Debug, Clone, Copy)]
enum Seg<'a> {
    Key(&'a str),
    Index(u64),
}

impl Seg<'_> {
    fn to_value(self) -> Value {
        match self {
            Self::Key(key) => Value::from(key),
            Self::Index(index) => Value::from(index),
        }
    }
}

/// Applies `ops` to `value` in order.
///
/// # Errors
///
/// Returns a [`DeltaError`] for a malformed op or a path that doesn't resolve;
/// `value` is then partially updated and should be discarded.
pub fn apply(value: &mut Value, ops: &[Value]) -> Result<(), DeltaError> {
    ops.iter().try_for_each(|op| apply_op(value, op))
}

fn apply_op(root: &mut Value, op: &Value) -> Result<(), DeltaError> {
    let Some(tuple) = op.as_array().filter(|tuple| !tuple.is_empty()) else {
        return Err(DeltaError::InvalidOp("op is not a tuple"));
    };
    match (tuple[0].as_str(), tuple.len()) {
        (Some("r"), 2) => {
            tuple[1].clone_into(root);
            Ok(())
        }
        (Some("r"), _) => Err(DeltaError::InvalidOp("r arity")),
        (Some("s"), 3) => {
            let path = path_arg(&tuple[1], true)?;
            let (parent, key) = parent_slot(root, &tuple[1], &path)?;
            write(parent, key, tuple[2].clone());
            Ok(())
        }
        (Some("s"), _) => Err(DeltaError::InvalidOp("s arity")),
        (Some("d"), 2) => {
            let path = path_arg(&tuple[1], true)?;
            let (parent, key) = parent_slot(root, &tuple[1], &path)?;
            match (parent, key) {
                (Value::Array(items), Slot::Index(index)) => {
                    if index >= items.len() {
                        return Err(DeltaError::Path(tuple[1].clone()));
                    }
                    items.remove(index);
                }
                (Value::Object(map), Slot::Key(key)) => _ = map.remove(&key),
                _ => unreachable!("parent_slot pairs arrays with indices and objects with keys"),
            }
            Ok(())
        }
        (Some("d"), _) => Err(DeltaError::InvalidOp("d arity")),
        (Some("a"), 3) => {
            let Some(text) = tuple[2].as_str() else {
                return Err(DeltaError::InvalidOp("a shape"));
            };
            let path = path_arg(&tuple[1], true)?;
            let (parent, key) = parent_slot(root, &tuple[1], &path)?;
            let Some(Value::String(current)) = read(parent, &key) else {
                return Err(DeltaError::Path(tuple[1].clone()));
            };
            current.push_str(text);
            Ok(())
        }
        (Some("a"), _) => Err(DeltaError::InvalidOp("a shape")),
        (Some("t"), 3) => {
            let Some(count) = non_negative_integer(&tuple[2]) else {
                return Err(DeltaError::InvalidOp("t shape"));
            };
            let path = path_arg(&tuple[1], true)?;
            let (parent, key) = parent_slot(root, &tuple[1], &path)?;
            let Some(Value::String(current)) = read(parent, &key) else {
                return Err(DeltaError::Path(tuple[1].clone()));
            };
            trim_utf16_front(current, count);
            Ok(())
        }
        (Some("t"), _) => Err(DeltaError::InvalidOp("t shape")),
        (Some("p"), 5) => splice(root, tuple),
        (Some("p"), _) => Err(DeltaError::InvalidOp("p arity")),
        (Some("m"), 3) => reorder(root, tuple),
        (Some("m"), _) => Err(DeltaError::InvalidOp("m arity")),
        _ => Err(DeltaError::InvalidOp("unknown op verb")),
    }
}

/// `["p", path, index, remove, items]`.
fn splice(root: &mut Value, tuple: &[Value]) -> Result<(), DeltaError> {
    let path = path_arg(&tuple[1], false)?;
    let Some(index) = non_negative_integer(&tuple[2]) else {
        return Err(DeltaError::InvalidOp("p index"));
    };
    let Some(remove) = non_negative_integer(&tuple[3]) else {
        return Err(DeltaError::InvalidOp("p remove"));
    };
    let Some(items) = tuple[4].as_array() else {
        return Err(DeltaError::InvalidOp("p items"));
    };
    let Value::Array(target) = resolve(root, &tuple[1], &path)? else {
        return Err(DeltaError::Path(tuple[1].clone()));
    };
    // `Array.prototype.splice` clamps both the start and the count.
    let start = index.min(target.len());
    let end = start.saturating_add(remove).min(target.len());
    target.splice(start..end, items.iter().cloned());
    Ok(())
}

/// `["m", path, permutation]`.
fn reorder(root: &mut Value, tuple: &[Value]) -> Result<(), DeltaError> {
    let path = path_arg(&tuple[1], false)?;
    let permutation = permutation(&tuple[2])?;
    let Value::Array(target) = resolve(root, &tuple[1], &path)? else {
        return Err(DeltaError::Path(tuple[1].clone()));
    };
    if target.len() != permutation.len() {
        return Err(DeltaError::Path(tuple[1].clone()));
    }
    let mut previous: Vec<Option<Value>> = std::mem::take(target).into_iter().map(Some).collect();
    *target = permutation
        .iter()
        .map(|&from| {
            previous[from]
                .take()
                .expect("a bijection takes each element once")
        })
        .collect();
    Ok(())
}

/// Validates a path argument (Chord's `assertPathArg`).
fn path_arg(path: &Value, non_empty: bool) -> Result<Vec<Seg<'_>>, DeltaError> {
    let Some(segments) = path.as_array() else {
        return Err(DeltaError::InvalidOp("path is not an array"));
    };
    if non_empty && segments.is_empty() {
        return Err(DeltaError::InvalidOp("path is empty"));
    }
    segments
        .iter()
        .map(|segment| match segment {
            Value::String(key) if RESERVED_SEGMENTS.contains(&key.as_str()) => {
                Err(DeltaError::UnsafePath(segment.clone()))
            }
            Value::String(key) => Ok(Seg::Key(key)),
            _ => non_negative_integer_u64(segment)
                .map(Seg::Index)
                .ok_or_else(|| DeltaError::UnsafePath(segment.clone())),
        })
        .collect()
}

/// Validates an `m` permutation: a bijection on `0..len` (Chord's
/// `assertPermutation`).
fn permutation(value: &Value) -> Result<Vec<usize>, DeltaError> {
    let Some(entries) = value.as_array() else {
        return Err(DeltaError::InvalidOp("m permutation is not an array"));
    };
    let mut seen = vec![false; entries.len()];
    entries
        .iter()
        .map(|entry| {
            let index = non_negative_integer(entry)
                .filter(|&index| index < entries.len() && !seen[index])
                .ok_or(DeltaError::InvalidOp("m permutation is not a bijection"))?;
            seen[index] = true;
            Ok(index)
        })
        .collect()
}

/// 2^64: the first integral `f64` past `u64`.
const PAST_U64: f64 = 18_446_744_073_709_551_616.0;

/// A JSON number that JS's `Number.isInteger` accepts and that is non-negative.
fn non_negative_integer_u64(value: &Value) -> Option<u64> {
    let Value::Number(number) = value else {
        return None;
    };
    number.as_u64().or_else(|| {
        let float = number.as_f64()?;
        // `Number.isInteger(1.0)` holds; integers past `u64` can't index
        // anything real.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked integral, non-negative, and in range"
        )]
        (float.fract() == 0.0 && (0.0..PAST_U64).contains(&float)).then_some(float as u64)
    })
}

/// [`non_negative_integer_u64`] as a `usize`, saturating: counts past memory
/// clamp like JS's.
fn non_negative_integer(value: &Value) -> Option<usize> {
    non_negative_integer_u64(value).map(|n| usize::try_from(n).unwrap_or(usize::MAX))
}

/// The key an `s`/`d`/`a`/`t` op writes under its parent.
enum Slot {
    Key(String),
    Index(usize),
}

/// Resolves the parent of `path`'s last segment and checks the segment fits it.
fn parent_slot<'v>(
    root: &'v mut Value,
    raw: &Value,
    path: &[Seg<'_>],
) -> Result<(&'v mut Value, Slot), DeltaError> {
    let (last, init) = path.split_last().expect("validated non-empty");
    let parent = resolve(root, raw, init)?;
    let slot = match (&*parent, *last) {
        (Value::Array(_), Seg::Key(_)) => return Err(DeltaError::UnsafePath(last.to_value())),
        (Value::Array(items), Seg::Index(index)) => {
            // An index may address an element or append exactly one past the
            // end.
            match usize::try_from(index) {
                Ok(index) if index <= items.len() => Slot::Index(index),
                _ => return Err(DeltaError::UnsafePath(last.to_value())),
            }
        }
        (_, Seg::Key(key)) => Slot::Key(key.to_owned()),
        // JS property keys are strings: `obj[3]` is `obj["3"]`.
        (_, Seg::Index(index)) => Slot::Key(index.to_string()),
    };
    Ok((parent, slot))
}

/// Walks `path` through own properties to an object or array (Chord's
/// `resolve`).
fn resolve<'v>(
    root: &'v mut Value,
    raw: &Value,
    path: &[Seg<'_>],
) -> Result<&'v mut Value, DeltaError> {
    let unresolvable = || DeltaError::Path(raw.clone());
    let mut node = root;
    for &segment in path {
        node = match (node, segment) {
            (Value::Array(_), Seg::Key(_)) => {
                return Err(DeltaError::UnsafePath(segment.to_value()));
            }
            (Value::Array(items), Seg::Index(index)) => usize::try_from(index)
                .ok()
                .and_then(|index| items.get_mut(index))
                .ok_or_else(unresolvable)?,
            (Value::Object(map), Seg::Key(key)) => map.get_mut(key).ok_or_else(unresolvable)?,
            (Value::Object(map), Seg::Index(index)) => {
                map.get_mut(&index.to_string()).ok_or_else(unresolvable)?
            }
            _ => return Err(unresolvable()),
        };
    }
    if node.is_object() || node.is_array() {
        Ok(node)
    } else {
        Err(unresolvable())
    }
}

fn read<'v>(parent: &'v mut Value, slot: &Slot) -> Option<&'v mut Value> {
    match (parent, slot) {
        (Value::Array(items), Slot::Index(index)) => items.get_mut(*index),
        (Value::Object(map), Slot::Key(key)) => map.get_mut(key),
        _ => None,
    }
}

fn write(parent: &mut Value, slot: Slot, value: Value) {
    match (parent, slot) {
        (Value::Array(items), Slot::Index(index)) => {
            if index == items.len() {
                items.push(value);
            } else {
                items[index] = value;
            }
        }
        (Value::Object(map), Slot::Key(key)) => _ = map.insert(key, value),
        _ => unreachable!("parent_slot pairs arrays with indices and objects with keys"),
    }
}

/// Drops `count` UTF-16 code units from the front, like JS's
/// `text.slice(count)`. A cut through a surrogate pair leaves a lone surrogate
/// in JS; Rust strings can't hold one, so it becomes U+FFFD.
fn trim_utf16_front(text: &mut String, count: usize) {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units >= count {
            text.drain(..offset);
            return;
        }
        units += ch.len_utf16();
        if units > count {
            // `count` falls inside this char's surrogate pair: its low
            // surrogate survives.
            let rest = offset + ch.len_utf8();
            text.replace_range(..rest, "\u{FFFD}");
            return;
        }
    }
    text.clear();
}
