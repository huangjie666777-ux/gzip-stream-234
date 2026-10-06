//! Chunked decompression of a multi-member gzip stream, plus a corrupted
//! input demo. Run with: cargo run --example chunked

use flate2::write::GzEncoder;
use flate2::Compression;
use gzip_stream234::{Config, Decoder, Status};
use std::io::Write;

fn gzip_member(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn decode(stream: &[u8], in_chunk: usize, out_chunk: usize) {
    let mut dec = Decoder::new(Config::default());
    let mut pos = 0usize;
    let mut text = Vec::new();
    loop {
        let end = pos >= stream.len();
        let take = in_chunk.min(stream.len() - pos);
        let mut buf = vec![0u8; out_chunk];
        match dec.process(&stream[pos..pos + take], &mut buf, end) {
            Ok(o) => {
                pos += o.consumed;
                text.extend_from_slice(&buf[..o.produced]);
                match o.status {
                    Status::NeedInput | Status::NeedOutput => {}
                    Status::MemberVerified => {
                        println!("member verified ({} bytes so far)", dec.total_output())
                    }
                    Status::StreamDone => {
                        println!("stream done: {:?}", String::from_utf8_lossy(&text));
                        break;
                    }
                }
            }
            Err(e) => {
                println!(
                    "error {:?}/{:?} after consuming {} and producing {} in the failing call",
                    e.kind, e.detail, e.consumed, e.produced
                );
                break;
            }
        }
    }
}

fn main() {
    let mut stream = gzip_member(b"first member, ");
    stream.extend_from_slice(&gzip_member(b""));
    stream.extend_from_slice(&gzip_member(b"third member"));
    println!("-- good multi-member stream, 5-byte input / 7-byte output chunks --");
    decode(&stream, 5, 7);

    println!("-- corrupted CRC --");
    let mut bad = gzip_member(b"payload that will not verify");
    let n = bad.len();
    bad[n - 8] ^= 0xFF;
    decode(&bad, 64, 64);
}
