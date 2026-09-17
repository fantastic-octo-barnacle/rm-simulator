// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Positional binary codec for the serde data model, shared by the live
//! checkpoints, every protocol message and the experiments.
//!
//! A value is first serialized into a [`Node`] tree that carries no field names
//! and no type tags: struct fields and tuples are positional, sequences and
//! maps carry a length, options a presence bit and enum variants an Elias-gamma
//! index (one bit for the first variant, three for the next two). The decoder
//! is driven by the Rust type, so nothing on the wire says what a value is;
//! both ends must agree on the types, which the protocol version guarantees.
//!
//! Deltas inherit shape from an explicitly named baseline tree, never the
//! previous packet. Every value in a delta costs one "changed" bit; a changed
//! map, option or enum adds one "same shape" bit and either recurses against
//! the baseline or carries the new value in full. A changed sequence either
//! pairs element by element with an equal-length baseline, or names how many
//! leading baseline elements it drops and its new length, pairs the rest in
//! order and carries elements past the baseline's end in full; the encoder
//! aligns records on an integer or text first field such as an id. A delta
//! header also carries a caller-defined hint ([`encode_hinted`]), which lets a
//! caller transform the baseline (for instance predict it forward) the same
//! way on both ends before differencing. A [`Rounding`] rule
//! chooses by field path how `f64` values pack, and the decoder walks the same
//! path, so no grid index travels: a [`Fixed::Grid`] value is a flag bit and
//! its step count, and in a delta an exponential-Golomb step difference; a
//! [`Fixed::Rotation`] quaternion is smallest-three. Unrounded values keep the
//! tagged form: a legacy grid search, then 32 bits when `f32` holds them
//! exactly and 64 bits otherwise.
//!
//! An [`Aligned`] value starts on a byte boundary in a full frame, so slowly
//! changing records keep the same bytes at the same bit phase from frame to
//! frame and the dictionary can match them; a delta ignores the alignment.
//!
//! Types that need a self-describing format (`#[serde(flatten)]`, untagged or
//! internally tagged enums, `skip_serializing_if`) are refused rather than
//! guessed at.
use serde::de::{self, DeserializeOwned, IntoDeserializer};
use serde::{Serialize, ser};
use std::{fmt, io};

/// First four bytes of every packed checkpoint [`encode`] writes. A frame that
/// starts with it is already the inflated checkpoint, so the wire framing above
/// it passes it through unchanged.
pub const MAGIC: &[u8; 4] = b"RMB1";
const LIMIT: usize = 4 << 20;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 64;
// Application assumptions, not rulebook constants. Values off these grids or
// outside their ranges keep their exact bits.
const GRIDS: [(f64, u32); 5] = [
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
/// A fixed-point grid selected by a [`Rounding`] rule: values round to
/// `1 / scale` steps held in a signed integer of `width` bits, and a delta
/// codes the step difference as an order-`k` exponential-Golomb number. The
/// grid is a property of the field path, so neither end names it on the wire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Steps per unit.
    pub scale: f64,
    /// Signed integer width in bits the step count must fit.
    pub width: u32,
    /// Exponential-Golomb order for a delta's step difference; the typical
    /// difference magnitude in bits.
    pub k: u32,
}
impl Grid {
    /// `value` rounded onto the grid, or `value` itself, exact, when the
    /// rounded step count is nonfinite or outside `width` bits. A zero step is
    /// positive zero. Idempotent: a rounded value rounds to itself.
    ///
    /// ```
    /// use rm_simulator_server::binary_snapshot::bitpack::Grid;
    /// let mm = Grid { scale: 1000., width: 18, k: 6 };
    /// assert_eq!(mm.round(1.23456), 1.235);
    /// assert_eq!(mm.round(200.), 200.);
    /// assert_eq!(mm.round(-0.0001).to_bits(), 0.0_f64.to_bits());
    /// ```
    pub fn round(self, value: f64) -> f64 {
        let q = (value * self.scale).round();
        let bound = (1_u64 << (self.width - 1)) as f64;
        if !q.is_finite() || q < -bound || q >= bound {
            return value;
        }
        q / self.scale + 0.0
    }
    /// The step count of `value` when it is exactly on this grid.
    fn steps(self, n: f64) -> Option<i64> {
        if n.to_bits() == (-0.0_f64).to_bits() {
            return None;
        }
        let q = (n * self.scale).round();
        let bound = (1_i64 << (self.width - 1)) as f64;
        (q >= -bound && q < bound && (q / self.scale).to_bits() == n.to_bits()).then_some(q as i64)
    }
}

/// How a [`Rounding`] rule packs the `f64` values beneath it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fixed {
    /// Exact: a legacy grid search, then `f32` or `f64` bits.
    Exact,
    /// Rounded onto this grid, which the wire does not name.
    Grid(Grid),
    /// A four-element tuple is a unit quaternion sent smallest-three: the
    /// index of its largest-magnitude component, then the other three,
    /// sign-normalized so the largest is positive, on a
    /// [`ROTATION_BITS`]-bit grid spanning ±1/√2. The largest is rebuilt as
    /// `sqrt(1 - sum of squares)`. A non-unit or unstable quaternion keeps its
    /// exact bits. Lone `f64` values beneath stay exact.
    Rotation,
}

/// Signed bits per smallest-three quaternion component. Application choice,
/// not a rule constant: over 200,000 random unit quaternions 15 bits measure a
/// 1.3e-4 rad worst and 4.7e-5 rad mean rotation error, against 5.8e-5 and
/// 2.9e-5 rad for the earlier four 16-bit components (51 bits against 84).
pub const ROTATION_BITS: u32 = 15;
/// Steps per unit for smallest-three components: the largest scale whose
/// ±1/√2 range fits [`ROTATION_BITS`] signed bits.
const ROTATION_SCALE: f64 = ((1_u64 << (ROTATION_BITS - 1)) - 1) as f64 * std::f64::consts::SQRT_2;
/// Exponential-Golomb order for smallest-three component step differences
/// against a dead-reckoned baseline (protocol 42), swept with the grid orders
/// in `fixed_point`.
const ROTATION_K: u32 = 4;
/// Largest accepted |norm² - 1| for a smallest-three quaternion. It must admit
/// a tie-raised reconstruction (about 1e-4 off unit at most); physics
/// quaternions are unit far more closely.
const ROTATION_NORM_TOLERANCE: f64 = 1e-3;

/// A smallest-three code: largest-component index and the other three steps.
type RotationCode = (u8, [i64; 3]);

fn rotation_quantize(x: [f64; 4]) -> Option<RotationCode> {
    if !x.iter().all(|v| v.is_finite()) {
        return None;
    }
    let norm = x.iter().map(|v| v * v).sum::<f64>();
    if (norm - 1.).abs() > ROTATION_NORM_TOLERANCE {
        return None;
    }
    let mut index = 0;
    for i in 1..4 {
        if x[i].abs() > x[index].abs() {
            index = i;
        }
    }
    let sign = if x[index] < 0. { -1. } else { 1. };
    let bound = (1_i64 << (ROTATION_BITS - 1)) as f64;
    let mut steps = [0; 3];
    for (slot, i) in (0..4).filter(|&i| i != index).enumerate() {
        let q = (sign * x[i] * ROTATION_SCALE).round();
        if q < -bound || q >= bound {
            return None;
        }
        steps[slot] = q as i64;
    }
    Some((index as u8, steps))
}
fn rotation_value((index, steps): RotationCode) -> [f64; 4] {
    let others = steps.map(|q| q as f64 / ROTATION_SCALE + 0.0);
    let sum = others.iter().map(|v| v * v).sum::<f64>();
    // Near a tie the rounded others can exceed the rebuilt largest. Raising it
    // to an exact tie makes the lower index win when the value is quantized
    // again, which then reproduces itself instead of flipping back and forth.
    let largest = others
        .iter()
        .fold((1. - sum).max(0.).sqrt(), |m, v| m.max(v.abs()));
    let mut x = [0.; 4];
    let mut rest = others.into_iter();
    for (i, slot) in x.iter_mut().enumerate() {
        *slot = if i == usize::from(index) {
            largest
        } else {
            rest.next().unwrap_or(0.)
        };
    }
    x
}
/// The code a quaternion packs as, when there is one both ends reproduce: the
/// code must quantize its own reconstruction back to itself, so a decoder that
/// rebuilds its baseline from the decoded value lands on the same code. A
/// near-tie between the two largest components can move the index once; a
/// value with no fixed point within three steps keeps its exact bits, which
/// the decoder's rebuild reproduces because this is a pure function.
fn rotation_code(x: [f64; 4]) -> Option<RotationCode> {
    let mut code = rotation_quantize(x)?;
    for _ in 0..3 {
        let again = rotation_quantize(rotation_value(code))?;
        if again == code {
            return Some(code);
        }
        code = again;
    }
    None
}

/// Bits in one [`smallest_three`] packing: a 2-bit index and three
/// [`ROTATION_BITS`]-bit components.
pub const SMALLEST_THREE_BITS: u32 = 2 + 3 * ROTATION_BITS;

/// A near-unit quaternion packed smallest-three into the low
/// [`SMALLEST_THREE_BITS`] bits, for fixed layouts with no baseline such as the
/// owner anchor. `None` for a non-finite or non-unit quaternion.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{from_smallest_three, smallest_three};
/// let yaw = [0.8_f64.sqrt(), 0., 0., -0.2_f64.sqrt()];
/// let back = from_smallest_three(smallest_three(yaw).unwrap()).unwrap();
/// assert!(yaw.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-4));
/// assert_eq!(smallest_three([2., 0., 0., 0.]), None);
/// ```
pub fn smallest_three(x: [f64; 4]) -> Option<u64> {
    let (index, steps) = rotation_quantize(x)?;
    let mask = (1_u64 << ROTATION_BITS) - 1;
    Some(
        steps
            .iter()
            .enumerate()
            .fold(u64::from(index), |packed, (i, q)| {
                packed | ((*q as u64 & mask) << (2 + ROTATION_BITS * i as u32))
            }),
    )
}
/// The quaternion a [`smallest_three`] packing rebuilds, its largest component
/// positive; `None` when bits above [`SMALLEST_THREE_BITS`] are set.
pub fn from_smallest_three(packed: u64) -> Option<[f64; 4]> {
    if packed >> SMALLEST_THREE_BITS != 0 {
        return None;
    }
    let shift = 64 - ROTATION_BITS;
    let steps = [0, 1, 2].map(|i| ((packed >> (2 + ROTATION_BITS * i) << shift) as i64) >> shift);
    Some(rotation_value(((packed & 3) as u8, steps)))
}

fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}
fn unzigzag(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}
fn exact_f32(n: f64) -> bool {
    f64::from(n as f32).to_bits() == n.to_bits()
}

/// Codec failure: a malformed frame, a type the positional format cannot carry,
/// or a traversal limit.
#[derive(Debug)]
pub struct Error(String);
impl Error {
    fn new(message: impl fmt::Display) -> Self {
        Self(message.to_string())
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "binary codec: {}", self.0)
    }
}
impl std::error::Error for Error {}
impl ser::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message)
    }
}
impl de::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::new(message)
    }
}
impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, error)
    }
}
type Result<T, E = Error> = std::result::Result<T, E>;
fn invalid() -> Error {
    Error::new("invalid frame")
}

/// One value in the serde data model with its names stripped. Retained
/// baselines are kept in this form, so a delta can be decoded without
/// re-encoding the baseline.
#[derive(Clone, Debug, Default)]
pub enum Node {
    /// `()`, a unit struct, or the payload of a unit variant. Zero bits.
    #[default]
    Unit,
    /// One bit.
    Bool(bool),
    /// Any unsigned integer or `char`, as a varint.
    Uint(u64),
    /// Any signed integer, zigzag varint.
    Int(i64),
    /// 32 raw bits.
    F32(f32),
    /// An unrounded `f64`: legacy grid search, exact `f32` or 64 raw bits.
    F64(f64),
    /// An `f64` under a [`Fixed::Grid`] rule, already rounded: a flag bit and
    /// `width` bits on the grid, otherwise its exact bits.
    Fixed(Grid, f64),
    /// A quaternion under [`Fixed::Rotation`]: its smallest-three code when it
    /// has one, and the value that code rebuilds (or the exact value).
    Rotation(Option<RotationCode>, [f64; 4]),
    /// Length-prefixed UTF-8, byte-aligned.
    Text(String),
    /// Length-prefixed bytes, byte-aligned.
    Bytes(Vec<u8>),
    /// Absent option: one bit.
    None,
    /// Present option: one bit, then the value.
    Some(Box<Node>),
    /// Struct or tuple: fields in declaration order, no length.
    Tuple(Vec<Node>),
    /// Sequence: varint length, then elements.
    Seq(Vec<Node>),
    /// Map: varint length, then key and value pairs.
    Map(Vec<(Node, Node)>),
    /// Enum: gamma-coded variant index, then the payload.
    Variant(u32, Box<Node>),
    /// An [`Aligned`] value: zero padding to a byte boundary in a full frame,
    /// then the value. A delta codes the value alone.
    Aligned(Box<Node>),
}
impl PartialEq for Node {
    /// Bit-exact equality: signed zero and NaN payloads differ.
    fn eq(&self, other: &Self) -> bool {
        use Node::*;
        match (self, other) {
            (Unit, Unit) | (None, None) => true,
            (Bool(a), Bool(b)) => a == b,
            (Uint(a), Uint(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (F32(a), F32(b)) => a.to_bits() == b.to_bits(),
            (F64(a), F64(b)) | (Fixed(_, a), Fixed(_, b)) => a.to_bits() == b.to_bits(),
            (Rotation(i, a), Rotation(j, b)) => {
                i == j && a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
            }
            (Text(a), Text(b)) => a == b,
            (Bytes(a), Bytes(b)) => a == b,
            (Some(a), Some(b)) => a == b,
            (Tuple(a), Tuple(b)) | (Seq(a), Seq(b)) => a == b,
            (Map(a), Map(b)) => a == b,
            (Variant(i, a), Variant(j, b)) => i == j && a == b,
            (Aligned(a), Aligned(b)) => a == b,
            _ => false,
        }
    }
}

/// Serde newtype name that marks an [`Aligned`] value; no other type uses it.
const ALIGNED: &str = "\u{0}bitpack::Aligned";

/// A value that starts on a byte boundary in a full frame. Wrap slowly
/// changing records (and each element of a slowly changing sequence) in it so
/// they keep a fixed byte phase whatever bit widths precede them, which is
/// what lets a dictionary match them. In a delta the value is coded as if it
/// were unwrapped. Other serde formats see a plain newtype, i.e. `T` itself.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{Aligned, from_bytes, to_bytes};
/// // One bit, then seven bits of padding, then the byte-aligned value.
/// let value = (true, Aligned(0x55_u8));
/// let bytes = to_bytes(&value).unwrap();
/// assert_eq!(bytes, [0x01, 0x55]);
/// assert_eq!(from_bytes::<(bool, Aligned<u8>)>(&bytes).unwrap(), value);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aligned<T>(pub T);
impl<T> std::ops::Deref for Aligned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T> std::ops::DerefMut for Aligned<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T: Serialize> Serialize for Aligned<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(ALIGNED, &self.0)
    }
}
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Aligned<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit<T>(std::marker::PhantomData<T>);
        impl<'de, T: serde::Deserialize<'de>> de::Visitor<'de> for Visit<T> {
            type Value = Aligned<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an aligned value")
            }
            fn visit_newtype_struct<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                T::deserialize(deserializer).map(Aligned)
            }
        }
        deserializer.deserialize_newtype_struct(ALIGNED, Visit(std::marker::PhantomData))
    }
}

/// Fixed-point rounding applied while a value is serialized, selected by the
/// field names on the path to it. The codec itself carries no names.
pub trait Rounding: Copy {
    /// The rule for field `key` of a struct under this rule.
    fn field(self, key: &'static str) -> Self;
    /// How `f64` values directly under this rule pack. The decoder asks the
    /// same rule along the same path, so nothing about it travels.
    fn fixed(self) -> Fixed;
}
/// Encode every value exactly.
#[derive(Clone, Copy)]
pub struct Exact;
impl Rounding for Exact {
    fn field(self, _: &'static str) -> Self {
        self
    }
    fn fixed(self) -> Fixed {
        Fixed::Exact
    }
}

/// The tree for `value`, with every number exact.
pub fn to_node<T: Serialize + ?Sized>(value: &T) -> Result<Node> {
    to_node_with(value, Exact)
}
/// The tree for `value`, rounding `f64` values by `rule`.
pub fn to_node_with<T: Serialize + ?Sized, R: Rounding>(value: &T, rule: R) -> Result<Node> {
    value.serialize(NodeSerializer(rule))
}
/// Decodes a tree produced by [`to_node`] or [`to_node_with`] into `T`.
pub fn from_node<T: DeserializeOwned>(node: &Node) -> Result<T> {
    T::deserialize(NodeDe(node))
}

/// A value's standalone bytes, without a frame header. Used for protocol
/// messages, which have no baseline.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{from_bytes, to_bytes};
/// let value = (7_u32, Some(1.25_f64), vec![true, false], "red".to_string());
/// let bytes = to_bytes(&value).unwrap();
/// assert_eq!(from_bytes::<(u32, Option<f64>, Vec<bool>, String)>(&bytes).unwrap(), value);
/// ```
pub fn to_bytes<T: Serialize + ?Sized>(value: &T) -> io::Result<Vec<u8>> {
    let mut w = Writer::default();
    w.full(&to_node(value)?, 0)?;
    w.finish()
}
/// Decodes [`to_bytes`] output, refusing trailing bytes.
pub fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > LIMIT {
        return Err(invalid().into());
    }
    let mut r = Reader::new(bytes, 0);
    let value = T::deserialize(BitDe {
        r: &mut r,
        base: None,
        depth: 0,
        rule: Exact,
    })?;
    r.finish()?;
    Ok(value)
}

/// Encode a full frame or a delta against `baseline`. `base_id == 0` means
/// independent; a positive id with no baseline proposes a retained full frame.
/// The header carries the epoch and baseline identity, so no schema or
/// dictionary is supplied out of band. Errors past 64 levels, 100,000 values or
/// a 4 MiB frame, or for a delta with a zero baseline id or a baseline of a
/// different type.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{decode, encode, to_node};
/// let before = to_node(&(41_u64, [1.25_f64, 2.5])).unwrap();
/// let after = to_node(&(42_u64, [1.25_f64, 2.75])).unwrap();
/// let bytes = encode(&after, Some(&before), 3, 1).unwrap();
/// let (value, id) = decode::<(u64, [f64; 2])>(&bytes, Some((&before, 1)), 3).unwrap();
/// assert_eq!((value, id), ((42, [1.25, 2.75]), 1));
/// ```
pub fn encode(
    node: &Node,
    baseline: Option<&Node>,
    epoch: u64,
    base_id: u64,
) -> io::Result<Vec<u8>> {
    encode_hinted(node, baseline, epoch, base_id, 0)
}

/// [`encode`] with a caller-defined `hint` in a delta frame's header, after the
/// baseline id. The codec gives it no meaning; a caller that transforms the
/// named baseline before differencing (such as predicting it forward in time)
/// puts the transform's parameter here, so the decoder can read it with
/// [`hint`] and apply the same transform before decoding the body. A full frame
/// carries no hint and ignores `hint`.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{encode, encode_hinted, hint, to_node};
/// let before = to_node(&[1_u32, 2]).unwrap();
/// let after = to_node(&[1_u32, 3]).unwrap();
/// let delta = encode_hinted(&after, Some(&before), 3, 1, 300).unwrap();
/// assert_eq!(hint(&delta).unwrap(), 300);
/// assert_eq!(hint(&encode(&after, None, 3, 1).unwrap()).unwrap(), 0);
/// ```
pub fn encode_hinted(
    node: &Node,
    baseline: Option<&Node>,
    epoch: u64,
    base_id: u64,
    hint: u64,
) -> io::Result<Vec<u8>> {
    if baseline.is_some() && base_id == 0 {
        return Err(invalid().into());
    }
    let mut w = Writer::default();
    w.bytes.extend_from_slice(MAGIC);
    w.bit = 32;
    w.bits(u64::from(baseline.is_some()), 1);
    w.var(epoch);
    w.var(base_id);
    if baseline.is_some() {
        w.var(hint);
    }
    match baseline {
        Some(base) => w.delta(base, node, 0)?,
        None => w.full(node, 0)?,
    }
    w.finish()
}

/// Inspect the bounded frame header: delta flag, epoch and baseline id. This
/// does not validate the body; callers must still call [`decode`] with a pinned
/// baseline before delivering any state.
pub fn header(bytes: &[u8]) -> io::Result<(bool, u64, u64)> {
    if bytes.len() > LIMIT || !bytes.starts_with(MAGIC) {
        return Err(invalid().into());
    }
    let mut r = Reader::new(bytes, 32);
    Ok((r.bit1()?, r.var()?, r.var()?))
}

