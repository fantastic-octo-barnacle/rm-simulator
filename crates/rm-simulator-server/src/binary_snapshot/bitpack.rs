// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Lossless value codec shared by the live binary checkpoints and experiments.
//! Self-contained full frames include every key. Deltas inherit key order and
//! container shape from an explicitly named baseline, never the previous packet.
use serde_json::{Map, Number, Value};
use std::io;

const LIMIT: usize = 4 << 20;
// Application assumptions, not rulebook constants. Fixed-point escapes retain
// exact f64 values when a number is outside these ranges or off these grids.
const GRIDS: [(f64, usize); 5] = [
    (1000., 18),
    (100., 16),
    (10000., 24),
    (32767., 16),
    (100000., 26),
];

fn grid(n: f64, index: usize) -> Option<i64> {
    if n.to_bits() == (-0.0_f64).to_bits() {
        return None;
    }
    let (scale, width) = GRIDS[index];
    let q = (n * scale).round();
    if q >= -(1_i64 << (width - 1)) as f64
        && q < (1_i64 << (width - 1)) as f64
        && (q / scale).to_bits() == n.to_bits()
    {
        Some(q as i64)
    } else {
        None
    }
}
fn fixed_pair(a: f64, b: f64) -> Option<(usize, i64, i64)> {
    (0..GRIDS.len()).find_map(|i| Some((i, grid(a, i)?, grid(b, i)?)))
}
fn unzigzag(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}
fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid experimental binary frame",
    )
}

/// Shared traversal accounting, including a delta's full-value fallback.
#[derive(Default)]
struct Budget {
    nodes: usize,
}
impl Budget {
    fn visit(&mut self, depth: usize) -> io::Result<()> {
        self.nodes += 1;
        if depth > 64 || self.nodes > 100_000 {
            return Err(invalid());
        }
        Ok(())
    }
}

