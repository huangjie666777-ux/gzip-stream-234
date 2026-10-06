//! gzip_stream234 —— 可嵌入的流式 gzip 解压库（RFC 1952）。
//!
//! 调用者按任意切分喂入压缩字节，并提供输出切片；库边解压边交付，
//! 不保存完整输入或已输出内容，内存占用有界。支持连续多成员、
//! 空成员、头部可选字段与头 CRC，逐成员校验 CRC32 与 ISIZE。
//!
//! # API 概览
//! - GzipStream::process：喂入输入切片、输出切片与输入结束标记，
//!   返回本次消费/产出数量与 Status。未消费的输入由调用者下次
//!   原样重传，库不保留其引用。
//! - Config：配置单成员头部字节上限与全流解压字节上限。
//! - GzipError：区分格式错误、校验不符与输入截断。
//!
//! # 验证边界
//! Status::MemberVerified 之前的输出属于待验证数据：只有当某成员
//! 的 CRC32 与 ISIZE 都校验通过后才报告该状态。调用者若需要强
//! 一致性，应缓冲输出直到收到 MemberVerified。
//!
//! # 限制
//! - 依赖 flate2 的增量 DEFLATE 解码，不自行实现 DEFLATE。
//! - 文件名/注释只解析不留存；FEXTRA 内容被跳过。
//! - 错误不可恢复：实例终止，后续调用重复返回同一错误。

mod error;
mod header;
mod member;

pub use error::GzipError;

use header::HeaderParser;
use member::{BodyProgress, Member};

/// 流配置。
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// 单成员头部最大字节数（默认 1 MiB），超限报 HeaderLimit。
    pub max_header_bytes: u64,
    /// 全流累计解压字节上限（默认无限制），超限前停止产出并报
    /// OutputLimit。
    pub max_output_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { max_header_bytes: 1 << 20, max_output_bytes: u64::MAX }
    }
}

/// 一次 process 调用的结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 输入已耗尽，需要更多输入。
    NeedInput,
    /// 输出切片已满，需要更多输出空间；未消费输入需下次重传。
    NeedOutput,
    /// 一个成员的 CRC32 与 ISIZE 校验通过。此前的输出已通过验证。
    MemberVerified,
    /// 输入已标记结束且最后一个成员完整校验，全流完成。
    Done,
}

/// process 的返回：本次消费与产出数量，以及状态或终止错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// 本次消费的输入字节数。
    pub consumed: usize,
    /// 本次写入输出切片的字节数。
    pub produced: usize,
    /// 状态；Err 表示实例已终止，后续调用重复返回同一错误。
    pub status: Result<Status, GzipError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Header,
    Body,
    Trailer,
    BetweenMembers,
    Finished,
}

/// 流式 gzip 解压器。单个实例处理一条（可多成员的）gzip 流。
pub struct GzipStream {
    config: Config,
    phase: Phase,
    header: HeaderParser,
    member: Member,
    total_out: u64,
    members_done: u64,
    error: Option<GzipError>,
}

impl GzipStream {
    pub fn new(config: Config) -> Self {
        GzipStream {
            config,
            phase: Phase::Header,
            header: HeaderParser::new(config.max_header_bytes),
            member: Member::new(),
            total_out: 0,
            members_done: 0,
            error: None,
        }
    }

    /// 已完整验证的成员数。
    pub fn members_verified(&self) -> u64 {
        self.members_done
    }

    /// 全流累计产出的解压字节数。
    pub fn bytes_produced(&self) -> u64 {
        self.total_out
    }

