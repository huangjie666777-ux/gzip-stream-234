//! RFC 1952 成员头部的增量解析器。
//!
//! 逐字节喂入，内部只保留固定 10 字节前缀与少量计数器；
//! 文件名与注释只参与 CRC 计算，不做任何留存，因此任意长的
//! 可选字段都保持有界内存。limit 约束单成员头部总字节数。

use crc32fast::Hasher;

use crate::error::GzipError;

const FTEXT: u8 = 0x01;
const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xE0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Fixed,
    ExtraLenLo,
    ExtraLenHi,
    Extra,
    Name,
    Comment,
    CrcLo,
    CrcHi,
    Done,
}

/// 逐字节头部解析器。完成前不分配任何堆内存。
pub struct HeaderParser {
    state: State,
    fixed: [u8; 10],
    fixed_len: usize,
    flags: u8,
    extra_remaining: u16,
    crc: Hasher,
    crc_expected: u16,
    seen: u64,
    limit: u64,
}

impl HeaderParser {
    pub fn new(limit: u64) -> Self {
        HeaderParser {
            state: State::Fixed,
            fixed: [0; 10],
            fixed_len: 0,
            flags: 0,
            extra_remaining: 0,
            crc: Hasher::new(),
            crc_expected: 0,
            seen: 0,
            limit,
        }
    }

    /// 喂入一个字节；返回 Ok(true) 表示头部完整且校验通过。
    pub fn feed(&mut self, byte: u8) -> Result<bool, GzipError> {
        if self.state == State::Done {
            return Ok(true);
        }
        self.seen += 1;
        if self.seen > self.limit {
            return Err(GzipError::HeaderLimit { limit: self.limit });
        }
        // FHCRC 覆盖 CRC 字段之前的全部头部字节。
        if !matches!(self.state, State::CrcLo | State::CrcHi) {
            self.crc.update(&[byte]);
        }
        match self.state {
            State::Fixed => {
                self.fixed[self.fixed_len] = byte;
                self.fixed_len += 1;
                // 魔数逐字节提前校验，短垃圾输入也能立即报错。
                if self.fixed_len == 1 && self.fixed[0] != 0x1f {
                    return Err(GzipError::Format("bad magic bytes"));
                }
                if self.fixed_len == 2 && self.fixed[1] != 0x8b {
                    return Err(GzipError::Format("bad magic bytes"));
                }
                if self.fixed_len == 10 {
                    self.parse_fixed()?;
                }
            }
            State::ExtraLenLo => {
                self.extra_remaining = byte as u16;
                self.state = State::ExtraLenHi;
            }
            State::ExtraLenHi => {
                self.extra_remaining |= (byte as u16) << 8;
                self.state = if self.extra_remaining == 0 { self.after_extra() } else { State::Extra };
            }
            State::Extra => {
                self.extra_remaining -= 1;
                if self.extra_remaining == 0 {
                    self.state = self.after_extra();
                }
            }
            State::Name => {
                if byte == 0 {
                    self.state = if self.flags & FCOMMENT != 0 { State::Comment } else { self.after_text() };
                }
            }
            State::Comment => {
                if byte == 0 {
                    self.state = self.after_text();
                }
            }
            State::CrcLo => {
                self.crc_expected = byte as u16;
                self.state = State::CrcHi;
            }
            State::CrcHi => {
                self.crc_expected |= (byte as u16) << 8;
                let actual = (self.crc.clone().finalize() & 0xFFFF) as u16;
                if actual != self.crc_expected {
                    return Err(GzipError::HeaderCrc { expected: self.crc_expected, actual });
                }
                self.state = State::Done;
                return Ok(true);
            }
            State::Done => unreachable!(),
        }
        Ok(self.state == State::Done)
    }

    fn parse_fixed(&mut self) -> Result<(), GzipError> {
        let h = &self.fixed;
        if h[0] != 0x1f || h[1] != 0x8b {
            return Err(GzipError::Format("bad magic bytes"));
        }
        if h[2] != 8 {
            return Err(GzipError::Format("unsupported compression method"));
        }
        let flags = h[3];
        if flags & RESERVED != 0 {
            return Err(GzipError::Format("reserved flag bits set"));
        }
        let _ = flags & FTEXT; // 仅提示信息，无解析影响。
        self.flags = flags;
        self.state = if flags & FEXTRA != 0 {
            State::ExtraLenLo
        } else if flags & FNAME != 0 {
            State::Name
        } else if flags & FCOMMENT != 0 {
            State::Comment
        } else {
            self.after_text()
        };
        Ok(())
    }

    fn after_extra(&self) -> State {
        if self.flags & FNAME != 0 {
            State::Name
        } else if self.flags & FCOMMENT != 0 {
            State::Comment
        } else {
            self.after_text()
        }
    }

    fn after_text(&self) -> State {
        if self.flags & FHCRC != 0 { State::CrcLo } else { State::Done }
    }
}
