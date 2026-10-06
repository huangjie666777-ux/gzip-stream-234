//! 错误类型：区分格式错误、校验不符与输入截断。

use core::fmt;

/// 不可恢复的错误。一旦出现，实例终止，后续调用重复返回同一错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GzipError {
    /// 头部或成员结构不符合 RFC 1952（魔数、方法、保留标志位、
    /// 缺少 NUL 终止、成员间出现非 gzip 数据等）。
    Format(&'static str),
    /// 头部 FHCRC 校验失败。
    HeaderCrc { expected: u16, actual: u16 },
    /// 成员数据 CRC32 校验失败。
    DataCrc { expected: u32, actual: u32 },
    /// 成员 ISIZE（原始长度 mod 2^32）与解压字节数不符。
    Isize { expected: u32, actual: u32 },
    /// 输入被标记结束时，流尚未到达合法结束位置（头/数据/尾注
    /// 被截断，或从未出现任何完整成员）。
    Truncated,
    /// 单成员头部超过配置的字节上限。
    HeaderLimit { limit: u64 },
    /// 全流解压输出超过配置的字节上限；在超限前已停止产出。
    OutputLimit { limit: u64 },
}

impl fmt::Display for GzipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GzipError::Format(msg) => write!(f, "gzip format error: {msg}"),
            GzipError::HeaderCrc { expected, actual } => {
                write!(f, "header crc mismatch: expected {expected:#06x}, got {actual:#06x}")
            }
            GzipError::DataCrc { expected, actual } => {
                write!(f, "data crc32 mismatch: expected {expected:#010x}, got {actual:#010x}")
            }
            GzipError::Isize { expected, actual } => {
                write!(f, "isize mismatch: expected {expected}, got {actual}")
            }
            GzipError::Truncated => write!(f, "input ended before stream completed"),
            GzipError::HeaderLimit { limit } => write!(f, "member header exceeds {limit} bytes"),
            GzipError::OutputLimit { limit } => write!(f, "decompressed output exceeds {limit} bytes"),
        }
    }
}

impl std::error::Error for GzipError {}
