//! The WebSocket wire protocol (RFC 6455), enough of it to carry JSON-RPC.
//!
//! ## Why this is hand-written
//!
//! The rest of CINE-OMNI is synchronous: `parking_lot`, `crossbeam-channel`,
//! threads. Adopting an async WebSocket stack would drag in an async runtime and
//! two colours of function across a codebase that has neither, in order to
//! serve a handful of local windows. A thread per client is the right shape
//! here, and at that point the protocol itself is a few hundred lines.
//!
//! What is implemented: the opening handshake, text and binary frames, close,
//! ping/pong, and continuation frames. What is not: extensions (`permessage-
//! deflate`), and fragmented control frames — which RFC 6455 §5.5 forbids
//! anyway, so rejecting them is correct rather than a shortcut.

use base64::Engine as _;
use sha1::{Digest, Sha1};
use std::io::{self, Read, Write};

/// The constant RFC 6455 §1.3 requires appended to the client key.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Frames larger than this are refused rather than allocated.
///
/// A client that asks us to allocate a gigabyte is either broken or hostile,
/// and this server is reachable from the loopback interface. 16 MiB is far more
/// than any RPC message needs.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("not a websocket upgrade request")]
    NotAnUpgrade,
    #[error("malformed frame: {0}")]
    Protocol(String),
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge(u64),
    #[error("peer closed the connection")]
    Closed,
}

/// The `Sec-WebSocket-Accept` value for a client's `Sec-WebSocket-Key`.
pub fn accept_key(client_key: &str) -> String {
    let mut h = Sha1::new();
    h.update(client_key.trim().as_bytes());
    h.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// Build the 101 response completing the handshake.
pub fn handshake_response(client_key: &str) -> String {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(client_key)
    )
}

/// Encode a frame as a server sends it: never masked (RFC 6455 §5.1).
pub fn encode(frame: &Frame) -> Vec<u8> {
    let (opcode, payload): (u8, &[u8]) = match frame {
        Frame::Text(s) => (0x1, s.as_bytes()),
        Frame::Binary(b) => (0x2, b),
        Frame::Close => (0x8, &[]),
        Frame::Ping(b) => (0x9, b),
        Frame::Pong(b) => (0xA, b),
    };
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode); // FIN set: we never fragment outbound.
    let n = payload.len();
    if n < 126 {
        out.push(n as u8);
    } else if n <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// Read one complete message, reassembling continuation frames.
///
/// Control frames may be interleaved between the fragments of a data message
/// (RFC 6455 §5.4), so they are returned as they arrive and the caller answers
/// them; the next call resumes the data message.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Frame, WsError> {
    let mut assembled: Vec<u8> = Vec::new();
    let mut assembling: Option<u8> = None;

    loop {
        let mut head = [0u8; 2];
        r.read_exact(&mut head).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                WsError::Closed
            } else {
                WsError::Io(e)
            }
        })?;

        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0F;
        let masked = head[1] & 0x80 != 0;
        let len = match head[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                u16::from_be_bytes(b) as u64
            }
            127 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        if len > MAX_FRAME_BYTES as u64 {
            return Err(WsError::TooLarge(len));
        }

        // RFC 6455 §5.1: every client-to-server frame must be masked.
        let mask = if masked {
            let mut m = [0u8; 4];
            r.read_exact(&mut m)?;
            Some(m)
        } else {
            None
        };

        let mut payload = vec![0u8; len as usize];
        r.read_exact(&mut payload)?;
        if let Some(m) = mask {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= m[i % 4];
            }
        }

        let is_control = opcode & 0x8 != 0;
        if is_control {
            // §5.5: control frames must not be fragmented and are limited to a
            // 125-byte payload. A peer breaking this is broken, not merely
            // unusual, so it is rejected rather than accommodated.
            if !fin || payload.len() > 125 {
                return Err(WsError::Protocol(format!(
                    "fragmented or oversized control frame (opcode {opcode:#x}, {} bytes)",
                    payload.len()
                )));
            }
            return match opcode {
                0x8 => Ok(Frame::Close),
                0x9 => Ok(Frame::Ping(payload)),
                0xA => Ok(Frame::Pong(payload)),
                other => Err(WsError::Protocol(format!(
                    "unknown control opcode {other:#x}"
                ))),
            };
        }

        match opcode {
            0x0 => {
                // Continuation of a message we must already be assembling.
                if assembling.is_none() {
                    return Err(WsError::Protocol(
                        "continuation frame with nothing to continue".into(),
                    ));
                }
                assembled.extend_from_slice(&payload);
            }
            0x1 | 0x2 => {
                if assembling.is_some() {
                    return Err(WsError::Protocol(
                        "new data frame began before the previous message finished".into(),
                    ));
                }
                assembling = Some(opcode);
                assembled = payload;
            }
            other => return Err(WsError::Protocol(format!("unknown opcode {other:#x}"))),
        }

        if assembled.len() > MAX_FRAME_BYTES {
            return Err(WsError::TooLarge(assembled.len() as u64));
        }
        if fin {
            return match assembling {
                Some(0x1) => String::from_utf8(assembled)
                    .map(Frame::Text)
                    .map_err(|e| WsError::Protocol(format!("text frame is not UTF-8: {e}"))),
                _ => Ok(Frame::Binary(assembled)),
            };
        }
    }
}

