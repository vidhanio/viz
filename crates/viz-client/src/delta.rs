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

use std::mem;

use serde_json::Value;

/// Path segments that reach the JS prototype chain; Chord rejects them.
const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// 2^64: the first integral `f64` past `u64`.
const PAST_U64: f64 = 18_446_744_073_709_551_616.0;

/// Why a batch couldn't be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
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

/// Applies `ops` to `value` in order.
///
/// # Errors
///
/// Returns an [`ApplyError`] for a malformed op or a path that doesn't
/// resolve; `value` is then partially updated and should be discarded.
pub fn apply(value: &mut Value, ops: &[Value]) -> Result<(), ApplyError> {
    for op in ops {
        Op::decode(op)?.apply(value)?;
    }
    Ok(())
}

/// A validated op, borrowing its payload from the batch.
enum Op<'o> {
    /// `r`.
    Replace(&'o Value),
    /// `s`.
    Set {
        path: SlotPath<'o>,
        value: &'o Value,
    },
    /// `d`.
    Delete(SlotPath<'o>),
    /// `a`.
    Append { path: SlotPath<'o>, text: &'o str },
    /// `t`.
    TrimFront { path: SlotPath<'o>, count: usize },
    /// `p`.
    Splice {
        path: Path<'o>,
        index: usize,
        remove: usize,
        items: &'o [Value],
    },
    /// `m`: a bijection on the array's indices.
    Reorder {
        path: Path<'o>,
        permutation: Vec<usize>,
    },
}

impl<'o> Op<'o> {
    /// Validates an op tuple's verb, arity, path and payload, in Chord's order.
    fn decode(op: &'o Value) -> Result<Self, ApplyError> {
        let Some((verb, args)) = op.as_array().and_then(|tuple| tuple.split_first()) else {
            return Err(ApplyError::InvalidOp("op is not a tuple"));
        };
        let invalid = |reason| Err(ApplyError::InvalidOp(reason));
        match (verb.as_str(), args) {
            (Some("r"), [value]) => Ok(Self::Replace(value)),
            (Some("r"), _) => invalid("r arity"),
            (Some("s"), [path, value]) => Ok(Self::Set {
                path: SlotPath::parse(path)?,
                value,
            }),
            (Some("s"), _) => invalid("s arity"),
            (Some("d"), [path]) => Ok(Self::Delete(SlotPath::parse(path)?)),
            (Some("d"), _) => invalid("d arity"),
            (Some("a"), [path, text]) => {
                let Some(text) = text.as_str() else {
                    return invalid("a shape");
                };
                Ok(Self::Append {
                    path: SlotPath::parse(path)?,
                    text,
                })
            }
            (Some("a"), _) => invalid("a shape"),
            (Some("t"), [path, count]) => {
                let Some(count) = non_negative_integer(count) else {
                    return invalid("t shape");
                };
                Ok(Self::TrimFront {
                    path: SlotPath::parse(path)?,
                    count,
                })
            }
            (Some("t"), _) => invalid("t shape"),
            (Some("p"), [path, index, remove, items]) => {
                let path = Path::parse(path)?;
                let Some(index) = non_negative_integer(index) else {
                    return invalid("p index");
                };
                let Some(remove) = non_negative_integer(remove) else {
                    return invalid("p remove");
                };
                let Some(items) = items.as_array() else {
                    return invalid("p items");
                };
                Ok(Self::Splice {
                    path,
                    index,
                    remove,
                    items,
                })
            }
            (Some("p"), _) => invalid("p arity"),
            (Some("m"), [path, permutation]) => Ok(Self::Reorder {
                path: Path::parse(path)?,
                permutation: decode_permutation(permutation)?,
            }),
            (Some("m"), _) => invalid("m arity"),
            _ => invalid("unknown op verb"),
        }
    }

    fn apply(self, root: &mut Value) -> Result<(), ApplyError> {
        match self {
            Self::Replace(value) => value.clone_into(root),
            Self::Set { path, value } => {
                let (parent, slot) = path.resolve(root)?;
                slot.write(parent, value.clone());
            }
            Self::Delete(path) => match path.resolve(root)? {
                (Value::Array(items), Slot::Index(index)) => {
                    if index >= items.len() {
                        return Err(path.unresolvable());
                    }
                    items.remove(index);
                }
                (Value::Object(map), Slot::Key(key)) => _ = map.remove(&key),
                _ => unreachable!("SlotPath::resolve pairs arrays with indices, objects with keys"),
            },
            Self::Append { path, text } => {
                let (parent, slot) = path.resolve(root)?;
                let Some(Value::String(current)) = slot.lookup(parent) else {
                    return Err(path.unresolvable());
                };
                current.push_str(text);
            }
            Self::TrimFront { path, count } => {
                let (parent, slot) = path.resolve(root)?;
                let Some(Value::String(current)) = slot.lookup(parent) else {
                    return Err(path.unresolvable());
                };
                trim_utf16_front(current, count);
            }
            Self::Splice {
                path,
                index,
                remove,
                items,
            } => {
                let Value::Array(target) = path.resolve(root)? else {
                    return Err(path.unresolvable());
                };
                // `Array.prototype.splice` clamps both the start and the count.
                let start = index.min(target.len());
                let end = start.saturating_add(remove).min(target.len());
                target.splice(start..end, items.iter().cloned());
            }
            Self::Reorder { path, permutation } => {
                let Value::Array(target) = path.resolve(root)? else {
                    return Err(path.unresolvable());
                };
                if target.len() != permutation.len() {
                    return Err(path.unresolvable());
                }
                let mut previous: Vec<Option<Value>> =
                    mem::take(target).into_iter().map(Some).collect();
                *target = permutation
                    .iter()
                    .map(|&from| {
                        previous[from]
                            .take()
                            .expect("a bijection takes each element once")
                    })
                    .collect();
            }
        }
        Ok(())
    }
}

/// A validated path segment.
#[derive(Debug, Clone, Copy)]
enum Segment<'o> {
    Key(&'o str),
    Index(u64),
}

impl Segment<'_> {
    fn to_value(self) -> Value {
        match self {
            Self::Key(key) => Value::from(key),
            Self::Index(index) => Value::from(index),
        }
    }
}

/// A validated path to an object or array, possibly the root (`p`, `m`).
struct Path<'o> {
    /// The path as sent, for errors.
    raw: &'o Value,
    segments: Vec<Segment<'o>>,
}

