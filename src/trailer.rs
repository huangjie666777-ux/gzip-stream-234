//! RFC 1952 member trailer: CRC32 and ISIZE, both little-endian.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailerError {
    CrcMismatch { expected: u32, actual: u32 },
    SizeMismatch { expected: u32, actual: u32 },
}

/// Collects the 8 trailer bytes incrementally, then verifies them against
/// the CRC32 and uncompressed size accumulated while decoding the member.
pub struct Trailer {
    buf: [u8; 8],
    len: usize,
}

impl Trailer {
    pub fn new() -> Self {
        Trailer { buf: [0; 8], len: 0 }
    }

    /// Copy as many trailer bytes as available. Returns (consumed, complete).
    pub fn feed(&mut self, input: &[u8]) -> (usize, bool) {
        let want = 8 - self.len;
        let take = want.min(input.len());
        self.buf[self.len..self.len + take].copy_from_slice(&input[..take]);
        self.len += take;
        (take, self.len == 8)
    }

    pub fn verify(&self, crc: u32, size: u64) -> Result<(), TrailerError> {
        let expected_crc = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let expected_size = u32::from_le_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        if expected_crc != crc {
            return Err(TrailerError::CrcMismatch {
                expected: expected_crc,
                actual: crc,
            });
        }
        let actual_size = (size & 0xFFFF_FFFF) as u32;
        if expected_size != actual_size {
            return Err(TrailerError::SizeMismatch {
                expected: expected_size,
                actual: actual_size,
            });
        }
        Ok(())
    }
}
