# gzip_stream234

Incremental, bounded-memory gzip (RFC 1952) stream decoder built on
flate2 (incremental raw-DEFLATE) and crc32fast. No I/O, no allocation of
stream content: the caller supplies input chunks and output slices.

## API

```rust
use gzip_stream234::{Config, Decoder, Status};

let mut dec = Decoder::new(Config::default());
let mut out = [0u8; 4096];
let outcome = dec.process(input_chunk, &mut out, /* end_of_input */ false)?;
// outcome.consumed bytes of input were used, out[..outcome.produced] is data.
```

`process` returns `Ok(Outcome { consumed, produced, status })` with:

- `Status::NeedInput` — provide more input bytes.
- `Status::NeedOutput` — output slice is full; call again with fresh space.
- `Status::MemberVerified` — a member's CRC32 + ISIZE just verified.
- `Status::StreamDone` — `end_of_input` flagged at a clean member boundary.

Errors return `Error { kind, detail, consumed, produced }` where `kind`
distinguishes `Format`, `Checksum`, `Truncated`, `Limit` and `Terminated`,
and `consumed`/`produced` report the failing call's exact counts so the
caller never replays blindly. After any error the instance is terminated.

## Contract and verification boundary

- Input may be split at arbitrary byte boundaries; unconsumed input is
  presented again on the next call. No slice references are retained.
- Zero-length input or output slices report what is needed instead of
  busy-looping.
- Output delivered before `MemberVerified` is *unverified*: a corrupt
  member can deliver bytes before its trailer check fails.
- Concatenated members (including empty members) are supported; decoder
  and checksums reset between members. Trailing garbage is parsed as the
  next member header and rejected, never silently ignored.
- `end_of_input` flagged mid-member (header, deflate body, or trailer)
  yields `ErrorKind::Truncated`, distinct from format/checksum failures.

## Limits

- `Config::max_header_bytes` — per-member header cap (default 1 MiB).
  FEXTRA/FNAME/FCOMMENT payloads are parsed and hashed, never stored.
- `Config::max_output_bytes` — total decompressed-byte cap across the
  whole stream; output stops before exceeding it and the call fails with
  `Detail::OutputLimit`.
- Memory stays bounded regardless of input length or compression ratio.

## Example

```sh
cargo run --example chunked   # multi-member chunked decode + corrupt input
cargo test                    # 12 integration tests
```
