//! SHA-512 (FIPS 180-4), private to this crate.
//!
//! # Why this is not a crate of its own
//!
//! [`skynet_sha256`](../../skynet_sha256/index.html) is a crate because the specification *names*
//! sha256: artifacts are content-addressed by it (§17), the descriptor's digest is one (§5), the
//! black box seals with one (§13). Two editions disagreeing about it means two editions
//! disagreeing about identity, so it is a shared definition with published vectors behind it.
//!
//! SHA-512 is named nowhere. It appears in this system exactly once — inside Ed25519's signing
//! equation, where RFC 8032 fixes it as the hash and no choice was ever available. Nothing outside
//! this crate hashes anything with it, no wire format carries its output, and no operator will ever
//! see one. Promoting it to a crate would advertise a primitive nobody may use.
//!
//! It is held to the same standard regardless: the FIPS 180-4 example vectors and the RFC 8032 test
//! vectors both run in this crate's tests, and the second is the one that matters — a wrong SHA-512
//! makes every signature fail in a way that looks exactly like a wrong key.

/// The eight fractional parts of the square roots of the first eight primes, to 64 bits.
///
/// The high 32 bits of each are SHA-256's `H0` exactly — the two algorithms share their derivation
/// and differ only in how much of it they keep. That is a usable check on this table by eye.
const H0: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

/// The 80 round constants: fractional parts of the cube roots of the first eighty primes.
///
/// The first sixty-four have the same high 32 bits as SHA-256's `K`, for the same reason.
const K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

/// A streaming SHA-512.
///
/// Streaming for the reason `skynet_sha256::Sha256` is: Ed25519 hashes `R || A || M`, and `M` is a
/// message of unbounded length. Concatenating the three into one buffer to hash them would be a
/// size limit nobody declared, imposed on the one input an attacker chooses.
#[derive(Debug, Clone)]
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buffered: usize,
    /// Length in bits. FIPS 180-4 gives SHA-512 a 128-bit length field; a `u64` of bits caps a
    /// message at two exbibytes, which is not a limit any caller here can reach.
    length_bits: u64,
}

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha512 {
    /// A fresh hasher.
    pub const fn new() -> Self {
        Self {
            state: H0,
            buffer: [0u8; 128],
            buffered: 0,
            length_bits: 0,
        }
    }

    /// Absorb bytes.
    pub fn update(&mut self, mut input: &[u8]) {
        self.length_bits = self.length_bits.wrapping_add((input.len() as u64) << 3);

        if self.buffered > 0 {
            let want = 128 - self.buffered;
            let take = want.min(input.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&input[..take]);
            self.buffered += take;
            input = &input[take..];
            if self.buffered == 128 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }

        while input.len() >= 128 {
            let (block, rest) = input.split_at(128);
            let mut fixed = [0u8; 128];
            fixed.copy_from_slice(block);
            self.compress(&fixed);
            input = rest;
        }

        if !input.is_empty() {
            self.buffer[..input.len()].copy_from_slice(input);
            self.buffered = input.len();
        }
    }

    /// The digest.
    pub fn finish(mut self) -> [u8; 64] {
        let length_bits = self.length_bits;

        // 0x80, then zeroes, then the length as a 128-bit big-endian count of bits. The high 64
        // bits are always zero here, which is what `length_bits` being a `u64` amounts to.
        let mut tail = [0u8; 256];
        tail[0] = 0x80;
        let pad = {
            let after = self.buffered + 1;
            let rem = after % 128;
            if rem <= 112 {
                112 - rem
            } else {
                240 - rem
            }
        };
        let end = 1 + pad;
        tail[end..end + 8].copy_from_slice(&0u64.to_be_bytes());
        tail[end + 8..end + 16].copy_from_slice(&length_bits.to_be_bytes());

        // `update` would add these to the length, so absorb them by hand.
        let tail = &tail[..end + 16];
        let mut consumed = 0;
        while consumed < tail.len() {
            let want = 128 - self.buffered;
            let take = want.min(tail.len() - consumed);
            self.buffer[self.buffered..self.buffered + take]
                .copy_from_slice(&tail[consumed..consumed + take]);
            self.buffered += take;
            consumed += take;
            if self.buffered == 128 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
        debug_assert_eq!(self.buffered, 0, "padding lands on a block boundary");

        let mut out = [0u8; 64];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 128]) {
        let mut w = [0u64; 80];
        for (i, slot) in w.iter_mut().enumerate().take(16) {
            let mut word = [0u8; 8];
            word.copy_from_slice(&block[i * 8..i * 8 + 8]);
            *slot = u64::from_be_bytes(word);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        let deltas = [a, b, c, d, e, f, g, h];
        for (slot, delta) in self.state.iter_mut().zip(deltas) {
            *slot = slot.wrapping_add(delta);
        }
    }
}

/// One-shot, for the callers that already hold the whole input.
///
/// Verification does not use it — it streams `R || A || M` rather than joining them — so outside
/// tests and signing there is nothing to compile.
#[cfg(any(test, feature = "test-signing"))]
pub fn digest(input: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(input);
    h.finish()
}