/// The [`encode_hinted`] hint of a delta frame, or zero for a full frame. Like
/// [`header`], this does not validate the body.
pub fn hint(bytes: &[u8]) -> io::Result<u64> {
    let (delta, ..) = header(bytes)?;
    if !delta {
        return Ok(0);
    }
    let mut r = Reader::new(bytes, 33);
    r.var()?;
    r.var()?;
    Ok(r.var()?)
}

/// Decode against only the named retained baseline, with every number exact.
/// The decoder limits bytes, depth, value count, lengths, variant indexes and
/// trailing padding.
pub fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    baseline: Option<(&Node, u64)>,
    epoch: u64,
) -> io::Result<(T, u64)> {
    decode_with(bytes, baseline, epoch, Exact)
}

/// [`decode`] for a frame whose tree was built by [`to_node_with`] under
/// `rule`: the rule supplies every grid and quaternion form the wire omits.
/// Rebuilding a baseline from the decoded value with the same rule gives the
/// encoder's tree bit for bit.
///
/// ```
/// use rm_simulator_server::binary_snapshot::bitpack::{
///     Fixed, Grid, Rounding, decode_with, encode, to_node_with,
/// };
/// #[derive(Clone, Copy)]
/// struct Millimetres;
/// impl Rounding for Millimetres {
///     fn field(self, _: &'static str) -> Self { self }
///     fn fixed(self) -> Fixed { Fixed::Grid(Grid { scale: 1000., width: 18, k: 6 }) }
/// }
/// let node = to_node_with(&[1.23456_f64, -2.0], Millimetres).unwrap();
/// let bytes = encode(&node, None, 0, 0).unwrap();
/// let (value, _) = decode_with::<[f64; 2], _>(&bytes, None, 0, Millimetres).unwrap();
/// assert_eq!(value, [1.235, -2.0]);
/// assert_eq!(to_node_with(&value, Millimetres).unwrap(), node);
/// ```
pub fn decode_with<T: DeserializeOwned, R: Rounding>(
    bytes: &[u8],
    baseline: Option<(&Node, u64)>,
    epoch: u64,
    rule: R,
) -> io::Result<(T, u64)> {
    let (delta, frame_epoch, id) = header(bytes)?;
    if frame_epoch != epoch {
        return Err(invalid().into());
    }
    let base = if delta {
        let (base, expected) = baseline.ok_or_else(invalid)?;
        if id == 0 || id != expected {
            return Err(invalid().into());
        }
        Some(base)
    } else {
        None
    };
    let mut r = Reader::new(bytes, 32);
    r.bit1()?;
    r.var()?;
    r.var()?;
    if delta {
        r.var()?;
    }
    let value = T::deserialize(BitDe {
        r: &mut r,
        base,
        depth: 0,
        rule,
    })?;
    r.finish()?;
    Ok((value, id))
}

// ---------------------------------------------------------------------------
// Serialization into a tree.

struct NodeSerializer<R>(R);

enum Kind {
    Seq,
    Tuple,
    /// A four-element tuple under [`Fixed::Rotation`].
    Rotation,
    Variant(u32),
}
#[doc(hidden)]
pub struct Items<R> {
    rule: R,
    items: Vec<Node>,
    kind: Kind,
}
impl<R> Items<R> {
    fn finish(self) -> Result<Node> {
        Ok(match self.kind {
            Kind::Seq => Node::Seq(self.items),
            Kind::Tuple => Node::Tuple(self.items),
            Kind::Rotation => {
                let mut x = [0.; 4];
                for (slot, item) in x.iter_mut().zip(&self.items) {
                    let Node::F64(v) = item else {
                        return Err(Error::new("a rotation rule needs four f64 values"));
                    };
                    *slot = *v;
                }
                match rotation_code(x) {
                    Some(code) => Node::Rotation(Some(code), rotation_value(code)),
                    None => Node::Rotation(None, x),
                }
            }
            Kind::Variant(index) => Node::Variant(index, Box::new(Node::Tuple(self.items))),
        })
    }
}
#[doc(hidden)]
pub struct Entries<R> {
    rule: R,
    entries: Vec<(Node, Node)>,
    key: Option<Node>,
}

impl<R: Rounding> ser::Serializer for NodeSerializer<R> {
    type Ok = Node;
    type Error = Error;
    type SerializeSeq = Items<R>;
    type SerializeTuple = Items<R>;
    type SerializeTupleStruct = Items<R>;
    type SerializeTupleVariant = Items<R>;
    type SerializeMap = Entries<R>;
    type SerializeStruct = Items<R>;
    type SerializeStructVariant = Items<R>;

    fn is_human_readable(&self) -> bool {
        false
    }
    fn serialize_bool(self, v: bool) -> Result<Node> {
        Ok(Node::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Node> {
        Ok(Node::Int(v.into()))
    }
    fn serialize_i16(self, v: i16) -> Result<Node> {
        Ok(Node::Int(v.into()))
    }
    fn serialize_i32(self, v: i32) -> Result<Node> {
        Ok(Node::Int(v.into()))
    }
    fn serialize_i64(self, v: i64) -> Result<Node> {
        Ok(Node::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<Node> {
        Ok(Node::Uint(v.into()))
    }
    fn serialize_u16(self, v: u16) -> Result<Node> {
        Ok(Node::Uint(v.into()))
    }
    fn serialize_u32(self, v: u32) -> Result<Node> {
        Ok(Node::Uint(v.into()))
    }
    fn serialize_u64(self, v: u64) -> Result<Node> {
        Ok(Node::Uint(v))
    }
    fn serialize_f32(self, v: f32) -> Result<Node> {
        Ok(Node::F32(v))
    }
    fn serialize_f64(self, v: f64) -> Result<Node> {
        Ok(match self.0.fixed() {
            Fixed::Grid(grid) => Node::Fixed(grid, grid.round(v)),
            Fixed::Exact | Fixed::Rotation => Node::F64(v),
        })
    }
    fn serialize_char(self, v: char) -> Result<Node> {
        Ok(Node::Uint(v.into()))
    }
    fn serialize_str(self, v: &str) -> Result<Node> {
        Ok(Node::Text(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Node> {
        Ok(Node::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Node> {
        Ok(Node::None)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Node> {
        Ok(Node::Some(Box::new(value.serialize(self)?)))
    }
    fn serialize_unit(self) -> Result<Node> {
        Ok(Node::Unit)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Node> {
        Ok(Node::Unit)
    }
    fn serialize_unit_variant(self, _: &'static str, index: u32, _: &'static str) -> Result<Node> {
        Ok(Node::Variant(index, Box::new(Node::Unit)))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Node> {
        let node = value.serialize(self)?;
        Ok(if name == ALIGNED {
            Node::Aligned(Box::new(node))
        } else {
            node
        })
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<Node> {
        Ok(Node::Variant(index, Box::new(value.serialize(self)?)))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Items<R>> {
        Ok(self.items(len.unwrap_or(0), Kind::Seq))
    }
    fn serialize_tuple(self, len: usize) -> Result<Items<R>> {
        let kind = if len == 4 && self.0.fixed() == Fixed::Rotation {
            Kind::Rotation
        } else {
            Kind::Tuple
        };
        Ok(self.items(len, kind))
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Items<R>> {
        Ok(self.items(len, Kind::Tuple))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Items<R>> {
        Ok(self.items(len, Kind::Variant(index)))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Entries<R>> {
        Ok(Entries {
            rule: self.0,
            entries: Vec::with_capacity(len.unwrap_or(0)),
            key: None,
        })
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Items<R>> {
        Ok(self.items(len, Kind::Tuple))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Items<R>> {
        Ok(self.items(len, Kind::Variant(index)))
    }
}
impl<R: Rounding> NodeSerializer<R> {
    fn items(self, len: usize, kind: Kind) -> Items<R> {
        Items {
            rule: self.0,
            items: Vec::with_capacity(len),
            kind,
        }
    }
}
impl<R: Rounding> ser::SerializeSeq for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(NodeSerializer(self.rule))?);
        Ok(())
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeTuple for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeTupleStruct for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeTupleVariant for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeStruct for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.items
            .push(value.serialize(NodeSerializer(self.rule.field(key)))?);
        Ok(())
    }
    fn skip_field(&mut self, key: &'static str) -> Result<()> {
        Err(Error::new(format_args!(
            "field `{key}` is skipped, which a positional codec cannot decode"
        )))
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeStructVariant for Items<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        ser::SerializeStruct::serialize_field(self, key, value)
    }
    fn skip_field(&mut self, key: &'static str) -> Result<()> {
        ser::SerializeStruct::skip_field(self, key)
    }
    fn end(self) -> Result<Node> {
        self.finish()
    }
}
impl<R: Rounding> ser::SerializeMap for Entries<R> {
    type Ok = Node;
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.key = Some(key.serialize(NodeSerializer(Exact))?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error::new("map value without a key"))?;
        self.entries
            .push((key, value.serialize(NodeSerializer(self.rule))?));
        Ok(())
    }
    fn end(self) -> Result<Node> {
        Ok(Node::Map(self.entries))
    }
}

// ---------------------------------------------------------------------------
// Deserialization from a tree.

struct NodeDe<'a>(&'a Node);

fn mismatch() -> Error {
    Error::new("value does not match the expected type")
}

impl<'de> de::Deserializer<'de> for NodeDe<'_> {
    type Error = Error;
    fn is_human_readable(&self) -> bool {
        false
    }
    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.0 {
            Node::Unit => visitor.visit_unit(),
            Node::Bool(v) => visitor.visit_bool(*v),
            Node::Uint(v) => visitor.visit_u64(*v),
            Node::Int(v) => visitor.visit_i64(*v),
            Node::F32(v) => visitor.visit_f32(*v),
            Node::F64(v) | Node::Fixed(_, v) => visitor.visit_f64(*v),
            Node::Rotation(_, x) => visitor.visit_seq(de::value::SeqDeserializer::<_, Error>::new(
                x.iter().copied(),
            )),
            Node::Text(v) => visitor.visit_str(v),
            Node::Bytes(v) => visitor.visit_bytes(v),
            Node::None => visitor.visit_none(),
            Node::Some(v) => visitor.visit_some(NodeDe(v)),
            Node::Tuple(items) | Node::Seq(items) => visit_node_seq(items, visitor),
            Node::Map(entries) => {
                let mut map = NodeMap {
                    entries: entries.iter(),
                    value: None,
                };
                let value = visitor.visit_map(&mut map)?;
                if map.entries.len() != 0 {
                    return Err(mismatch());
                }
                Ok(value)
            }
            Node::Variant(index, payload) => visitor.visit_enum(NodeEnum(*index, payload)),
            Node::Aligned(inner) => NodeDe(inner).deserialize_any(visitor),
        }
    }
    fn deserialize_char<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.0 {
            Node::Uint(v) => visitor.visit_char(
                u32::try_from(*v)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(mismatch)?,
            ),
            _ => Err(mismatch()),
        }
    }
    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.0 {
            Node::None => visitor.visit_none(),
            Node::Some(v) => visitor.visit_some(NodeDe(v)),
            _ => Err(mismatch()),
        }
    }
    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        match self.0 {
            Node::Aligned(inner) => visitor.visit_newtype_struct(NodeDe(inner)),
            _ => visitor.visit_newtype_struct(self),
        }
    }
    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.0 {
            Node::Variant(index, payload) => visitor.visit_enum(NodeEnum(*index, payload)),
            _ => Err(mismatch()),
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 str string bytes
        byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
        ignored_any
    }
}
fn visit_node_seq<'de, V: de::Visitor<'de>>(items: &[Node], visitor: V) -> Result<V::Value> {
    let mut seq = NodeSeq(items.iter());
    let value = visitor.visit_seq(&mut seq)?;
    if seq.0.len() != 0 {
        return Err(mismatch());
    }
    Ok(value)
}
struct NodeSeq<'a>(std::slice::Iter<'a, Node>);
impl<'de> de::SeqAccess<'de> for NodeSeq<'_> {
    type Error = Error;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        self.0
            .next()
            .map(|n| seed.deserialize(NodeDe(n)))
            .transpose()
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}
struct NodeMap<'a> {
    entries: std::slice::Iter<'a, (Node, Node)>,
    value: Option<&'a Node>,
}
impl<'de> de::MapAccess<'de> for NodeMap<'_> {
    type Error = Error;
    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some(value);
        seed.deserialize(NodeDe(key)).map(Some)
    }
    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        seed.deserialize(NodeDe(self.value.take().ok_or_else(mismatch)?))
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}
struct NodeEnum<'a>(u32, &'a Node);
impl<'de, 'a> de::EnumAccess<'de> for NodeEnum<'a> {
    type Error = Error;
    type Variant = NodeDe<'a>;
    fn variant_seed<V: de::DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, NodeDe<'a>)> {
        let de: de::value::U32Deserializer<Error> = self.0.into_deserializer();
        Ok((seed.deserialize(de)?, NodeDe(self.1)))
    }
}
impl<'de> de::VariantAccess<'de> for NodeDe<'_> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        match self.0 {
            Node::Unit => Ok(()),
            _ => Err(mismatch()),
        }
    }
    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: de::Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value> {
        de::Deserializer::deserialize_any(self, visitor)
    }
    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        de::Deserializer::deserialize_any(self, visitor)
    }
}