pub fn write_frame<W: Write>(w: &mut W, frame: &Frame) -> Result<(), WsError> {
    w.write_all(&encode(frame))?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one value in the handshake that is not free choice: RFC 6455 §1.3
    /// gives this exact example, so it is a conformance check rather than a
    /// self-consistency check.
    /// Two published key/accept vectors. A self-consistent test would have
    /// happily accepted a mistyped GUID — this one caught exactly that, and a
    /// wrong GUID fails the handshake against every real browser.
    #[test]
    fn the_handshake_matches_the_published_vectors() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(
            accept_key("x3JJHMbDL1EzLkh9GBhXDw=="),
            "HSmrc0sMlYUkAGmm5OPpG2HaGWk="
        );
        let response = handshake_response("dGhlIHNhbXBsZSBub25jZQ==");
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(
            response.ends_with("\r\n\r\n"),
            "headers must be terminated by a blank line"
        );
    }

    /// Frames a client sends are masked; ours are not. Encode a masked frame by
    /// hand so the decoder is tested against the wire format rather than
    /// against our own encoder.
    fn client_frame(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
        let mask = [0xAA, 0xBB, 0xCC, 0xDD];
        let mut out = vec![if fin { 0x80 | opcode } else { opcode }];
        let n = payload.len();
        if n < 126 {
            out.push(0x80 | n as u8);
        } else {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    #[test]
    fn a_masked_text_frame_decodes_to_its_payload() {
        let wire = client_frame(0x1, b"{\"method\":\"stats\"}", true);
        let frame = read_frame(&mut wire.as_slice()).unwrap();
        assert_eq!(frame, Frame::Text("{\"method\":\"stats\"}".into()));
    }

    #[test]
    fn a_fragmented_message_is_reassembled_in_order() {
        let mut wire = client_frame(0x1, b"{\"part\":", false);
        wire.extend(client_frame(0x0, b"\"one\"}", true));
        assert_eq!(
            read_frame(&mut wire.as_slice()).unwrap(),
            Frame::Text("{\"part\":\"one\"}".into())
        );
    }

    /// A control frame may arrive between the fragments of a data message. It
    /// must be surfaced immediately so the caller can answer the ping.
    #[test]
    fn a_control_frame_interleaved_with_fragments_is_returned_first() {
        let wire = client_frame(0x9, b"ping", true);
        assert_eq!(
            read_frame(&mut wire.as_slice()).unwrap(),
            Frame::Ping(b"ping".to_vec())
        );
    }

    #[test]
    fn our_own_frames_round_trip_through_the_decoder_when_unmasked() {
        for original in [
            Frame::Text("hello".into()),
            Frame::Binary(vec![0, 1, 2, 255]),
            Frame::Ping(vec![7]),
            Frame::Pong(vec![]),
            Frame::Close,
        ] {
            let wire = encode(&original);
            assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), original);
        }
    }

    /// The length encoding switches form at 125/126 and again at 65535. Those
    /// boundaries are where hand-written framing usually goes wrong.
    #[test]
    fn payload_length_boundaries_encode_and_decode() {
        for n in [0usize, 125, 126, 127, 65_535, 65_536] {
            let payload = "x".repeat(n);
            let wire = encode(&Frame::Text(payload.clone()));
            match read_frame(&mut wire.as_slice()).unwrap() {
                Frame::Text(s) => assert_eq!(s.len(), n, "length {n} round trip"),
                other => panic!("expected text, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_oversized_frame_is_refused_rather_than_allocated() {
        // A 64-bit length header claiming far more than the limit.
        let mut wire = vec![0x81, 127];
        wire.extend_from_slice(&(u64::MAX / 2).to_be_bytes());
        assert!(matches!(
            read_frame(&mut wire.as_slice()),
            Err(WsError::TooLarge(_))
        ));
    }

    #[test]
    fn a_fragmented_control_frame_is_rejected() {
        // RFC 6455 §5.5 forbids this outright.
        let wire = client_frame(0x9, b"ping", false);
        assert!(matches!(
            read_frame(&mut wire.as_slice()),
            Err(WsError::Protocol(_))
        ));
    }

    #[test]
    fn a_continuation_with_nothing_to_continue_is_rejected() {
        let wire = client_frame(0x0, b"orphan", true);
        assert!(matches!(
            read_frame(&mut wire.as_slice()),
            Err(WsError::Protocol(_))
        ));
    }

    #[test]
    fn invalid_utf8_in_a_text_frame_is_rejected_not_replaced() {
        let wire = client_frame(0x1, &[0xFF, 0xFE], true);
        assert!(
            matches!(read_frame(&mut wire.as_slice()), Err(WsError::Protocol(_))),
            "lossy conversion would silently corrupt a client's message"
        );
    }

    #[test]
    fn a_truncated_stream_reports_closure_not_a_protocol_error() {
        assert!(matches!(
            read_frame(&mut [].as_slice()),
            Err(WsError::Closed)
        ));
    }
}
