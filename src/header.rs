//! RFC 1952 member header parser: fixed 10-byte header plus optional
//! FEXTRA / FNAME / FCOMMENT / FHCRC fields. Optional field payloads are
//! parsed and hashed but never retained, keeping memory bounded.

use crc32fast::Hasher;

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xE0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    BadMagic,
    BadMethod,
    BadFlags,
    BadHeaderCrc,
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Fixed,
    ExtraLen,
    Extra,
    Name,
    Comment,
    Hcrc,
}

pub enum Step {
    NeedMore,
    Done,
}

/// Byte-at-a-time header state machine. Every consumed byte is counted so
/// a per-member header size cap can be enforced.
pub struct HeaderParser {
    phase: Phase,
    fixed: [u8; 10],
    fixed_len: usize,
    hasher: Hasher,
    flags: u8,
    extra_remaining: usize,
    extra_len_len: usize,
    hcrc: u16,
    hcrc_len: usize,
    bytes: usize,
    limit: usize,
}

impl HeaderParser {
    pub fn new(limit: usize) -> Self {
        HeaderParser {
            phase: Phase::Fixed,
            fixed: [0; 10],
            fixed_len: 0,
            hasher: Hasher::new(),
            flags: 0,
            extra_remaining: 0,
            extra_len_len: 0,
            hcrc: 0,
            hcrc_len: 0,
            bytes: 0,
            limit,
        }
    }

    pub fn bytes_consumed(&self) -> usize {
        self.bytes
    }

    /// Feed one header byte. Returns Done when the full header (including
    /// the optional CRC16) has been consumed and validated.
    pub fn feed(&mut self, byte: u8) -> Result<Step, HeaderError> {
        self.bytes += 1;
        if self.bytes > self.limit {
            return Err(HeaderError::TooLarge);
        }
        match self.phase {
            Phase::Fixed => {
                // Validate each fixed field as soon as its byte arrives so
                // trailing garbage fails fast instead of looking truncated.
                match self.fixed_len {
                    0 if byte != 0x1f => return Err(HeaderError::BadMagic),
                    1 if byte != 0x8b => return Err(HeaderError::BadMagic),
                    2 if byte != 8 => return Err(HeaderError::BadMethod),
                    3 if byte & RESERVED != 0 => return Err(HeaderError::BadFlags),
                    _ => {}
                }
                self.fixed[self.fixed_len] = byte;
                self.fixed_len += 1;
                if self.fixed_len == 4 {
                    self.flags = self.fixed[3];
                }
                if self.fixed_len < 10 {
                    return Ok(Step::NeedMore);
                }
                self.hasher.update(&self.fixed);
                self.advance(self.after_fixed())
            }
            Phase::ExtraLen => {
                self.hasher.update(&[byte]);
                self.extra_remaining |= (byte as usize) << (8 * self.extra_len_len);
                self.extra_len_len += 1;
                if self.extra_len_len == 2 {
                    let next = if self.extra_remaining > 0 {
                        Some(Phase::Extra)
                    } else {
                        self.after_extra()
                    };
                    self.advance(next)
                } else {
                    Ok(Step::NeedMore)
                }
            }
            Phase::Extra => {
                self.hasher.update(&[byte]);
                self.extra_remaining -= 1;
                if self.extra_remaining == 0 {
                    let next = self.after_extra();
                    self.advance(next)
                } else {
                    Ok(Step::NeedMore)
                }
            }
            Phase::Name => {
                self.hasher.update(&[byte]);
                if byte == 0 {
                    let next = self.after_name();
                    self.advance(next)
                } else {
                    Ok(Step::NeedMore)
                }
            }
            Phase::Comment => {
                self.hasher.update(&[byte]);
                if byte == 0 {
                    let next = self.after_comment();
                    self.advance(next)
                } else {
                    Ok(Step::NeedMore)
                }
            }
            Phase::Hcrc => {
                // The CRC16 bytes themselves are not part of the hashed header.
                self.hcrc |= (byte as u16) << (8 * self.hcrc_len);
                self.hcrc_len += 1;
                if self.hcrc_len == 2 {
                    let expect = (self.hasher.clone().finalize() & 0xFFFF) as u16;
                    if self.hcrc != expect {
                        return Err(HeaderError::BadHeaderCrc);
                    }
                    return Ok(Step::Done);
                }
                Ok(Step::NeedMore)
            }
        }
    }

    /// Apply a phase transition; None means no optional fields remain.
    fn advance(&mut self, next: Option<Phase>) -> Result<Step, HeaderError> {
        match next {
            Some(phase) => {
                self.phase = phase;
                Ok(Step::NeedMore)
            }
            None => Ok(Step::Done),
        }
    }

    fn after_fixed(&self) -> Option<Phase> {
        if self.flags & FEXTRA != 0 {
            Some(Phase::ExtraLen)
        } else {
            self.after_extra()
        }
    }

    fn after_extra(&self) -> Option<Phase> {
        if self.flags & FNAME != 0 {
            Some(Phase::Name)
        } else {
            self.after_name()
        }
    }

    fn after_name(&self) -> Option<Phase> {
        if self.flags & FCOMMENT != 0 {
            Some(Phase::Comment)
        } else {
            self.after_comment()
        }
    }

    fn after_comment(&self) -> Option<Phase> {
        if self.flags & FHCRC != 0 {
            Some(Phase::Hcrc)
        } else {
            None
        }
    }
}
