//! 单成员的 DEFLATE 解码与尾注（CRC32 + ISIZE）校验。
//!
//! 解码交给 flate2 的增量 Decompress；这里只负责推进解码、
//! 累计 CRC、逐字节收集 8 字节尾注并在集齐后校验。

use crc32fast::Hasher;
use flate2::{Decompress, FlushDecompress, Status};

use crate::error::GzipError;

/// 一次解码推进的结果。
pub enum BodyProgress {
    /// 输入已耗尽，需要更多输入。
    NeedInput,
    /// 输出切片已满（或输出预算已用尽但流未结束），需要更多输出空间。
    NeedOutput,
    /// DEFLATE 流结束，接下来应读取尾注。
    StreamEnd,
}

pub struct Member {
    dec: Decompress,
    crc: Hasher,
    trailer: [u8; 8],
    trailer_len: usize,
}

impl Member {
    pub fn new() -> Self {
        Member {
            dec: Decompress::new(false), // 原始 DEFLATE，不含 zlib 包装。
            crc: Hasher::new(),
            trailer: [0; 8],
            trailer_len: 0,
        }
    }

    /// 推进解压。out_budget 是本次允许产出的最大字节数（输出上限
    /// 的剩余额度与调用者输出切片剩余长度的较小者）。
    pub fn decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        out_budget: usize,
    ) -> Result<(usize, usize, BodyProgress), GzipError> {
        let cap = output.len().min(out_budget);
        let before_in = self.dec.total_in();
        let before_out = self.dec.total_out();
        let status = self
            .dec
            .decompress(input, &mut output[..cap], FlushDecompress::None)
            .map_err(|_| GzipError::Format("corrupt deflate stream"))?;
        let consumed = (self.dec.total_in() - before_in) as usize;
        let produced = (self.dec.total_out() - before_out) as usize;
        self.crc.update(&output[..produced]);
        let progress = match status {
            Status::StreamEnd => BodyProgress::StreamEnd,
            Status::Ok | Status::BufError => {
                if consumed == input.len() {
                    BodyProgress::NeedInput
                } else {
                    BodyProgress::NeedOutput
                }
            }
        };
        Ok((consumed, produced, progress))
    }

    /// 喂入尾注的一个字节；集齐 8 字节后校验 CRC32 与 ISIZE。
    pub fn feed_trailer(&mut self, byte: u8) -> Result<bool, GzipError> {
        self.trailer[self.trailer_len] = byte;
        self.trailer_len += 1;
        if self.trailer_len < 8 {
            return Ok(false);
        }
        let expected_crc = u32::from_le_bytes(self.trailer[0..4].try_into().unwrap());
        let expected_isize = u32::from_le_bytes(self.trailer[4..8].try_into().unwrap());
        let actual_crc = self.crc.clone().finalize();
        if actual_crc != expected_crc {
            return Err(GzipError::DataCrc { expected: expected_crc, actual: actual_crc });
        }
        let actual_isize = (self.dec.total_out() & 0xFFFF_FFFF) as u32;
        if actual_isize != expected_isize {
            return Err(GzipError::Isize { expected: expected_isize, actual: actual_isize });
        }
        Ok(true)
    }
}
