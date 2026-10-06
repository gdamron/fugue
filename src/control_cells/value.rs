//! Values that fit in one 32-bit control cell.

/// A `Copy` value stored in a control cell as its `u32` bits.
///
/// `from_bits` must be total: bits that no `to_bits` produces (an
/// out-of-range enum tag, say) map to a defined value rather than panicking,
/// because the audio thread decodes whatever a cell holds.
pub(crate) trait CellValue: Copy + Send + 'static {
    /// Encodes the value as cell bits.
    fn to_bits(self) -> u32;
    /// Decodes cell bits; total over every `u32`.
    fn from_bits(bits: u32) -> Self;
}

impl CellValue for f32 {
    fn to_bits(self) -> u32 {
        f32::to_bits(self)
    }
    fn from_bits(bits: u32) -> Self {
        f32::from_bits(bits)
    }
}

impl CellValue for u32 {
    fn to_bits(self) -> u32 {
        self
    }
    fn from_bits(bits: u32) -> Self {
        bits
    }
}

impl CellValue for i32 {
    fn to_bits(self) -> u32 {
        self as u32
    }
    fn from_bits(bits: u32) -> Self {
        bits as i32
    }
}

impl CellValue for bool {
    fn to_bits(self) -> u32 {
        u32::from(self)
    }
    fn from_bits(bits: u32) -> Self {
        bits != 0
    }
}
