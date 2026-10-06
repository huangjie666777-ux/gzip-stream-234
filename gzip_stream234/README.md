# gzip_stream234

可嵌入的流式 gzip 解压库（RFC 1952），Rust 1.85.1，依赖 flate2 1.1.2
（增量 DEFLATE 解码）与 crc32fast 1.4.2。无前端、无 HTTP。

## API

- GzipStream::new(Config)：创建实例。Config 可配 max_header_bytes
  （单成员头部上限，默认 1 MiB）与 max_output_bytes（全流解压字节
  上限，默认不限）。
- process(input, output, end) -> Outcome：喂入任意切分的输入切片与
  输出切片；返回 consumed / produced 与状态。未消费的输入由调用者
  下次原样重传，库不保留引用；零长切片安全，不会忙循环。
- 状态：NeedInput（输入耗尽）、NeedOutput（输出写满，续接不丢失
  或重复字节）、MemberVerified（一个成员 CRC32 + ISIZE 校验通过）、
  Done（end 标记下末成员完整校验，全流完成）。

## 验证边界

MemberVerified 之前交付的输出属于待验证数据：只有成员尾注的 CRC32
与 ISIZE 都校验通过才报告该状态。需要强一致性的调用者应缓冲输出
直到收到 MemberVerified。成员结束但输入未标结束时，库等待后续成员；
成员间出现非 gzip 数据（尾随垃圾）报 Format 错误，绝不忽略。

## 错误

GzipError 区分：Format（结构不符）、HeaderCrc / DataCrc / Isize
（校验不符）、Truncated（end 时流未完整）、HeaderLimit / OutputLimit
（超限，超限前已停止产出）。错误不可恢复：实例终止，出错调用返回
本次已消费与已交付量，后续调用以 0/0 重复同一错误，调用者不应盲目
重放。

## 限制

- 不自行实现 DEFLATE，依赖 flate2 增量解码。
- 文件名/注释只解析不留存，FEXTRA 跳过；不保存完整输入或已输出
  内容，长文件与高压缩比输入内存有界。
- 连续多成员与空成员合法；成员间重置解码器与校验状态。

## 文件结构

- src/lib.rs：公共 API、状态机与各阶段协作。
- src/header.rs：成员头部增量解析（固定头、可选字段、头 CRC）。
- src/member.rs：DEFLATE 推进、CRC 累计、尾注校验。
- src/error.rs：错误类型。
- tests/stream.rs：自测（cargo test）。
- examples/chunked.rs：分块解压多成员与损坏输入示例
  （cargo run --example chunked）。
