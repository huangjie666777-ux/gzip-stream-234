//! gzip_stream234: incremental, bounded-memory gzip stream decoder.
//!
//! The caller drives decoding by repeatedly calling [`Decoder::process`]
//! with a chunk of input, an output slice, and an end-of-input flag.
//! Each call reports how many input bytes were consumed and how many
//! output bytes were produced, plus a [`Status`] telling the caller what
//! to do next. Input may be split at arbitrary byte boundaries and the
//! output slice may be of any size; no bytes are lost or duplicated when
//! a call pauses for more input or output space.
//!
//! The decoder handles RFC 1952 headers (including FEXTRA, FNAME,
//! FCOMMENT and FHCRC), verifies CRC32 and ISIZE for every member, and
//! supports concatenated multi-member streams. Members are only reported
//! as verified after their trailer checks pass; output delivered before
//! that point is unverified. Trailing garbage after a member is never
//! silently ignored: it is parsed as the next member header and errors
//! if invalid.
//!
//! Memory use is bounded: file names and comments are parsed but not
//! stored, and neither the full input nor already-delivered output is
//! retained. Configurable caps limit per-member header size and total
//! decompressed bytes.

mod header;
mod inflate;
mod trailer;

use crc32fast::Hasher;
use header::{HeaderError, HeaderParser};
use inflate::Inflater;
use trailer::{Trailer, TrailerError};

/// Decoder configuration.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Maximum bytes consumed by a single member header (optional fields
    /// included). Exceeding it fails with [`Detail::HeaderTooLarge`].
    pub max_header_bytes: usize,
    /// Maximum total decompressed bytes delivered across the whole stream.
    /// Output stops before exceeding the cap and the decoder fails with
    /// [`Detail::OutputLimit`].
    pub max_output_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_header_bytes: 1 << 20,
            max_output_bytes: u64::MAX,
        }
    }
}

/// What the caller should do after a successful call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// More input bytes are needed to make progress.
    NeedInput,
    /// The output slice is full (or too small); call again with more space.
    NeedOutput,
    /// A member's CRC32 and ISIZE verified successfully. All output
    /// delivered for that member up to and including this call is now
    /// verified. More members may follow.
    MemberVerified,
    /// End-of-input was flagged and the stream ended cleanly on a member
    /// boundary. No further calls are meaningful.
    StreamDone,
}

/// Coarse error category, for callers that only need to distinguish
/// format errors, checksum mismatches and truncated final input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Malformed gzip structure (bad magic, method, flags, deflate data).
    Format,
    /// Header CRC16, member CRC32 or ISIZE did not match.
    Checksum,
    /// Input ended (end-of-input flagged) in the middle of a member.
    Truncated,
    /// A configured limit (header size or output bytes) was exceeded.
    Limit,
    /// The decoder was used after a previous error terminated it.
    Terminated,
}

/// Detailed failure reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    InvalidMagic,
    InvalidMethod,
    InvalidFlags,
    HeaderCrcMismatch,
    HeaderTooLarge,
    CrcMismatch,
    IsizeMismatch,
    InflateError,
    OutputLimit,
    Truncated,
    Terminated,
}

/// Fatal error. Carries the bytes consumed and produced by the failing
/// call so the caller never has to replay blindly. After an error the
/// decoder is terminated and further calls fail with [`Detail::Terminated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub detail: Detail,
    pub consumed: usize,
    pub produced: usize,
}

/// Result of a successful [`Decoder::process`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub consumed: usize,
    pub produced: usize,
    pub status: Status,
}

enum State {
    Header(HeaderParser),
    Body { inflater: Inflater, hasher: Hasher, size: u64 },
    Trailer { trailer: Trailer, crc: u32, size: u64 },
    Done,
    Failed,
}

/// Streaming gzip decoder. See crate-level docs for the contract.
pub struct Decoder {
    config: Config,
    state: State,
    total_output: u64,
}

impl Decoder {
    pub fn new(config: Config) -> Self {
        Decoder {
            config,
            state: State::Header(HeaderParser::new(config.max_header_bytes)),
            total_output: 0,
        }
    }

    /// Total decompressed bytes delivered so far (verified and unverified).
    pub fn total_output(&self) -> u64 {
        self.total_output
    }