struct Writer {
    bytes: Vec<u8>,
    bit: usize,
    packed: bool,
    fixed: bool,
    budget: Budget,
}
impl Writer {
    fn bits(&mut self, value: u64, width: usize) {
        let width = if self.packed {
            width
        } else {
            width.div_ceil(8) * 8
        };
        for i in 0..width {
            if self.bit.is_multiple_of(8) {
                self.bytes.push(0);
            }
            self.bytes[self.bit / 8] |= (((value >> i) & 1) as u8) << (self.bit % 8);
            self.bit += 1;
        }
    }
    fn var(&mut self, mut value: u64) {
        while value >= 128 {
            self.bits((value & 127) | 128, 8);
            value >>= 7;
        }
        self.bits(value, 8);
    }
    fn string(&mut self, value: &str) {
        // Keep keys and text byte-aligned so compression can reuse them even
        // when preceding numerical fields occupy different bit widths.
        if self.packed && !self.bit.is_multiple_of(8) {
            self.bits(0, 8 - self.bit % 8);
        }
        self.var(value.len() as u64);
        for byte in value.bytes() {
            self.bits(u64::from(byte), 8);
        }
    }
    fn full(&mut self, value: &Value, depth: usize) -> io::Result<()> {
        self.budget.visit(depth)?;
        match value {
            Value::Null => self.bits(0, 4),
            Value::Bool(false) => self.bits(1, 4),
            Value::Bool(true) => self.bits(2, 4),
            Value::Number(n) if n.is_f64() => {
                let n = n.as_f64().unwrap();
                if let Some((i, q, _)) = self.fixed.then(|| fixed_pair(n, n)).flatten() {
                    self.bits(9, 4);
                    self.bits(i as u64, 3);
                    self.bits((q as u64) & ((1 << GRIDS[i].1) - 1), GRIDS[i].1);
                } else {
                    self.bits(3, 4);
                    self.bits(n.to_bits(), 64);
                }
            }
            Value::Number(n) if n.is_u64() => {
                self.bits(4, 4);
                self.var(n.as_u64().unwrap());
            }
            Value::Number(n) => {
                self.bits(5, 4);
                let n = n.as_i64().unwrap();
                self.var(((n << 1) ^ (n >> 63)) as u64);
            }
            Value::String(s) => {
                self.bits(6, 4);
                self.string(s);
            }
            Value::Array(a) => {
                self.bits(7, 4);
                self.var(a.len() as u64);
                for v in a {
                    self.full(v, depth + 1)?;
                }
            }
            Value::Object(o) => {
                self.bits(8, 4);
                self.var(o.len() as u64);
                for (k, v) in o {
                    self.string(k);
                    self.full(v, depth + 1)?;
                }
            }
        }
        Ok(())
    }
    fn delta(&mut self, before: &Value, after: &Value, depth: usize) -> io::Result<()> {
        self.budget.visit(depth)?;
        let unchanged = exact(before, after);
        self.bits(u64::from(!unchanged), 1);
        if unchanged {
            return Ok(());
        }
        let same_shape = match (before, after) {
            (Value::Array(a), Value::Array(b)) => a.len() == b.len(),
            (Value::Object(a), Value::Object(b)) => a.keys().eq(b.keys()),
            (Value::Number(a), Value::Number(b)) => a.is_f64() && b.is_f64(),
            _ => false,
        };
        self.bits(u64::from(same_shape), 1);
        if !same_shape {
            self.full(after, depth + 1)?;
            return Ok(());
        }
        match (before, after) {
            (Value::Array(a), Value::Array(b)) => {
                for (a, b) in a.iter().zip(b) {
                    self.delta(a, b, depth + 1)?;
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                for (k, b) in b {
                    self.delta(&a[k], b, depth + 1)?;
                }
            }
            (Value::Number(a), Value::Number(b)) => {
                let fixed = self
                    .fixed
                    .then(|| fixed_pair(a.as_f64().unwrap(), b.as_f64().unwrap()))
                    .flatten();
                if self.fixed {
                    self.bits(u64::from(fixed.is_some()), 1);
                }
                if let Some((i, a, b)) = fixed {
                    let difference = zigzag(b - a);
                    let width = (64 - difference.leading_zeros()) as usize;
                    self.bits(i as u64, 3);
                    self.bits((width - 1) as u64, 6);
                    self.bits(difference, width);
                    return Ok(());
                }
                let xor = a.as_f64().unwrap().to_bits() ^ b.as_f64().unwrap().to_bits();
                let leading = xor.leading_zeros() as usize;
                let trailing = xor.trailing_zeros() as usize;
                let width = 64 - leading - trailing;
                self.bits(leading as u64, 6);
                self.bits((width - 1) as u64, 6);
                self.bits(xor >> trailing, width);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    bit: usize,
    packed: bool,
    budget: Budget,
    fixed: bool,
}
impl Reader<'_> {
    fn bits(&mut self, width: usize) -> io::Result<u64> {
        let stored = if self.packed {
            width
        } else {
            width.div_ceil(8) * 8
        };
        if self.bit + stored > self.bytes.len() * 8 {
            return Err(invalid());
        }
        let mut value = 0;
        for i in 0..stored {
            value |= u64::from((self.bytes[self.bit / 8] >> (self.bit % 8)) & 1) << i;
            self.bit += 1;
        }
        if width < 64 && value >> width != 0 {
            return Err(invalid());
        }
        Ok(value)
    }
    fn var(&mut self) -> io::Result<u64> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.bits(8)?;
            if shift == 63 && byte > 1 {
                return Err(invalid());
            }
            value |= (byte & 127) << shift;
            if byte < 128 {
                if shift > 0 && byte == 0 {
                    return Err(invalid());
                }
                return Ok(value);
            }
        }
        Err(invalid())
    }
    fn length(&mut self) -> io::Result<usize> {
        let len = usize::try_from(self.var()?).map_err(|_| invalid())?;
        if len > LIMIT || len > self.bytes.len() * 8 - self.bit {
            return Err(invalid());
        }
        Ok(len)
    }
    fn string(&mut self) -> io::Result<String> {
        if self.packed && !self.bit.is_multiple_of(8) && self.bits(8 - self.bit % 8)? != 0 {
            return Err(invalid());
        }
        let len = self.length()?;
        if len > (self.bytes.len() * 8 - self.bit) / 8 {
            return Err(invalid());
        }
        let bytes = (0..len)
            .map(|_| self.bits(8).map(|x| x as u8))
            .collect::<io::Result<Vec<_>>>()?;
        String::from_utf8(bytes).map_err(|_| invalid())
    }
    fn full(&mut self, depth: usize) -> io::Result<Value> {
        self.budget.visit(depth)?;
        Ok(match self.bits(4)? {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            3 => {
                Value::Number(Number::from_f64(f64::from_bits(self.bits(64)?)).ok_or_else(invalid)?)
            }
            4 => Value::from(self.var()?),
            5 => {
                let n = self.var()?;
                Value::from(((n >> 1) as i64) ^ -((n & 1) as i64))
            }
            6 => Value::String(self.string()?),
            7 => {
                let len = self.length()?;
                Value::Array(
                    (0..len)
                        .map(|_| self.full(depth + 1))
                        .collect::<io::Result<Vec<_>>>()?,
                )
            }
            8 => {
                let len = self.length()?;
                let mut map = Map::new();
                for _ in 0..len {
                    let key = self.string()?;
                    if map.insert(key, self.full(depth + 1)?).is_some() {
                        return Err(invalid());
                    }
                }
                Value::Object(map)
            }
            9 if self.fixed => {
                let i = self.bits(3)? as usize;
                if i >= GRIDS.len() {
                    return Err(invalid());
                }
                let width = GRIDS[i].1;
                let raw = self.bits(width)?;
                let q = ((raw << (64 - width)) as i64) >> (64 - width);
                Value::from(q as f64 / GRIDS[i].0)
            }
            _ => return Err(invalid()),
        })
    }
    fn delta(&mut self, before: &Value, depth: usize) -> io::Result<Value> {
        self.budget.visit(depth)?;
        if self.bits(1)? == 0 {
            return Ok(before.clone());
        }
        if self.bits(1)? == 0 {
            return self.full(depth + 1);
        }
        Ok(match before {
            Value::Array(a) => Value::Array(
                a.iter()
                    .map(|v| self.delta(v, depth + 1))
                    .collect::<io::Result<Vec<_>>>()?,
            ),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| Ok((k.clone(), self.delta(v, depth + 1)?)))
                    .collect::<io::Result<Map<_, _>>>()?,
            ),
            Value::Number(n) if n.is_f64() => {
                if self.fixed && self.bits(1)? != 0 {
                    let i = self.bits(3)? as usize;
                    if i >= GRIDS.len() {
                        return Err(invalid());
                    }
                    let a = grid(n.as_f64().unwrap(), i).ok_or_else(invalid)?;
                    let width = self.bits(6)? as usize + 1;
                    let b = a
                        .checked_add(unzigzag(self.bits(width)?))
                        .ok_or_else(invalid)?;
                    let value = b as f64 / GRIDS[i].0;
                    if grid(value, i) != Some(b) {
                        return Err(invalid());
                    }
                    return Ok(Value::from(value));
                }
                let leading = self.bits(6)? as usize;
                let width = self.bits(6)? as usize + 1;
                if leading + width > 64 {
                    return Err(invalid());
                }
                let xor = self.bits(width)? << (64 - leading - width);
                Value::Number(
                    Number::from_f64(f64::from_bits(n.as_f64().unwrap().to_bits() ^ xor))
                        .ok_or_else(invalid)?,
                )
            }
            _ => return Err(invalid()),
        })
    }
}

