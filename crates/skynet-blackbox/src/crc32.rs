//! CRC-32, the IEEE 802.3 one, used by two things that both need it.
//!
//! MCAP requires it in `DataEnd` and `Footer` (§26's format is not ours to choose), and the ring
//! wants a per-record integrity check for a different reason: a record half-written when the power
//! failed must report itself as damaged rather than deserialise into a plausible cycle. One
//! implementation serves both, and it is 40 lines, which is a better answer than a dependency for a
//! workspace whose no-third-party property CI asserts on every commit.
//!
//! The reflected form, polynomial `0xEDB88320`, initial value `0xFFFFFFFF`, final xor
//! `0xFFFFFFFF` — the same CRC `gzip`, `png` and `mcap` mean when they say CRC-32.

/// The lookup table, built at compile time.
///
/// A `const fn` loop rather than a literal table: 256 hex constants are 256 opportunities for a
/// transcription error that a test with a few vectors would not necessarily catch, and the
/// generator polynomial is the thing a reader should be able to check.
const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// A running CRC-32, so a caller can hash bytes it never has in one place.
///
/// Sealing needs exactly that: MCAP's `data_section_crc32` covers every byte from the file magic to
/// the `DataEnd` record, and those bytes are produced record by record.
#[derive(Debug, Clone, Copy)]
pub struct Crc32 {
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// A fresh accumulator.
    pub const fn new() -> Self {
        Crc32 { state: 0xFFFF_FFFF }
    }

    /// Fold `bytes` in.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut state = self.state;
        for byte in bytes {
            let index = ((state ^ *byte as u32) & 0xFF) as usize;
            state = TABLE[index] ^ (state >> 8);
        }
        self.state = state;
    }

    /// The value so far. Reading it does not end the accumulation.
    pub fn value(self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

/// The CRC-32 of one slice.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.value()
}