    /// 推进解压。
    ///
    /// - input：本次可用的压缩字节，可任意切分；返回的 consumed
    ///   之外的部分由调用者下次重传。
    /// - output：本次可写入的解压缓冲区；写满即暂停，续接不丢失
    ///   或重复字节。
    /// - end：true 表示这是最后一批输入。
    ///
    /// 零长切片安全：不会忙循环，直接报告 NeedInput/NeedOutput。
    pub fn process(&mut self, input: &[u8], output: &mut [u8], end: bool) -> Outcome {
        if let Some(err) = &self.error {
            return Outcome { consumed: 0, produced: 0, status: Err(err.clone()) };
        }
        match self.run(input, output, end) {
            Ok((consumed, produced, status)) => Outcome { consumed, produced, status: Ok(status) },
            Err((consumed, produced, err)) => {
                self.error = Some(err.clone());
                Outcome { consumed, produced, status: Err(err) }
            }
        }
    }

    fn run(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        end: bool,
    ) -> Result<(usize, usize, Status), (usize, usize, GzipError)> {
        let mut pos = 0usize;
        let mut produced = 0usize;
        macro_rules! fail {
            ($err:expr) => {
                return Err((pos, produced, $err))
            };
        }
        loop {
            match self.phase {
                Phase::Header => {
                    if pos == input.len() {
                        break self.finish(pos, produced, end, Status::NeedInput);
                    }
                    match self.header.feed(input[pos]) {
                        Ok(done) => {
                            pos += 1;
                            if done {
                                self.member = Member::new();
                                self.phase = Phase::Body;
                            }
                        }
                        Err(err) => fail!(err),
                    }
                }
                Phase::Body => {
                    let remaining = self.config.max_output_bytes - self.total_out;
                    let cap = (output.len() - produced).min(remaining as usize);
                    if cap == 0 {
                        if remaining == 0 {
                            fail!(GzipError::OutputLimit { limit: self.config.max_output_bytes });
                        }
                        break self.finish(pos, produced, end, Status::NeedOutput);
                    }
                    let (consumed, made, progress) = match self.member.decompress(
                        &input[pos..],
                        &mut output[produced..],
                        cap,
                    ) {
                        Ok(res) => res,
                        Err(err) => fail!(err),
                    };
                    pos += consumed;
                    produced += made;
                    self.total_out += made as u64;
                    match progress {
                        BodyProgress::StreamEnd => self.phase = Phase::Trailer,
                        BodyProgress::NeedInput => {
                            break self.finish(pos, produced, end, Status::NeedInput);
                        }
                        BodyProgress::NeedOutput => {
                            if self.total_out >= self.config.max_output_bytes {
                                fail!(GzipError::OutputLimit { limit: self.config.max_output_bytes });
                            }
                            break self.finish(pos, produced, end, Status::NeedOutput);
                        }
                    }
                }
                Phase::Trailer => {
                    if pos == input.len() {
                        break self.finish(pos, produced, end, Status::NeedInput);
                    }
                    match self.member.feed_trailer(input[pos]) {
                        Ok(done) => {
                            pos += 1;
                            if done {
                                self.members_done += 1;
                                self.phase = Phase::BetweenMembers;
                                return Ok((pos, produced, Status::MemberVerified));
                            }
                        }
                        Err(err) => fail!(err),
                    }
                }
                Phase::BetweenMembers => {
                    if pos == input.len() {
                        if end {
                            self.phase = Phase::Finished;
                            return Ok((pos, produced, Status::Done));
                        }
                        return Ok((pos, produced, Status::NeedInput));
                    }
                    // 还有输入：必须是下一个成员的头部，否则报格式错误，
                    // 绝不忽略尾随垃圾。
                    self.header = HeaderParser::new(self.config.max_header_bytes);
                    self.phase = Phase::Header;
                }
                Phase::Finished => {
                    if pos < input.len() {
                        fail!(GzipError::Format("trailing data after end of stream"));
                    }
                    return Ok((pos, produced, Status::Done));
                }
            }
        }
    }

    fn finish(
        &self,
        consumed: usize,
        produced: usize,
        end: bool,
        status: Status,
    ) -> Result<(usize, usize, Status), (usize, usize, GzipError)> {
        if end && status == Status::NeedInput {
            // 输入已结束但流不在合法的完成位置。
            Err((consumed, produced, GzipError::Truncated))
        } else {
            Ok((consumed, produced, status))
        }
    }
}
