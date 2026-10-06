//! 示例：分块解压多成员 gzip 流，以及损坏输入的错误表现。
//! 运行：cargo run --example chunked

use flate2::write::GzEncoder;
use flate2::Compression;
use gzip_stream234::{Config, GzipStream, Status};
use std::io::Write;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn main() {
    // 拼接三个成员，其中第二个是空成员。
    let mut archive = gzip(b"first member\n");
    archive.extend_from_slice(&gzip(b""));
    archive.extend_from_slice(&gzip(b"third member\n"));

    let mut stream = GzipStream::new(Config::default());
    let mut text = Vec::new();
    let mut in_pos = 0usize;
    let mut out_buf = [0u8; 7]; // 故意用小输出切片演示暂停/续接。
    println!("== multi-member, chunked input (9-byte chunks) ==");
    loop {
        let take = 9.min(archive.len() - in_pos);
        let end = in_pos + take == archive.len();
        let outcome = stream.process(&archive[in_pos..in_pos + take], &mut out_buf, end);
        in_pos += outcome.consumed;
        text.extend_from_slice(&out_buf[..outcome.produced]);
        match outcome.status {
            Ok(Status::MemberVerified) => println!("member {} verified", stream.members_verified()),
            Ok(Status::Done) => break,
            Ok(_) => {}
            Err(err) => panic!("unexpected error: {err}"),
        }
    }
    print!("decompressed: {}", String::from_utf8_lossy(&text));

    // 损坏输入：篡改 CRC32。
    println!("== corrupted member ==");
    let mut bad = gzip(b"payload");
    let n = bad.len();
    bad[n - 8] ^= 0xFF;
    let mut stream = GzipStream::new(Config::default());
    let outcome = stream.process(&bad, &mut out_buf, true);
    println!(
        "consumed={} produced={} status={:?}",
        outcome.consumed, outcome.produced, outcome.status
    );
}