/// Compare floating-point bits too: JSON value equality hides signed zero.
pub fn exact(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
            a.is_f64() == b.is_f64()
                && a.as_f64().unwrap().to_bits() == b.as_f64().unwrap().to_bits()
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| exact(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.keys().eq(b.keys()) && a.iter().all(|(k, v)| exact(v, &b[k]))
        }
        _ => a == b,
    }
}

/// Encode a full frame or a delta. `base_id == 0` means independent; a positive
/// id with no baseline proposes a retained full frame. Header includes an epoch
/// and baseline identity, so no schema or dictionary is supplied out of band.
/// Returns an error when traversal exceeds 64 levels or 100,000 visited values,
/// the frame exceeds 4 MiB, or a delta has a zero baseline id. Unchanged delta
/// subtrees consume one visit, matching the decoder.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{encode, decode};
/// let state = serde_json::json!({"tick": 42, "x": 1.25});
/// let bytes = encode(&state, None, 3, 0, true, true).unwrap();
/// assert_eq!(decode(&bytes, None, 3).unwrap().0, state);
/// ```
pub fn encode(
    value: &Value,
    baseline: Option<&Value>,
    epoch: u64,
    base_id: u64,
    packed: bool,
    fixed: bool,
) -> io::Result<Vec<u8>> {
    if baseline.is_some() && base_id == 0 {
        return Err(invalid());
    }
    let mut w = Writer {
        bytes: b"RMB0".to_vec(),
        bit: 32,
        packed,
        fixed,
        budget: Budget::default(),
    };
    w.bits(u64::from(packed) | (u64::from(fixed) << 1), 8);
    w.bits(u64::from(baseline.is_some()), 1);
    w.var(epoch);
    w.var(base_id);
    if let Some(base) = baseline {
        w.delta(base, value, 0)?;
    } else {
        w.full(value, 0)?;
    }
    if w.bytes.len() > LIMIT {
        return Err(invalid());
    }
    Ok(w.bytes)
}