// ---------------------------------------------------------------------------
// Bits.

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    bit: usize,
    nodes: usize,
}
impl Writer {
    fn bits(&mut self, mut value: u64, mut width: u32) {
        while width > 0 {
            let used = (self.bit % 8) as u32;
            if used == 0 {
                self.bytes.push(0);
            }
            let take = (8 - used).min(width);
            let mask = if take == 64 {
                u64::MAX
            } else {
                (1 << take) - 1
            };
            *self.bytes.last_mut().unwrap() |= ((value & mask) as u8) << used;
            value = value.checked_shr(take).unwrap_or(0);
            width -= take;
            self.bit += take as usize;
        }
    }
    fn var(&mut self, mut value: u64) {
        while value >= 128 {
            self.bits((value & 127) | 128, 8);
            value >>= 7;
        }
        self.bits(value, 8);
    }
    fn gamma(&mut self, index: u32) {
        let n = u64::from(index) + 1;
        let width = 63 - n.leading_zeros();
        // `width` ones, a zero, then the low `width` bits of `n`.
        self.bits((1 << width) - 1, width);
        self.bits(0, 1);
        self.bits(n, width);
    }
    /// Order-`k` exponential-Golomb: the gamma code of `(value >> k) + 1`,
    /// then the low `k` bits. Zero costs `k + 1` bits.
    fn golomb(&mut self, value: u64, k: u32) {
        let n = (value >> k) + 1;
        let width = 63 - n.leading_zeros();
        self.bits((1 << width) - 1, width);
        self.bits(0, 1);
        self.bits(n, width);
        self.bits(value, k);
    }
    /// An exact float: one bit for `f32`-exact, then 32 or 64 bits.
    fn exact(&mut self, n: f64) {
        if exact_f32(n) {
            self.bits(1, 1);
            self.bits(u64::from((n as f32).to_bits()), 32);
        } else {
            self.bits(0, 1);
            self.bits(n.to_bits(), 64);
        }
    }
    fn fixed(&mut self, grid: Grid, n: f64) {
        match grid.steps(n) {
            Some(q) => {
                self.bits(1, 1);
                self.bits(q as u64, grid.width);
            }
            None => {
                self.bits(0, 1);
                self.exact(n);
            }
        }
    }
    fn rotation(&mut self, code: Option<RotationCode>, x: &[f64; 4]) {
        match code {
            Some((index, steps)) => {
                self.bits(1, 1);
                self.bits(u64::from(index), 2);
                for q in steps {
                    self.bits(q as u64, ROTATION_BITS);
                }
            }
            None => {
                self.bits(0, 1);
                for v in x {
                    self.exact(*v);
                }
            }
        }
    }
    fn align(&mut self) {
        if !self.bit.is_multiple_of(8) {
            self.bits(0, 8 - (self.bit % 8) as u32);
        }
    }
    fn blob(&mut self, bytes: &[u8]) {
        // Text stays byte-aligned so compression can reuse it even when the
        // preceding fields occupy different bit widths.
        self.align();
        self.var(bytes.len() as u64);
        self.bytes.extend_from_slice(bytes);
        self.bit += bytes.len() * 8;
    }
    fn visit(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err(Error::new("traversal limit"));
        }
        Ok(())
    }
    fn finish(self) -> io::Result<Vec<u8>> {
        if self.bytes.len() > LIMIT {
            return Err(Error::new("frame exceeds 4 MiB").into());
        }
        Ok(self.bytes)
    }
    fn float(&mut self, n: f64) {
        if let Some((i, q, _)) = fixed_pair(n, n) {
            self.bits(0, 2);
            self.bits(i as u64, 3);
            self.bits(q as u64, GRIDS[i].1);
        } else if exact_f32(n) {
            self.bits(1, 2);
            self.bits(u64::from((n as f32).to_bits()), 32);
        } else {
            self.bits(2, 2);
            self.bits(n.to_bits(), 64);
        }
    }
    fn full(&mut self, node: &Node, depth: usize) -> Result<()> {
        if let Node::Aligned(inner) = node {
            self.align();
            return self.full(inner, depth);
        }
        self.visit(depth)?;
        match node {
            Node::Unit => {}
            Node::Bool(v) => self.bits(u64::from(*v), 1),
            Node::Uint(v) => self.var(*v),
            Node::Int(v) => self.var(zigzag(*v)),
            Node::F32(v) => self.bits(u64::from(v.to_bits()), 32),
            Node::F64(v) => self.float(*v),
            Node::Fixed(grid, v) => self.fixed(*grid, *v),
            Node::Rotation(code, x) => self.rotation(*code, x),
            Node::Text(v) => self.blob(v.as_bytes()),
            Node::Bytes(v) => self.blob(v),
            Node::None => self.bits(0, 1),
            Node::Some(v) => {
                self.bits(1, 1);
                self.full(v, depth + 1)?;
            }
            Node::Tuple(items) => {
                for item in items {
                    self.full(item, depth + 1)?;
                }
            }
            Node::Seq(items) => {
                self.var(items.len() as u64);
                for item in items {
                    self.full(item, depth + 1)?;
                }
            }
            Node::Map(entries) => {
                self.var(entries.len() as u64);
                for (key, value) in entries {
                    self.full(key, depth + 1)?;
                    self.full(value, depth + 1)?;
                }
            }
            Node::Variant(index, payload) => {
                self.gamma(*index);
                self.full(payload, depth + 1)?;
            }
            Node::Aligned(_) => unreachable!("aligned values are unwrapped above"),
        }
        Ok(())
    }
    fn delta(&mut self, before: &Node, after: &Node, depth: usize) -> Result<()> {
        match (before, after) {
            (Node::Aligned(a), Node::Aligned(b)) => return self.delta(a, b, depth),
            (Node::Aligned(_), _) | (_, Node::Aligned(_)) => {
                return Err(Error::new("baseline has a different type"));
            }
            _ => {}
        }
        self.visit(depth)?;
        let unchanged = before == after;
        self.bits(u64::from(!unchanged), 1);
        if unchanged {
            return Ok(());
        }
        match (before, after) {
            (Node::Fixed(grid, a), Node::Fixed(_, b)) => {
                // A changed on-grid pair differs by at least one step, so zero
                // marks the exact escape.
                match (grid.steps(*a), grid.steps(*b)) {
                    (Some(a), Some(b)) => self.golomb(zigzag(b - a), grid.k),
                    _ => {
                        self.golomb(0, grid.k);
                        self.exact(*b);
                    }
                }
            }
            (Node::Rotation(Some((i, a)), _), Node::Rotation(Some((j, b)), _)) if i == j => {
                self.bits(1, 1);
                for (a, b) in a.iter().zip(b) {
                    self.golomb(zigzag(b - a), ROTATION_K);
                }
            }
            (Node::Rotation(..), Node::Rotation(code, x)) => {
                self.bits(0, 1);
                self.rotation(*code, x);
            }
            (Node::F64(a), Node::F64(b)) => {
                if let Some((i, a, b)) = fixed_pair(*a, *b) {
                    let difference = zigzag(b - a);
                    let width = 64 - difference.leading_zeros();
                    self.bits(1, 1);
                    self.bits(i as u64, 3);
                    self.bits(u64::from(width - 1), 6);
                    self.bits(difference, width);
                } else {
                    let xor = a.to_bits() ^ b.to_bits();
                    let leading = xor.leading_zeros();
                    let trailing = xor.trailing_zeros();
                    let width = 64 - leading - trailing;
                    self.bits(0, 1);
                    self.bits(u64::from(leading), 6);
                    self.bits(u64::from(width - 1), 6);
                    self.bits(xor >> trailing, width);
                }
            }
            (Node::Tuple(a), Node::Tuple(b)) if a.len() == b.len() => {
                for (a, b) in a.iter().zip(b) {
                    self.delta(a, b, depth + 1)?;
                }
            }
            (Node::Seq(a), Node::Seq(b)) => {
                let drop = seq_alignment(a, b);
                if drop == 0 && a.len() == b.len() {
                    self.bits(1, 1);
                    for (a, b) in a.iter().zip(b) {
                        self.delta(a, b, depth + 1)?;
                    }
                } else {
                    // Realigned: the baseline's first `drop` elements are gone,
                    // the rest pair with the new ones in order, and any new
                    // element past the baseline travels in full.
                    self.bits(0, 1);
                    self.var(drop as u64);
                    self.var(b.len() as u64);
                    let mut bases = a[drop..].iter();
                    for item in b {
                        match bases.next() {
                            Some(base) => self.delta(base, item, depth + 1)?,
                            None => self.full(item, depth + 1)?,
                        }
                    }
                }
            }
            (Node::Map(a), Node::Map(b)) => {
                self.bits(u64::from(a.len() == b.len()), 1);
                if a.len() == b.len() {
                    for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                        self.delta(ak, bk, depth + 1)?;
                        self.delta(av, bv, depth + 1)?;
                    }
                } else {
                    self.full(after, depth + 1)?;
                }
            }
            (Node::Some(a), Node::Some(b)) => {
                self.bits(1, 1);
                self.delta(a, b, depth + 1)?;
            }
            (Node::None | Node::Some(_), Node::None | Node::Some(_)) => {
                self.bits(0, 1);
                self.full(after, depth + 1)?;
            }
            (Node::Variant(i, a), Node::Variant(j, b)) => {
                self.bits(u64::from(i == j), 1);
                if i == j {
                    self.delta(a, b, depth + 1)?;
                } else {
                    self.full(after, depth + 1)?;
                }
            }
            (Node::Bool(_), Node::Bool(_))
            | (Node::Uint(_), Node::Uint(_))
            | (Node::Int(_), Node::Int(_))
            | (Node::F32(_), Node::F32(_))
            | (Node::Text(_), Node::Text(_))
            | (Node::Bytes(_), Node::Bytes(_)) => self.full(after, depth + 1)?,
            _ => return Err(Error::new("baseline has a different type")),
        }
        Ok(())
    }
}

