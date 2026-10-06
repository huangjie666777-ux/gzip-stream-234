use flate2::write::{DeflateEncoder, GzEncoder};
use flate2::Compression;
use gzip_stream234::{Config, Decoder, Detail, ErrorKind, Status};
use std::io::Write;

fn gzip_member(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Hand-built member exercising FEXTRA, FNAME, FCOMMENT and FHCRC.
fn fancy_member(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let flags: u8 = 0x02 | 0x04 | 0x08 | 0x10;
    let mut head = vec![0x1f, 0x8b, 8, flags, 0, 0, 0, 0, 0, 3];
    head.extend_from_slice(&[4, 0]); // XLEN = 4
    head.extend_from_slice(b"EXTR");
    head.extend_from_slice(b"file name.bin\0");
    head.extend_from_slice(b"a comment\0");
    let crc = crc32fast::hash(&head);
    head.extend_from_slice(&(crc as u16).to_le_bytes());
    out.extend_from_slice(&head);
    let mut def = DeflateEncoder::new(Vec::new(), Compression::fast());
    def.write_all(data).unwrap();
    out.extend_from_slice(&def.finish().unwrap());
    out.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Drive the decoder with the given chunk sizes until it finishes or errs.
fn run(data: &[u8], in_chunk: usize, out_chunk: usize) -> (Vec<u8>, usize, Status) {
    let mut dec = Decoder::new(Config::default());
    let mut out_all = Vec::new();
    let mut pos = 0usize;
    let mut members = 0usize;
    loop {
        let end = pos >= data.len();
        let take = in_chunk.min(data.len() - pos);
        let mut buf = vec![0u8; out_chunk];
        match dec.process(&data[pos..pos + take], &mut buf, end) {
            Ok(o) => {
                pos += o.consumed;
                out_all.extend_from_slice(&buf[..o.produced]);
                match o.status {
                    Status::MemberVerified => members += 1,
                    Status::StreamDone => return (out_all, members, Status::StreamDone),
                    Status::NeedInput => assert!(o.consumed > 0 || take == 0, "busy loop"),
                    Status::NeedOutput => assert!(o.produced > 0 || out_chunk == 0),
                }
            }
            Err(e) => panic!("unexpected error {:?} at pos {}", e, pos),
        }
    }
}

#[test]
fn single_member_various_chunkings() {
    let payload: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let gz = gzip_member(&payload);
    for &(ic, oc) in &[(usize::MAX, 1 << 16), (1, 1), (7, 13), (3, 1 << 16), (1 << 16, 5)] {
        let (out, members, _) = run(&gz, ic, oc);
        assert_eq!(out, payload, "chunking ({}, {})", ic, oc);
        assert_eq!(members, 1);
    }
}

#[test]
fn multi_member_and_empty_member() {
    let a = b"hello ".to_vec();
    let b = b"world".to_vec();
    let mut stream = gzip_member(&a);
    stream.extend_from_slice(&gzip_member(b"")); // empty member is legal
    stream.extend_from_slice(&gzip_member(&b));
    let (out, members, _) = run(&stream, 4, 3);
    assert_eq!(out, b"hello world");
    assert_eq!(members, 3);
}

#[test]
fn optional_header_fields_and_hcrc() {
    let payload = b"optional fields rock";
    let gz = fancy_member(payload);
    let (out, members, _) = run(&gz, 2, 4);
    assert_eq!(out, payload);
    assert_eq!(members, 1);
}

#[test]
fn corrupt_crc_is_checksum_error() {
    let mut gz = gzip_member(b"some data here");
    let n = gz.len();
    gz[n - 8] ^= 0xFF; // corrupt stored CRC32
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 64];
    let err = dec.process(&gz, &mut out, true).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Checksum);
    assert_eq!(err.detail, Detail::CrcMismatch);
    // Error reports exact consumption so the caller need not replay.
    assert_eq!(err.consumed, gz.len());
    assert_eq!(err.produced, 14);
    // Instance is terminated afterwards.
    let err2 = dec.process(&[], &mut out, true).unwrap_err();
    assert_eq!(err2.kind, ErrorKind::Terminated);
}