    /// Process one chunk. `input` may be empty and `output` may be empty;
    /// such calls report what is needed instead of busy-looping. The
    /// decoder never retains references to either slice. Set
    /// `end_of_input` when no more input will ever arrive.
    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        end_of_input: bool,
    ) -> Result<Outcome, Error> {
        if matches!(self.state, State::Failed) {
            return Err(Error {
                kind: ErrorKind::Terminated,
                detail: Detail::Terminated,
                consumed: 0,
                produced: 0,
            });
        }
        if matches!(self.state, State::Done) {
            return Ok(Outcome {
                consumed: 0,
                produced: 0,
                status: Status::StreamDone,
            });
        }
        match self.run(input, output, end_of_input) {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                self.state = State::Failed;
                Err(err)
            }
        }
    }

    fn run(&mut self, input: &[u8], output: &mut [u8], end: bool) -> Result<Outcome, Error> {
        let mut consumed = 0usize;
        let mut produced = 0usize;
        loop {
            match &mut self.state {
                State::Header(parser) => {
                    if consumed == input.len() {
                        if end {
                            if parser.bytes_consumed() == 0 {
                                // Clean member boundary: stream is complete.
                                self.state = State::Done;
                                return Ok(Outcome {
                                    consumed,
                                    produced,
                                    status: Status::StreamDone,
                                });
                            }
                            return Err(fail(
                                ErrorKind::Truncated,
                                Detail::Truncated,
                                consumed,
                                produced,
                            ));
                        }
                        return Ok(Outcome {
                            consumed,
                            produced,
                            status: Status::NeedInput,
                        });
                    }
                    match parser.feed(input[consumed]) {
                        Ok(header::Step::NeedMore) => consumed += 1,
                        Ok(header::Step::Done) => {
                            consumed += 1;
                            self.state = State::Body {
                                inflater: Inflater::new(),
                                hasher: Hasher::new(),
                                size: 0,
                            };
                        }
                        Err(e) => {
                            consumed += 1;
                            let (kind, detail) = match e {
                                HeaderError::BadMagic => (ErrorKind::Format, Detail::InvalidMagic),
                                HeaderError::BadMethod => (ErrorKind::Format, Detail::InvalidMethod),
                                HeaderError::BadFlags => (ErrorKind::Format, Detail::InvalidFlags),
                                HeaderError::BadHeaderCrc => {
                                    (ErrorKind::Checksum, Detail::HeaderCrcMismatch)
                                }
                                HeaderError::TooLarge => (ErrorKind::Limit, Detail::HeaderTooLarge),
                            };
                            return Err(fail(kind, detail, consumed, produced));
                        }
                    }
                }
                State::Body { inflater, hasher, size } => {
                    let remaining_limit = self.config.max_output_bytes - self.total_output;
                    let out_cap = (output.len() - produced).min(remaining_limit as usize);
                    let step = inflater
                        .run(&input[consumed..], &mut output[produced..produced + out_cap])
                        .map_err(|_| fail(ErrorKind::Format, Detail::InflateError, consumed, produced))?;
                    consumed += step.consumed;
                    produced += step.produced;
                    self.total_output += step.produced as u64;
                    if step.produced > 0 {
                        hasher.update(&output[produced - step.produced..produced]);
                        *size += step.produced as u64;
                    }
                    if step.stream_end {
                        let crc = hasher.clone().finalize();
                        let size = *size;
                        self.state = State::Trailer {
                            trailer: Trailer::new(),
                            crc,
                            size,
                        };
                        continue;
                    }
                    if step.produced > 0 && self.total_output == self.config.max_output_bytes {
                        // Cap reached exactly, with more deflate data pending.
                        return Err(fail(ErrorKind::Limit, Detail::OutputLimit, consumed, produced));
                    }
                    if step.consumed == 0 && step.produced == 0 {
                        if out_cap == 0 && output.len() > produced {
                            // Output cap hit with no room to decode further.
                            return Err(fail(ErrorKind::Limit, Detail::OutputLimit, consumed, produced));
                        }
                        if consumed == input.len() {
                            if end {
                                return Err(fail(
                                    ErrorKind::Truncated,
                                    Detail::Truncated,
                                    consumed,
                                    produced,
                                ));
                            }
                            return Ok(Outcome {
                                consumed,
                                produced,
                                status: Status::NeedInput,
                            });
                        }
                        return Ok(Outcome {
                            consumed,
                            produced,
                            status: Status::NeedOutput,
                        });
                    }
                    if produced == output.len() {
                        return Ok(Outcome {
                            consumed,
                            produced,
                            status: Status::NeedOutput,
                        });
                    }
                }
                State::Trailer { trailer, crc, size } => {
                    let (took, complete) = trailer.feed(&input[consumed..]);
                    consumed += took;
                    if !complete {
                        if end {
                            return Err(fail(
                                ErrorKind::Truncated,
                                Detail::Truncated,
                                consumed,
                                produced,
                            ));
                        }
                        return Ok(Outcome {
                            consumed,
                            produced,
                            status: Status::NeedInput,
                        });
                    }
                    let crc = *crc;
                    let size = *size;
                    match trailer.verify(crc, size) {
                        Ok(()) => {
                            self.state = State::Header(HeaderParser::new(self.config.max_header_bytes));
                            return Ok(Outcome {
                                consumed,
                                produced,
                                status: Status::MemberVerified,
                            });
                        }
                        Err(TrailerError::CrcMismatch { .. }) => {
                            return Err(fail(ErrorKind::Checksum, Detail::CrcMismatch, consumed, produced));
                        }
                        Err(TrailerError::SizeMismatch { .. }) => {
                            return Err(fail(ErrorKind::Checksum, Detail::IsizeMismatch, consumed, produced));
                        }
                    }
                }
                State::Done | State::Failed => unreachable!(),
            }
        }
    }

}

fn fail(kind: ErrorKind, detail: Detail, consumed: usize, produced: usize) -> Error {
    Error {
        kind,
        detail,
        consumed,
        produced,
    }
}