/// How many leading baseline elements a changed sequence drops before its
/// elements pair with the baseline's in order. Records are matched on their
/// first field when it is an integer or text (an identity such as a projectile
/// id): the first baseline element whose key equals the new first element's
/// key starts the pairing. With no such match an equal-length sequence pairs
/// in place and any other is sent in full (every baseline element dropped).
fn seq_alignment(a: &[Node], b: &[Node]) -> usize {
    fn key(node: &Node) -> Option<&Node> {
        let node = match node {
            Node::Aligned(inner) => inner,
            other => other,
        };
        match node {
            Node::Tuple(items) => match items.first()? {
                key @ (Node::Uint(_) | Node::Int(_) | Node::Text(_)) => Some(key),
                _ => None,
            },
            _ => None,
        }
    }
    let found = b
        .first()
        .and_then(key)
        .and_then(|first| a.iter().position(|base| key(base) == Some(first)));
    match found {
        Some(drop) => drop,
        None if a.len() == b.len() => 0,
        None => a.len(),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    bit: usize,
    nodes: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], bit: usize) -> Self {
        Self {
            bytes,
            bit,
            nodes: 0,
        }
    }
    fn remaining(&self) -> usize {
        (self.bytes.len() * 8).saturating_sub(self.bit)
    }
    fn bits(&mut self, mut width: u32) -> Result<u64> {
        if width as usize > self.remaining() {
            return Err(invalid());
        }
        let mut value = 0_u64;
        let mut shift = 0;
        while width > 0 {
            let used = (self.bit % 8) as u32;
            let take = (8 - used).min(width);
            let byte = u64::from(self.bytes[self.bit / 8] >> used) & ((1 << take) - 1);
            value |= byte << shift;
            shift += take;
            width -= take;
            self.bit += take as usize;
        }
        Ok(value)
    }
    fn bit1(&mut self) -> Result<bool> {
        Ok(self.bits(1)? != 0)
    }
    fn var(&mut self) -> Result<u64> {
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
    fn length(&mut self) -> Result<usize> {
        let len = usize::try_from(self.var()?).map_err(|_| invalid())?;
        if len > MAX_NODES || len > self.remaining() {
            return Err(invalid());
        }
        Ok(len)
    }
    fn gamma(&mut self) -> Result<u32> {
        let mut width = 0;
        while self.bit1()? {
            width += 1;
            if width > 32 {
                return Err(invalid());
            }
        }
        let n = (1_u64 << width) | self.bits(width)?;
        u32::try_from(n - 1).map_err(|_| invalid())
    }
    fn golomb(&mut self, k: u32) -> Result<u64> {
        let mut width = 0;
        while self.bit1()? {
            width += 1;
            if width + k > 63 {
                return Err(invalid());
            }
        }
        let n = (1_u64 << width) | self.bits(width)?;
        Ok(((n - 1) << k) | self.bits(k)?)
    }
    fn exact(&mut self) -> Result<f64> {
        Ok(if self.bit1()? {
            f64::from(f32::from_bits(self.bits(32)? as u32))
        } else {
            f64::from_bits(self.bits(64)?)
        })
    }
    /// An escaped value must be one rounding leaves alone, so the decoder's
    /// rebuilt baseline equals the encoder's. A delta escapes an on-grid value
    /// whose baseline was off the grid.
    fn escaped(&mut self, grid: Grid) -> Result<f64> {
        let n = self.exact()?;
        if grid.round(n).to_bits() != n.to_bits() {
            return Err(invalid());
        }
        Ok(n)
    }
    fn on_grid(grid: Grid, q: i64) -> Result<f64> {
        let value = q as f64 / grid.scale + 0.0;
        if grid.steps(value) != Some(q) {
            return Err(invalid());
        }
        Ok(value)
    }
    fn fixed(&mut self, grid: Grid) -> Result<f64> {
        if !self.bit1()? {
            return self.escaped(grid);
        }
        let width = grid.width;
        let raw = self.bits(width)?;
        Self::on_grid(grid, ((raw << (64 - width)) as i64) >> (64 - width))
    }
    fn fixed_delta(&mut self, grid: Grid, before: f64) -> Result<f64> {
        let difference = self.golomb(grid.k)?;
        if difference == 0 {
            return self.escaped(grid);
        }
        let b = grid
            .steps(before)
            .ok_or_else(invalid)?
            .checked_add(unzigzag(difference))
            .ok_or_else(invalid)?;
        Self::on_grid(grid, b)
    }
    /// A smallest-three code must be the one its own value quantizes to, and an
    /// exact quaternion one with no such code; see [`rotation_code`].
    fn checked_rotation(code: Option<RotationCode>, x: [f64; 4]) -> Result<[f64; 4]> {
        let x = match code {
            Some(code) => rotation_value(code),
            None => x,
        };
        if rotation_code(x) != code {
            return Err(invalid());
        }
        Ok(x)
    }
    fn rotation(&mut self) -> Result<[f64; 4]> {
        if !self.bit1()? {
            let mut x = [0.; 4];
            for v in &mut x {
                *v = self.exact()?;
            }
            return Self::checked_rotation(None, x);
        }
        let index = self.bits(2)? as u8;
        let mut steps = [0; 3];
        for q in &mut steps {
            let raw = self.bits(ROTATION_BITS)?;
            *q = ((raw << (64 - ROTATION_BITS)) as i64) >> (64 - ROTATION_BITS);
        }
        Self::checked_rotation(Some((index, steps)), [0.; 4])
    }
    fn rotation_delta(&mut self, before: Option<RotationCode>) -> Result<[f64; 4]> {
        if !self.bit1()? {
            return self.rotation();
        }
        let (index, mut steps) = before.ok_or_else(invalid)?;
        for q in &mut steps {
            *q = q
                .checked_add(unzigzag(self.golomb(ROTATION_K)?))
                .ok_or_else(invalid)?;
        }
        Self::checked_rotation(Some((index, steps)), [0.; 4])
    }
    /// Skips to the next byte boundary, refusing nonzero padding.
    fn align(&mut self) -> Result<()> {
        if !self.bit.is_multiple_of(8) && self.bits(8 - (self.bit % 8) as u32)? != 0 {
            return Err(invalid());
        }
        Ok(())
    }
    fn blob(&mut self) -> Result<Vec<u8>> {
        self.align()?;
        let len = self.length()?;
        if len > self.remaining() / 8 {
            return Err(invalid());
        }
        let start = self.bit / 8;
        self.bit += len * 8;
        Ok(self.bytes[start..start + len].to_vec())
    }
    fn float(&mut self) -> Result<f64> {
        Ok(match self.bits(2)? {
            0 => {
                let i = self.bits(3)? as usize;
                let (scale, width) = *GRIDS.get(i).ok_or_else(invalid)?;
                let raw = self.bits(width)?;
                let q = ((raw << (64 - width)) as i64) >> (64 - width);
                let value = q as f64 / scale;
                // The encoder only uses a grid it round-trips through.
                if grid(value, i) != Some(q) {
                    return Err(invalid());
                }
                value
            }
            1 => f64::from(f32::from_bits(self.bits(32)? as u32)),
            2 => f64::from_bits(self.bits(64)?),
            _ => return Err(invalid()),
        })
    }
    fn float_delta(&mut self, before: f64) -> Result<f64> {
        if self.bit1()? {
            let i = self.bits(3)? as usize;
            let (scale, _) = *GRIDS.get(i).ok_or_else(invalid)?;
            let a = grid(before, i).ok_or_else(invalid)?;
            let width = self.bits(6)? as u32 + 1;
            let b = a
                .checked_add(unzigzag(self.bits(width)?))
                .ok_or_else(invalid)?;
            let value = b as f64 / scale;
            if grid(value, i) != Some(b) {
                return Err(invalid());
            }
            return Ok(value);
        }
        let leading = self.bits(6)? as u32;
        let width = self.bits(6)? as u32 + 1;
        if leading + width > 64 {
            return Err(invalid());
        }
        let xor = self.bits(width)? << (64 - leading - width);
        Ok(f64::from_bits(before.to_bits() ^ xor))
    }
    fn visit(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err(Error::new("traversal limit"));
        }
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        if self.bit.div_ceil(8) != self.bytes.len() {
            return Err(invalid());
        }
        if !self.bit.is_multiple_of(8) && self.bytes[self.bytes.len() - 1] >> (self.bit % 8) != 0 {
            return Err(invalid());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Type-driven decoding from bits, optionally against a baseline tree.

struct BitDe<'r, 'x, 'b, R> {
    r: &'r mut Reader<'x>,
    base: Option<&'b Node>,
    depth: usize,
    /// The rounding rule on this value's path, mirroring the serializer's.
    rule: R,
}
/// How one value arrives: in full, unchanged from the baseline, or changed
/// relative to it.
enum Entry<'b> {
    Full,
    Same(&'b Node),
    Changed(&'b Node),
}
impl<'x, 'b, R: Rounding> BitDe<'_, 'x, 'b, R> {
    fn enter(&mut self) -> Result<Entry<'b>> {
        self.r.visit(self.depth)?;
        Ok(match self.base {
            None => Entry::Full,
            Some(base) if self.r.bit1()? => Entry::Changed(base),
            Some(base) => Entry::Same(base),
        })
    }
    /// A changed container's "same shape" bit: `Some(base)` to recurse against
    /// the baseline, `None` to read the value in full.
    fn shape(&mut self, entry: Entry<'b>) -> Result<Option<&'b Node>> {
        match entry {
            Entry::Changed(base) if self.r.bit1()? => Ok(Some(base)),
            _ => Ok(None),
        }
    }
    fn child<'s>(&'s mut self, base: Option<&'b Node>) -> BitDe<'s, 'x, 'b, R> {
        BitDe {
            r: self.r,
            base,
            depth: self.depth + 1,
            rule: self.rule,
        }
    }
    /// A struct (with its field names, for the rule) or a tuple.
    fn positional<'de, V: de::Visitor<'de>>(
        mut self,
        len: usize,
        fields: Option<&'static [&'static str]>,
        visitor: V,
    ) -> Result<V::Value> {
        match self.enter()? {
            Entry::Same(base) => de::Deserializer::deserialize_any(NodeDe(base), visitor),
            Entry::Changed(Node::Tuple(items)) if items.len() == len => {
                self.items(Some(items), len, fields, visitor)
            }
            Entry::Changed(_) => Err(mismatch()),
            Entry::Full => self.items(None, len, fields, visitor),
        }
    }
    fn rotation<'de, V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        let x = match self.enter()? {
            Entry::Same(base) => return de::Deserializer::deserialize_any(NodeDe(base), visitor),
            Entry::Full => self.r.rotation()?,
            Entry::Changed(Node::Rotation(code, _)) => self.r.rotation_delta(*code)?,
            Entry::Changed(_) => return Err(mismatch()),
        };
        visitor.visit_seq(de::value::SeqDeserializer::<_, Error>::new(x.into_iter()))
    }
    fn scalar<'de, V: de::Visitor<'de>>(
        mut self,
        visitor: V,
        read: impl FnOnce(&mut Reader<'x>, V) -> Result<V::Value>,
    ) -> Result<V::Value> {
        match self.enter()? {
            Entry::Same(base) => de::Deserializer::deserialize_any(NodeDe(base), visitor),
            Entry::Full | Entry::Changed(_) => read(self.r, visitor),
        }
    }
    fn items<'de, V: de::Visitor<'de>>(
        &mut self,
        bases: Option<&'b [Node]>,
        len: usize,
        fields: Option<&'static [&'static str]>,
        visitor: V,
    ) -> Result<V::Value> {
        let mut seq = BitSeq {
            de: self.child(None),
            bases: bases.map(|b| b.iter()),
            remaining: len,
            fields: fields.map(|f| f.iter()),
            tail_full: false,
        };
        let value = visitor.visit_seq(&mut seq)?;
        if seq.remaining != 0 {
            return Err(mismatch());
        }
        Ok(value)
    }
}