/// Inspect the bounded frame header: delta flag, epoch and baseline id. This
/// does not validate the body; callers must still call `decode` with a pinned
/// baseline before delivering any state.
pub fn header(bytes: &[u8]) -> io::Result<(bool, u64, u64)> {
    if bytes.len() > LIMIT || !bytes.starts_with(b"RMB0") || bytes.get(4).is_none_or(|b| *b > 3) {
        return Err(invalid());
    }
    let mut r = Reader {
        bytes,
        bit: 40,
        packed: bytes[4] & 1 != 0,
        budget: Budget::default(),
        fixed: bytes[4] & 2 != 0,
    };
    Ok((r.bits(1)? != 0, r.var()?, r.var()?))
}

/// Decode against only the named retained baseline. The decoder limits
/// bytes, depth, node count, lengths, tags, finite floats and trailing padding.
pub fn decode(
    bytes: &[u8],
    baseline: Option<(&Value, u64)>,
    epoch: u64,
) -> io::Result<(Value, u64)> {
    if bytes.len() > LIMIT || !bytes.starts_with(b"RMB0") || bytes.get(4).is_none_or(|b| *b > 3) {
        return Err(invalid());
    }
    let mut r = Reader {
        bytes,
        bit: 40,
        packed: bytes[4] & 1 != 0,
        budget: Budget::default(),
        fixed: bytes[4] & 2 != 0,
    };
    let delta = r.bits(1)? != 0;
    if r.var()? != epoch {
        return Err(invalid());
    }
    let id = r.var()?;
    let value = if delta {
        let (base, expected_id) = baseline.ok_or_else(invalid)?;
        if id == 0 || id != expected_id {
            return Err(invalid());
        }
        r.delta(base, 0)?
    } else {
        r.full(0)?
    };
    if r.bit.div_ceil(8) != bytes.len() {
        return Err(invalid());
    }
    if r.packed && !r.bit.is_multiple_of(8) && bytes[bytes.len() - 1] >> (r.bit % 8) != 0 {
        return Err(invalid());
    }
    Ok((value, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lossless_types_shapes_extremes_and_signed_zero() {
        let states = [
            json!({"float": -0.0, "balls": [], "opt": null, "int": u64::MAX}),
            json!({"float": 0.0, "balls": [1.25, true, "红"], "opt": 4, "int": i64::MIN}),
            json!({"float": f64::MAX, "balls": [1.250000000001, false, "blue"], "opt": null, "int": -1}),
            json!({"float": f64::MIN_POSITIVE, "new": [f64::from_bits(1), -f64::MAX]}),
        ];
        for packed in [false, true] {
            for state in &states {
                let bytes = encode(state, None, 7, 1, packed, true).unwrap();
                assert!(exact(&decode(&bytes, None, 7).unwrap().0, state));
                for base in &states {
                    let bytes = encode(state, Some(base), 7, 1, packed, true).unwrap();
                    assert!(exact(&decode(&bytes, Some((base, 1)), 7).unwrap().0, state));
                    assert!(decode(&bytes, None, 7).is_err());
                    assert!(decode(&bytes, Some((base, 2)), 7).is_err());
                    assert!(decode(&bytes, Some((base, 1)), 8).is_err());
                }
            }
        }
    }

    #[test]
    fn truncation_trailing_bytes_and_hostile_input_are_rejected_without_panics() {
        for flags in 0_u8..4 {
            let packed = flags & 1 != 0;
            let state = json!({"x": [1.25, "test", false], "v": u64::MAX});
            let mut bytes = encode(&state, None, 0, 0, packed, flags & 2 != 0).unwrap();
            for len in 0..bytes.len() {
                assert!(decode(&bytes[..len], None, 0).is_err());
            }
            bytes.push(0);
            assert!(decode(&bytes, None, 0).is_err());
            // Deterministic malformed bodies exercise lengths, varints and tags.
            let mut seed = 1_u64;
            for delta in [false, true] {
                for len in 0..256 {
                    let mut writer = Writer {
                        bytes: b"RMB0".to_vec(),
                        bit: 32,
                        packed,
                        fixed: flags & 2 != 0,
                        budget: Budget::default(),
                    };
                    writer.bits(u64::from(flags), 8);
                    writer.bits(u64::from(delta), 1);
                    writer.var(0);
                    writer.var(1);
                    for _ in 0..len {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        writer.bits((seed >> 32) & 255, 8);
                    }
                    let _ = decode(&writer.bytes, Some((&state, 1)), 0);
                }
            }
        }
    }

    #[test]
    fn full_and_delta_traversal_limits_match() {
        for packed in [false, true] {
            for fixed in [false, true] {
                // Full arrays visit the root plus each element; changed integer
                // deltas additionally visit a full replacement for each leaf.
                for (base, leaf, accepted) in [
                    (None, Value::Null, 99_999),
                    (Some(Value::from(0)), Value::from(1), 49_999),
                ] {
                    for len in [accepted, accepted + 1] {
                        let value = Value::Array(vec![leaf.clone(); len]);
                        let baseline = base.as_ref().map(|v| Value::Array(vec![v.clone(); len]));
                        let encoded = encode(&value, baseline.as_ref(), 0, 1, packed, fixed);
                        if len == accepted {
                            let bytes = encoded.unwrap();
                            let decoded = decode(&bytes, baseline.as_ref().map(|b| (b, 1)), 0)
                                .unwrap()
                                .0;
                            assert!(exact(&decoded, &value));
                        } else {
                            assert!(encoded.is_err());
                        }
                    }
                }
                for depth in [63, 64, 65] {
                    let mut value = Value::from(1);
                    let mut baseline = Value::from(0);
                    for _ in 0..depth {
                        value = Value::Array(vec![value]);
                        baseline = Value::Array(vec![baseline]);
                    }
                    for base in [None, Some(&baseline)] {
                        let encoded = encode(&value, base, 0, 1, packed, fixed);
                        let limit = if base.is_some() { 63 } else { 64 };
                        if depth <= limit {
                            assert!(exact(
                                &decode(&encoded.unwrap(), base.map(|b| (b, 1)), 0)
                                    .unwrap()
                                    .0,
                                &value
                            ));
                        } else {
                            assert!(encoded.is_err());
                        }
                    }
                }
                // An unchanged subtree counts as one delta visit, even when
                // expanding it as a full frame would exceed the node budget.
                let value = Value::Array(vec![Value::Null; 100_000]);
                let bytes = encode(&value, Some(&value), 0, 1, packed, fixed).unwrap();
                assert!(exact(
                    &decode(&bytes, Some((&value, 1)), 0).unwrap().0,
                    &value
                ));
            }
        }
    }

    #[test]
    fn independent_baseline_survives_lost_reordered_and_duplicate_deltas() {
        let base = json!({"tick": 1, "pos": [1.0, 2.0, 3.0]});
        for packed in [false, true] {
            let a = json!({"tick": 2, "pos": [1.1, 2.2, 3.3]});
            let b = json!({"tick": 3, "pos": [1.2, 2.4, 3.6]});
            let packet_a = encode(&a, Some(&base), 0, 9, packed, true).unwrap();
            let packet_b = encode(&b, Some(&base), 0, 9, packed, true).unwrap();
            for (packet, expected) in [(&packet_b, &b), (&packet_b, &b), (&packet_a, &a)] {
                assert!(exact(
                    &decode(packet, Some((&base, 9)), 0).unwrap().0,
                    expected
                ));
            }
        }
    }
}