impl<'o> Path<'o> {
    /// Validates a path argument (Chord's `assertPathArg`).
    fn parse(raw: &'o Value) -> Result<Self, ApplyError> {
        let Some(segments) = raw.as_array() else {
            return Err(ApplyError::InvalidOp("path is not an array"));
        };
        let segments = segments
            .iter()
            .map(|segment| match segment {
                Value::String(key) if RESERVED_SEGMENTS.contains(&key.as_str()) => {
                    Err(ApplyError::UnsafePath(segment.clone()))
                }
                Value::String(key) => Ok(Segment::Key(key)),
                _ => non_negative_integer_u64(segment)
                    .map(Segment::Index)
                    .ok_or_else(|| ApplyError::UnsafePath(segment.clone())),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { raw, segments })
    }

    fn unresolvable(&self) -> ApplyError {
        ApplyError::Path(self.raw.clone())
    }

    /// Walks the path through own properties to an object or array (Chord's
    /// `resolve`).
    fn resolve<'v>(&self, root: &'v mut Value) -> Result<&'v mut Value, ApplyError> {
        let mut node = root;
        for &segment in &self.segments {
            node = match (node, segment) {
                (Value::Array(_), Segment::Key(_)) => {
                    return Err(ApplyError::UnsafePath(segment.to_value()));
                }
                (Value::Array(items), Segment::Index(index)) => usize::try_from(index)
                    .ok()
                    .and_then(|index| items.get_mut(index))
                    .ok_or_else(|| self.unresolvable())?,
                (Value::Object(map), Segment::Key(key)) => {
                    map.get_mut(key).ok_or_else(|| self.unresolvable())?
                }
                (Value::Object(map), Segment::Index(index)) => map
                    .get_mut(&index.to_string())
                    .ok_or_else(|| self.unresolvable())?,
                _ => return Err(self.unresolvable()),
            };
        }
        if node.is_object() || node.is_array() {
            Ok(node)
        } else {
            Err(self.unresolvable())
        }
    }
}