impl<'de, R: Rounding> de::Deserializer<'de> for BitDe<'_, '_, '_, R> {
    type Error = Error;
    fn is_human_readable(&self) -> bool {
        false
    }
    fn deserialize_any<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error::new(
            "the positional codec cannot decode a self-describing type",
        ))
    }
    fn deserialize_bool<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| v.visit_bool(r.bit1()?))
    }
    fn deserialize_i8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| v.visit_i64(unzigzag(r.var()?)))
    }
    fn deserialize_u8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| v.visit_u64(r.var()?))
    }
    fn deserialize_f32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| {
            v.visit_f32(f32::from_bits(r.bits(32)? as u32))
        })
    }
    fn deserialize_f64<V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        if let Fixed::Grid(grid) = self.rule.fixed() {
            return match self.enter()? {
                Entry::Full => visitor.visit_f64(self.r.fixed(grid)?),
                Entry::Same(base) => de::Deserializer::deserialize_any(NodeDe(base), visitor),
                Entry::Changed(Node::Fixed(_, before)) => {
                    visitor.visit_f64(self.r.fixed_delta(grid, *before)?)
                }
                Entry::Changed(_) => Err(mismatch()),
            };
        }
        match self.enter()? {
            Entry::Full => visitor.visit_f64(self.r.float()?),
            Entry::Same(base) => de::Deserializer::deserialize_any(NodeDe(base), visitor),
            Entry::Changed(Node::F64(before)) => visitor.visit_f64(self.r.float_delta(*before)?),
            Entry::Changed(_) => Err(mismatch()),
        }
    }
    fn deserialize_char<V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        match self.enter()? {
            Entry::Same(base) => NodeDe(base).deserialize_char(visitor),
            Entry::Full | Entry::Changed(_) => visitor.visit_char(
                u32::try_from(self.r.var()?)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(invalid)?,
            ),
        }
    }
    fn deserialize_str<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_string(visitor)
    }
    fn deserialize_string<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| {
            v.visit_string(String::from_utf8(r.blob()?).map_err(|_| invalid())?)
        })
    }
    fn deserialize_bytes<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_byte_buf(visitor)
    }
    fn deserialize_byte_buf<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |r, v| v.visit_byte_buf(r.blob()?))
    }
    fn deserialize_option<V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        let entry = self.enter()?;
        if let Entry::Same(base) = entry {
            return NodeDe(base).deserialize_option(visitor);
        }
        match self.shape(entry)? {
            Some(Node::Some(inner)) => visitor.visit_some(self.child(Some(inner))),
            Some(_) => Err(invalid()),
            None if self.r.bit1()? => visitor.visit_some(self.child(None)),
            None => visitor.visit_none(),
        }
    }
    fn deserialize_unit<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.scalar(visitor, |_, v| v.visit_unit())
    }
    fn deserialize_unit_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }
    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        if name != ALIGNED {
            return visitor.visit_newtype_struct(self);
        }
        // Mirrors the writer: padding only where the value is coded in full,
        // and a delta recurses against the baseline's unwrapped value.
        let base = match self.base {
            None => {
                self.r.align()?;
                None
            }
            Some(Node::Aligned(inner)) => Some(&**inner),
            Some(_) => return Err(mismatch()),
        };
        visitor.visit_newtype_struct(BitDe {
            r: self.r,
            base,
            depth: self.depth,
            rule: self.rule,
        })
    }
    fn deserialize_seq<V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        let entry = self.enter()?;
        if let Entry::Same(base) = entry {
            return de::Deserializer::deserialize_any(NodeDe(base), visitor);
        }
        match entry {
            Entry::Changed(Node::Seq(items)) => {
                if self.r.bit1()? {
                    return self.items(Some(items), items.len(), None, visitor);
                }
                let drop = usize::try_from(self.r.var()?).map_err(|_| invalid())?;
                let bases = items.get(drop..).ok_or_else(invalid)?;
                let len = self.r.length()?;
                let mut seq = BitSeq {
                    de: self.child(None),
                    bases: Some(bases.iter()),
                    remaining: len,
                    fields: None,
                    tail_full: true,
                };
                let value = visitor.visit_seq(&mut seq)?;
                if seq.remaining != 0 {
                    return Err(mismatch());
                }
                Ok(value)
            }
            Entry::Changed(_) => Err(invalid()),
            _ => {
                let len = self.r.length()?;
                self.items(None, len, None, visitor)
            }
        }
    }
    fn deserialize_tuple<V: de::Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        if len == 4 && self.rule.fixed() == Fixed::Rotation {
            return self.rotation(visitor);
        }
        self.positional(len, None, visitor)
    }
    fn deserialize_tuple_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.positional(len, None, visitor)
    }
    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.positional(fields.len(), Some(fields), visitor)
    }
    fn deserialize_map<V: de::Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        let entry = self.enter()?;
        if let Entry::Same(base) = entry {
            return de::Deserializer::deserialize_any(NodeDe(base), visitor);
        }
        let (bases, len) = match self.shape(entry)? {
            Some(Node::Map(entries)) => (Some(entries.iter()), entries.len()),
            Some(_) => return Err(invalid()),
            None => (None, self.r.length()?),
        };
        let mut map = BitMap {
            de: self.child(None),
            bases,
            value: None,
            remaining: len,
        };
        let value = visitor.visit_map(&mut map)?;
        if map.remaining != 0 {
            return Err(mismatch());
        }
        Ok(value)
    }
    fn deserialize_enum<V: de::Visitor<'de>>(
        mut self,
        _: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let entry = self.enter()?;
        if let Entry::Same(base) = entry {
            return NodeDe(base).deserialize_enum("", variants, visitor);
        }
        let (index, payload) = match self.shape(entry)? {
            Some(Node::Variant(index, payload)) => (*index, Some(&**payload)),
            Some(_) => return Err(invalid()),
            None => (self.r.gamma()?, None),
        };
        if index as usize >= variants.len() {
            return Err(invalid());
        }
        visitor.visit_enum(BitEnum {
            de: self.child(payload),
            index,
        })
    }
    fn deserialize_identifier<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error::new("the positional codec carries no identifiers"))
    }
    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error::new("the positional codec cannot skip a value"))
    }
    fn deserialize_i128<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error::new("128-bit integers are not supported"))
    }
    fn deserialize_u128<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error::new("128-bit integers are not supported"))
    }
}