#[test]
fn corrupt_isize_is_checksum_error() {
    let mut gz = gzip_member(b"abcd");
    let n = gz.len();
    gz[n - 1] ^= 0x01;
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 64];
    let err = dec.process(&gz, &mut out, true).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Checksum);
    assert_eq!(err.detail, Detail::IsizeMismatch);
}

#[test]
fn truncated_final_input() {
    let gz = gzip_member(b"never finished");
    let cut = gz.len() - 5; // mid-trailer
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 64];
    let err = dec.process(&gz[..cut], &mut out, true).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Truncated);
}

#[test]
fn trailing_garbage_is_rejected() {
    let mut stream = gzip_member(b"ok");
    stream.extend_from_slice(b"GARBAGE");
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 16];
    let o = dec.process(&stream, &mut out, false).unwrap();
    assert_eq!(o.status, Status::MemberVerified);
    let err = dec
        .process(&stream[o.consumed..], &mut out, true)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Format);
    assert_eq!(err.detail, Detail::InvalidMagic);
}

#[test]
fn bad_header_crc_is_checksum_error() {
    let mut gz = fancy_member(b"x");
    // Flip a byte inside FNAME; header CRC16 no longer matches.
    gz[16] ^= 0x01;
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 16];
    let err = dec.process(&gz, &mut out, true).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Checksum);
    assert_eq!(err.detail, Detail::HeaderCrcMismatch);
}

#[test]
fn header_size_limit() {
    let gz = fancy_member(b"x");
    let mut dec = Decoder::new(Config {
        max_header_bytes: 12,
        ..Config::default()
    });
    let mut out = [0u8; 16];
    let err = dec.process(&gz, &mut out, true).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Limit);
    assert_eq!(err.detail, Detail::HeaderTooLarge);
}

#[test]
fn output_limit_stops_before_exceeding() {
    let payload = vec![7u8; 10_000];
    let gz = gzip_member(&payload);
    let mut dec = Decoder::new(Config {
        max_output_bytes: 5_000,
        ..Config::default()
    });
    let mut delivered = 0usize;
    let mut pos = 0usize;
    loop {
        let mut buf = [0u8; 512];
        match dec.process(&gz[pos..], &mut buf, true) {
            Ok(o) => {
                pos += o.consumed;
                delivered += o.produced;
                assert!(delivered <= 5_000);
            }
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::Limit);
                assert_eq!(e.detail, Detail::OutputLimit);
                delivered += e.produced;
                assert_eq!(delivered, 5_000);
                break;
            }
        }
    }
}

#[test]
fn zero_length_calls_do_not_busy_loop() {
    let gz = gzip_member(b"data");
    let mut dec = Decoder::new(Config::default());
    let mut out = [0u8; 8];
    // Empty input, not at end: immediately reports NeedInput.
    let o = dec.process(&[], &mut out, false).unwrap();
    assert_eq!(o.status, Status::NeedInput);
    assert_eq!((o.consumed, o.produced), (0, 0));
    // Feed header only, then ask again with empty input.
    let o = dec.process(&gz[..10], &mut out, false).unwrap();
    assert_eq!(o.consumed, 10);
    let o = dec.process(&[], &mut out, false).unwrap();
    assert_eq!(o.status, Status::NeedInput);
    assert_eq!((o.consumed, o.produced), (0, 0));
}

#[test]
fn highly_compressed_input_stays_bounded() {
    // ~10 MiB of zeros compresses to ~10 KiB; decoder memory stays small
    // and output is delivered incrementally through a tiny buffer.
    let payload = vec![0u8; 10 << 20];
    let gz = gzip_member(&payload);
    let (out, members, _) = run(&gz, 1024, 1024);
    assert_eq!(out.len(), payload.len());
    assert!(out.iter().all(|&b| b == 0));
    assert_eq!(members, 1);
}