/// A validated non-empty path to a property or element (`s`, `d`, `a`, `t`):
/// its parent's path and its last segment.
struct SlotPath<'o> {
    /// The parent's path; its `raw` is the whole path.
    parent: Path<'o>,
    last: Segment<'o>,
}

impl<'o> SlotPath<'o> {
    fn parse(raw: &'o Value) -> Result<Self, ApplyError> {
        let mut parent = Path::parse(raw)?;
        let Some(last) = parent.segments.pop() else {
            return Err(ApplyError::InvalidOp("path is empty"));
        };
        Ok(Self { parent, last })
    }

    fn unresolvable(&self) -> ApplyError {
        self.parent.unresolvable()
    }

    /// Resolves the parent and checks the last segment fits it.
    fn resolve<'v>(&self, root: &'v mut Value) -> Result<(&'v mut Value, Slot), ApplyError> {
        let parent = self.parent.resolve(root)?;
        let slot = match (&*parent, self.last) {
            (Value::Array(_), Segment::Key(_)) => {
                return Err(ApplyError::UnsafePath(self.last.to_value()));
            }
            // An index may address an element or append exactly one past the
            // end.
            (Value::Array(items), Segment::Index(index)) => match usize::try_from(index) {
                Ok(index) if index <= items.len() => Slot::Index(index),
                _ => return Err(ApplyError::UnsafePath(self.last.to_value())),
            },
            (_, Segment::Key(key)) => Slot::Key(key.to_owned()),
            // JS property keys are strings: `obj[3]` is `obj["3"]`.
            (_, Segment::Index(index)) => Slot::Key(index.to_string()),
        };
        Ok((parent, slot))
    }
}

/// Where an `s`/`d`/`a`/`t` op writes under its parent: arrays pair with
/// indices, objects with keys.
enum Slot {
    Key(String),
    Index(usize),
}

impl Slot {
    fn lookup<'v>(&self, parent: &'v mut Value) -> Option<&'v mut Value> {
        match self {
            Self::Index(index) => parent.as_array_mut()?.get_mut(*index),
            Self::Key(key) => parent.as_object_mut()?.get_mut(key),
        }
    }

    fn write(self, parent: &mut Value, value: Value) {
        match (parent, self) {
            (Value::Array(items), Self::Index(index)) => {
                if index == items.len() {
                    items.push(value);
                } else {
                    items[index] = value;
                }
            }
            (Value::Object(map), Self::Key(key)) => _ = map.insert(key, value),
            _ => unreachable!("SlotPath::resolve pairs arrays with indices, objects with keys"),
        }
    }
}

/// Validates an `m` permutation: a bijection on `0..len` (Chord's
/// `assertPermutation`).
fn decode_permutation(value: &Value) -> Result<Vec<usize>, ApplyError> {
    let Some(entries) = value.as_array() else {
        return Err(ApplyError::InvalidOp("m permutation is not an array"));
    };
    let mut seen = vec![false; entries.len()];
    entries
        .iter()
        .map(|entry| {
            let index = non_negative_integer(entry)
                .filter(|&index| index < entries.len() && !seen[index])
                .ok_or(ApplyError::InvalidOp("m permutation is not a bijection"))?;
            seen[index] = true;
            Ok(index)
        })
        .collect()
}

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
