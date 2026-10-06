//! Thin wrapper around flate2's incremental raw-DEFLATE decoder.
//! One fresh decoder is used per gzip member.

use flate2::{Decompress, DecompressError, FlushDecompress, Status};

pub struct Inflater {
    inner: Decompress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub consumed: usize,
    pub produced: usize,
    pub stream_end: bool,
}

impl Inflater {
    /// Raw DEFLATE (no zlib wrapper), as required inside a gzip member.
    pub fn new() -> Self {
        Inflater {
            inner: Decompress::new(false),
        }
    }

    /// Push input and pull output once. Never blocks; reports exactly how
    /// many bytes were consumed and produced by this call.
    pub fn run(&mut self, input: &[u8], output: &mut [u8]) -> Result<Step, DecompressError> {
        let in_before = self.inner.total_in();
        let out_before = self.inner.total_out();
        let status = self.inner.decompress(input, output, FlushDecompress::None)?;
        Ok(Step {
            consumed: (self.inner.total_in() - in_before) as usize,
            produced: (self.inner.total_out() - out_before) as usize,
            stream_end: status == Status::StreamEnd,
        })
    }
}