struct BitSeq<'s, 'x, 'b, R> {
    de: BitDe<'s, 'x, 'b, R>,
    bases: Option<std::slice::Iter<'b, Node>>,
    remaining: usize,
    /// A struct's field names in declaration order, which select each field's
    /// rule as the serializer's names did.
    fields: Option<std::slice::Iter<'static, &'static str>>,
    /// A realigned sequence: elements past the baseline's end arrive in full
    /// instead of being a shape mismatch.
    tail_full: bool,
}
impl<'de, R: Rounding> de::SeqAccess<'de> for BitSeq<'_, '_, '_, R> {
    type Error = Error;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let base = match &mut self.bases {
            Some(bases) => match bases.next() {
                Some(base) => Some(base),
                None if self.tail_full => None,
                None => return Err(mismatch()),
            },
            None => None,
        };
        let rule = match &mut self.fields {
            Some(fields) => self.de.rule.field(fields.next().ok_or_else(mismatch)?),
            None => self.de.rule,
        };
        seed.deserialize(BitDe {
            r: self.de.r,
            base,
            depth: self.de.depth,
            rule,
        })
        .map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining.min(4096))
    }
}
struct BitMap<'s, 'x, 'b, R> {
    de: BitDe<'s, 'x, 'b, R>,
    bases: Option<std::slice::Iter<'b, (Node, Node)>>,
    value: Option<&'b Node>,
    remaining: usize,
}
impl<'de, R: Rounding> de::MapAccess<'de> for BitMap<'_, '_, '_, R> {
    type Error = Error;
    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let key = match &mut self.bases {
            Some(bases) => {
                let (key, value) = bases.next().ok_or_else(mismatch)?;
                self.value = Some(value);
                Some(key)
            }
            None => None,
        };
        seed.deserialize(BitDe {
            r: self.de.r,
            base: key,
            depth: self.de.depth,
            rule: Exact,
        })
        .map(Some)
    }
    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        seed.deserialize(BitDe {
            r: self.de.r,
            base: self.value.take(),
            depth: self.de.depth,
            rule: self.de.rule,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining.min(4096))
    }
}
struct BitEnum<'s, 'x, 'b, R> {
    de: BitDe<'s, 'x, 'b, R>,
    index: u32,
}
impl<'de, 's, 'x, 'b, R: Rounding> de::EnumAccess<'de> for BitEnum<'s, 'x, 'b, R> {
    type Error = Error;
    type Variant = BitDe<'s, 'x, 'b, R>;
    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let index: de::value::U32Deserializer<Error> = self.index.into_deserializer();
        Ok((seed.deserialize(index)?, self.de))
    }
}
impl<'de, R: Rounding> de::VariantAccess<'de> for BitDe<'_, '_, '_, R> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        de::Deserializer::deserialize_unit(self, de::IgnoredAny).map(|_| ())
    }
    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: de::Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        self.positional(len, None, visitor)
    }
    fn struct_variant<V: de::Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.positional(fields.len(), Some(fields), visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    enum Shape {
        Empty,
        Point(f64),
        Pair(i32, Option<u8>),
        Named { label: String, flags: Vec<bool> },
    }
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Unit;
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Wrapper(u64);
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct State {
        tick: u64,
        pos: [f64; 3],
        shapes: Vec<Shape>,
        opt: Option<Box<State>>,
        map: BTreeMap<String, i64>,
        unit: Unit,
        wrapper: Wrapper,
        ch: char,
        small: f32,
        bytes: Vec<u8>,
    }
    fn state(seed: u64) -> State {
        State {
            tick: seed,
            pos: [1.25 * seed as f64, -0.0, f64::MAX],
            shapes: (0..seed % 4)
                .map(|i| match i {
                    0 => Shape::Empty,
                    1 => Shape::Point(0.1 + seed as f64),
                    2 => Shape::Pair(-(seed as i32), Some(seed as u8)),
                    _ => Shape::Named {
                        label: "红".repeat(seed as usize % 3),
                        flags: vec![true, seed.is_multiple_of(2)],
                    },
                })
                .collect(),
            opt: (seed % 3 == 1).then(|| Box::new(state(seed / 3))),
            map: (0..seed % 3)
                .map(|i| (format!("k{i}"), i as i64 - 1))
                .collect(),
            unit: Unit,
            wrapper: Wrapper(u64::MAX - seed),
            ch: 'é',
            small: seed as f32 / 3.,
            bytes: vec![seed as u8; (seed % 5) as usize],
        }
    }

    #[test]
    fn full_and_delta_frames_round_trip_every_shape() {
        let states: Vec<State> = (0..12).map(state).collect();
        for value in &states {
            let node = to_node(value).unwrap();
            assert_eq!(&from_node::<State>(&node).unwrap(), value);
            let bytes = encode(&node, None, 7, 1).unwrap();
            let decoded = decode::<State>(&bytes, None, 7).unwrap().0;
            assert_eq!(to_node(&decoded).unwrap(), node);
            for base in &states {
                let base = to_node(base).unwrap();
                let bytes = encode(&node, Some(&base), 7, 1).unwrap();
                let decoded = decode::<State>(&bytes, Some((&base, 1)), 7).unwrap().0;
                assert_eq!(to_node(&decoded).unwrap(), node);
                assert!(decode::<State>(&bytes, None, 7).is_err());
                assert!(decode::<State>(&bytes, Some((&base, 2)), 7).is_err());
                assert!(decode::<State>(&bytes, Some((&base, 1)), 8).is_err());
            }
        }
    }

    #[test]
    fn floats_pick_the_narrowest_exact_form() {
        let bits = |n: f64| to_bytes(&n).unwrap().len();
        assert_eq!(bits(1.235), 3); // 2 + 3 + 18 bits on the millimetre grid
        assert_eq!(bits(0.1_f32 as f64), 5); // exact f32
        assert_eq!(bits(0.123456789), 9); // raw f64
        assert_eq!(
            from_bytes::<f64>(&to_bytes(&-0.0_f64).unwrap())
                .unwrap()
                .to_bits(),
            (-0.0_f64).to_bits()
        );
    }

    /// A rule shaped like the checkpoint's: `position` on the millimetre grid,
    /// `rotation` smallest-three, everything else exact.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Rule {
        Root,
        Millimetres,
        Rotation,
        Exact,
    }
    impl Rounding for Rule {
        fn field(self, key: &'static str) -> Self {
            match (self, key) {
                (Rule::Root, "position") => Rule::Millimetres,
                (Rule::Root, "rotation") => Rule::Rotation,
                (Rule::Root, "nested") => Rule::Root,
                (Rule::Millimetres, _) => self,
                _ => Rule::Exact,
            }
        }
        fn fixed(self) -> Fixed {
            match self {
                Rule::Millimetres => Fixed::Grid(Grid {
                    scale: 1000.,
                    width: 18,
                    k: 8,
                }),
                Rule::Rotation => Fixed::Rotation,
                _ => Fixed::Exact,
            }
        }
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct Rounded {
        position: [f64; 3],
        rotation: [f64; 4],
        exact: f64,
        nested: Option<Box<Rounded>>,
    }

    /// Full frames and deltas between every pair decode to values whose rebuilt
    /// tree is bit-identical to the encoder's, and hostile truncations fail.
    fn assert_rule_baselines_are_exact(values: &[Rounded]) {
        let nodes: Vec<Node> = values
            .iter()
            .map(|v| to_node_with(v, Rule::Root).unwrap())
            .collect();
        for node in &nodes {
            let bytes = encode(node, None, 3, 0).unwrap();
            let (value, _) = decode_with::<Rounded, _>(&bytes, None, 3, Rule::Root).unwrap();
            assert_eq!(&to_node_with(&value, Rule::Root).unwrap(), node);
            for base in &nodes {
                let bytes = encode(node, Some(base), 3, 5).unwrap();
                let (value, _) =
                    decode_with::<Rounded, _>(&bytes, Some((base, 5)), 3, Rule::Root).unwrap();
                assert_eq!(&to_node_with(&value, Rule::Root).unwrap(), node);
                for len in 0..bytes.len() {
                    assert!(
                        decode_with::<Rounded, _>(&bytes[..len], Some((base, 5)), 3, Rule::Root)
                            .is_err()
                    );
                }
            }
        }
    }

    #[test]
    fn grid_rules_send_no_grid_index_and_rebuild_exact_baselines() {
        let rounded = |position: [f64; 3], exact: f64| Rounded {
            position,
            rotation: [1., 0., 0., 0.],
            exact,
            nested: None,
        };
        let values = [
            rounded([1.23456, -2.0, 0.0], 0.123456789),
            rounded([1.2351, -2.0004, -0.0001], 0.123456789),
            // Out of range, negative zero and NaN keep their exact bits.
            rounded([200., -0.0, f64::NAN], 1e300),
            rounded([-131.072, 131.0709, f64::INFINITY], -0.0),
        ];
        let node = to_node_with(&values[0], Rule::Root).unwrap();
        let Node::Tuple(fields) = &node else {
            panic!("a struct is a tuple");
        };
        assert_eq!(
            fields[0],
            Node::Tuple(vec![
                Node::Fixed(
                    Grid {
                        scale: 1000.,
                        width: 18,
                        k: 8
                    },
                    1.235
                ),
                Node::Fixed(
                    Grid {
                        scale: 1000.,
                        width: 18,
                        k: 8
                    },
                    -2.0
                ),
                Node::Fixed(
                    Grid {
                        scale: 1000.,
                        width: 18,
                        k: 8
                    },
                    0.0
                ),
            ])
        );
        // A 49-bit header, three 1 + 18 bit positions, a 1 + 2 + 3 × 15 bit
        // identity, the 2 + 64 bit tagged exact value and an absent option.
        let bits: usize = 49 + 3 * 19 + 48 + 66 + 1;
        assert_eq!(to_bytes_with(&values[0]), bits.div_ceil(8));
        assert_rule_baselines_are_exact(&values);
    }

    fn to_bytes_with(value: &Rounded) -> usize {
        encode(&to_node_with(value, Rule::Root).unwrap(), None, 0, 0)
            .unwrap()
            .len()
    }

    #[test]
    fn smallest_three_rotations_rebuild_exact_baselines_near_ties_and_sign_flips() {
        let unit = |q: [f64; 4]| {
            let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
            q.map(|v| v / norm)
        };
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let mut rotations = vec![
            [1., 0., 0., 0.],
            [-1., 0., 0., 0.],
            [0., 0., 0., 1.],
            [0.5, -0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5, 0.5],
        ];
        // Two largest components within a few steps of each other, both
        // orderings and both signs, straddling the index switch.
        for step in -6..=6 {
            let e = f64::from(step) * 1e-5;
            for q in [
                [half + e, half - e, 0., 0.],
                [half - e, -(half + e), 1e-3, 0.],
                [0.3, -(0.6 + e), 0.6 - e, 0.43],
            ] {
                let q = unit(q);
                rotations.push(q);
                rotations.push(q.map(|v| -v));
            }
        }
        let values: Vec<Rounded> = rotations
            .iter()
            .map(|&rotation| Rounded {
                position: [0.; 3],
                rotation,
                exact: 0.,
                nested: None,
            })
            .collect();
        for value in &values {
            let Node::Tuple(fields) = to_node_with(value, Rule::Root).unwrap() else {
                panic!("a struct is a tuple");
            };
            let Node::Rotation(Some((index, _)), x) = &fields[1] else {
                panic!("{:?} has no smallest-three code", value.rotation);
            };
            // Sign-normalized onto q or -q, within the grid's error.
            let dot: f64 = x.iter().zip(value.rotation).map(|(a, b)| a * b).sum();
            assert!(dot.abs() > 1. - 1e-8, "{:?} -> {x:?}", value.rotation);
            assert!(x[usize::from(*index)] > 0.);
        }
        // Every unit quaternion near a two-way tie, at any tilt, has a code
        // that reproduces itself.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut uniform = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 11) as f64 / (1_u64 << 53) as f64 - 0.5
        };
        for _ in 0..20_000 {
            let q = unit([
                half + uniform() * 1e-4,
                half + uniform() * 1e-4,
                uniform() * 0.4,
                uniform() * 0.1,
            ]);
            let code = rotation_code(q).expect("a near-tie quaternion has a code");
            assert_eq!(rotation_code(rotation_value(code)), Some(code));
        }
        // q and -q share one code.
        assert_eq!(
            to_node_with(&values[0], Rule::Root).unwrap(),
            to_node_with(&values[1], Rule::Root).unwrap()
        );
        // A non-unit quaternion keeps its exact bits.
        let mut scaled = values[3].clone();
        scaled.rotation = [1., 1., 0., 0.];
        let Node::Tuple(fields) = to_node_with(&scaled, Rule::Root).unwrap() else {
            panic!("a struct is a tuple");
        };
        assert_eq!(fields[1], Node::Rotation(None, [1., 1., 0., 0.]));
        let mut all = values;
        all.push(scaled);
        assert_rule_baselines_are_exact(&all);
    }

    #[test]
    fn exponential_golomb_codes_round_trip() {
        for k in [0, 1, 7, 12] {
            for value in [0, 1, 2, 127, 128, 4095, 1 << 40, u64::MAX >> 13] {
                let mut w = Writer::default();
                w.golomb(value, k);
                let len = w.bit;
                let mut r = Reader::new(&w.bytes, 0);
                assert_eq!(r.golomb(k).unwrap(), value);
                assert_eq!(r.bit, len);
            }
        }
        let mut w = Writer::default();
        w.golomb(0, 8);
        assert_eq!(w.bit, 9);
    }

    #[test]
    fn aligned_values_pad_full_frames_only_and_refuse_nonzero_padding() {
        type Record = (bool, Aligned<Vec<Aligned<(bool, u8)>>>, bool);
        let value: Record = (
            true,
            Aligned(vec![Aligned((true, 9)), Aligned((false, 3))]),
            true,
        );
        let node = to_node(&value).unwrap();
        // A 49-bit header and the flag, padding to byte 7, the length byte, the
        // first element (already aligned), padding, the second, the last flag.
        let full = encode(&node, None, 1, 0).unwrap();
        let (decoded, _) = decode::<Record>(&full, None, 1).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(to_node(&decoded).unwrap(), node);
        assert_eq!(full.len(), 12);
        // The same value unwrapped packs densely.
        let dense = encode(
            &to_node(&(true, vec![(true, 9_u8), (false, 3_u8)], true)).unwrap(),
            None,
            1,
            0,
        )
        .unwrap();
        assert!(dense.len() < full.len());
        // A delta against an aligned baseline carries no padding and rebuilds
        // the aligned tree.
        let next: Record = (
            true,
            Aligned(vec![Aligned((true, 9)), Aligned((true, 4))]),
            false,
        );
        let delta = encode(&to_node(&next).unwrap(), Some(&node), 1, 2).unwrap();
        let (decoded, _) = decode::<Record>(&delta, Some((&node, 2)), 1).unwrap();
        assert_eq!(decoded, next);
        // A length change codes the sequence in full inside the delta, padded
        // on both ends alike.
        let longer: Record = (false, Aligned(vec![Aligned((true, 1)); 3]), true);
        let delta = encode(&to_node(&longer).unwrap(), Some(&node), 1, 2).unwrap();
        assert_eq!(
            decode::<Record>(&delta, Some((&node, 2)), 1).unwrap().0,
            longer
        );
        // Padding bits must be zero.
        let mut corrupt = full.clone();
        corrupt[6] |= 0x80;
        assert!(decode::<Record>(&corrupt, None, 1).is_err());
        // Other formats see the plain value.
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            "[true,[[true,9],[false,3]],true]"
        );
    }

    #[test]
    fn variant_indexes_are_gamma_coded() {
        for index in [0, 1, 2, 3, 6, 7, 1000, u32::MAX - 1] {
            let mut w = Writer::default();
            w.gamma(index);
            let len = w.bit;
            let mut r = Reader::new(&w.bytes, 0);
            assert_eq!(r.gamma().unwrap(), index);
            assert_eq!(r.bit, len);
        }
        let mut w = Writer::default();
        w.gamma(0);
        assert_eq!(w.bit, 1);
    }

    #[test]
    fn skipped_fields_and_self_describing_types_are_refused() {
        #[derive(Serialize)]
        struct Skips {
            #[serde(skip_serializing_if = "Option::is_none")]
            a: Option<u8>,
        }
        assert!(to_node(&Skips { a: None }).is_err());
        assert!(from_bytes::<serde_json::Value>(&[0]).is_err());
    }

    #[test]
    fn truncated_trailing_and_hostile_frames_are_rejected_without_panics() {
        let value = state(11);
        let node = to_node(&value).unwrap();
        let base = to_node(&state(7)).unwrap();
        for baseline in [None, Some(&base)] {
            let mut bytes = encode(&node, baseline, 0, 1).unwrap();
            let pinned = baseline.map(|b| (b, 1));
            for len in 0..bytes.len() {
                assert!(decode::<State>(&bytes[..len], pinned, 0).is_err());
            }
            bytes.push(0);
            assert!(decode::<State>(&bytes, pinned, 0).is_err());
            let mut seed = 1_u64;
            for len in 0..256 {
                let mut w = Writer::default();
                w.bytes.extend_from_slice(MAGIC);
                w.bit = 32;
                w.bits(u64::from(baseline.is_some()), 1);
                w.var(0);
                w.var(1);
                for _ in 0..len {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    w.bits((seed >> 32) & 255, 8);
                }
                let frame = w.bytes;
                let _ = decode::<State>(&frame, pinned, 0);
            }
        }
    }

    #[test]
    fn changed_sequences_realign_on_their_first_field() {
        type Ball = (u64, f64);
        let round = |base: &[Ball], next: &[Ball]| {
            let (base, next) = (to_node(&base).unwrap(), to_node(&next).unwrap());
            let bytes = encode(&next, Some(&base), 0, 1).unwrap();
            let (value, _) = decode::<Vec<Ball>>(&bytes, Some((&base, 1)), 0).unwrap();
            assert_eq!(to_node(&value).unwrap(), next);
            (bytes.len(), encode(&next, None, 0, 0).unwrap().len())
        };
        let base: Vec<Ball> = (1..=8).map(|id| (id, id as f64 * 1.5)).collect();
        // Two retired at the front, two spawned at the back: the six survivors
        // pair with their own baseline entries and only the spawns go in full.
        let mut shifted: Vec<Ball> = base[2..].to_vec();
        shifted.extend([(9, 13.5), (10, 15.)]);
        let (delta, full) = round(&base, &shifted);
        assert!(delta * 2 < full, "{delta} >= half of {full}");
        assert_eq!(seq_alignment(&[to_node(&base[0]).unwrap()], &[]), 1);
        // Shorter, longer, unkeyed and wholly new sequences stay lossless.
        round(&base, &base[3..5]);
        round(&base[..2], &base);
        round(&base, &[(40, 1.), (41, 2.)]);
        round(&base, &[]);
        round(&[], &base);
        let plain = |v: &[f64]| to_node(&v).unwrap();
        let bytes = encode(&plain(&[1., 2., 3.]), Some(&plain(&[1., 2.])), 0, 1).unwrap();
        let (value, _) = decode::<Vec<f64>>(&bytes, Some((&plain(&[1., 2.]), 1)), 0).unwrap();
        assert_eq!(value, [1., 2., 3.]);
        // A realignment that drops more than the baseline holds is refused.
        let mut w = Writer::default();
        w.bytes.extend_from_slice(MAGIC);
        w.bit = 32;
        w.bits(1, 1);
        w.var(0);
        w.var(1);
        w.var(0);
        w.bits(1, 1);
        w.bits(0, 1);
        w.var(9);
        w.var(0);
        let short = to_node(&[(1_u64, 1.0_f64)]).unwrap();
        assert!(decode::<Vec<Ball>>(&w.bytes, Some((&short, 1)), 0).is_err());
    }

    #[test]
    fn traversal_limits_hold_for_full_and_delta_frames() {
        let value = vec![0_u8; MAX_NODES];
        assert!(encode(&to_node(&value).unwrap(), None, 0, 0).is_err());
        let fits = to_node(&vec![0_u8; MAX_NODES - 1]).unwrap();
        let bytes = encode(&fits, None, 0, 0).unwrap();
        assert_eq!(
            decode::<Vec<u8>>(&bytes, None, 0).unwrap().0.len(),
            MAX_NODES - 1
        );
        // An unchanged subtree is one visit, however large.
        let bytes = encode(&fits, Some(&fits), 0, 1).unwrap();
        // Magic, flag and epoch, id, the zero hint and the unchanged bit.
        assert_eq!(bytes.len(), 8);
        assert_eq!(
            decode::<Vec<u8>>(&bytes, Some((&fits, 1)), 0)
                .unwrap()
                .0
                .len(),
            MAX_NODES - 1
        );
    }

    #[test]
    fn independent_baseline_survives_lost_reordered_and_duplicate_deltas() {
        let base = to_node(&(1_u64, [1.0_f64, 2.0, 3.0])).unwrap();
        let a = (2_u64, [1.1_f64, 2.2, 3.3]);
        let b = (3_u64, [1.2_f64, 2.4, 3.6]);
        let packet_a = encode(&to_node(&a).unwrap(), Some(&base), 0, 9).unwrap();
        let packet_b = encode(&to_node(&b).unwrap(), Some(&base), 0, 9).unwrap();
        for (packet, expected) in [(&packet_b, b), (&packet_b, b), (&packet_a, a)] {
            assert_eq!(
                decode::<(u64, [f64; 3])>(packet, Some((&base, 9)), 0)
                    .unwrap()
                    .0,
                expected
            );
        }
    }
}
