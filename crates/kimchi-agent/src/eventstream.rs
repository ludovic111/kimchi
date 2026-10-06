//! The AWS event stream framing (`application/vnd.amazon.eventstream`), which Bedrock's
//! ConverseStream answers in.
//!
//! Each message: total length (u32, big endian), headers length (u32), a CRC32 of those eight
//! bytes, the headers, the payload, and a CRC32 of everything before it. A header is a one-byte
//! name length, the name, a one-byte value type and the value (type 7: a string with a u16
//! length). As described in the AWS SDKs' event stream encoding
//! (docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html#streaming-event-stream).

/// One decoded message.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub headers: Vec<(String, String)>,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Collects bytes as they arrive and hands out whole messages.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

/// Largest message accepted (Bedrock's are small; this only stops a broken stream).
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, `Ok(None)` until one has arrived.
    pub fn next(&mut self) -> Result<Option<Message>, String> {
        if self.buf.len() < 12 {
            return Ok(None);
        }
        let total = u32::from_be_bytes(self.buf[0..4].try_into().unwrap_or_default()) as usize;
        let headers_len = u32::from_be_bytes(self.buf[4..8].try_into().unwrap_or_default()) as usize;
        let prelude_crc = u32::from_be_bytes(self.buf[8..12].try_into().unwrap_or_default());
        if crc32fast::hash(&self.buf[..8]) != prelude_crc {
            return Err("The response stream is damaged (prelude checksum).".into());
        }
        if total < 16 + headers_len || total > MAX_MESSAGE {
            return Err("The response stream is damaged (message length).".into());
        }
        if self.buf.len() < total {
            return Ok(None);
        }
        let frame: Vec<u8> = self.buf.drain(..total).collect();
        let crc = u32::from_be_bytes(frame[total - 4..].try_into().unwrap_or_default());
        if crc32fast::hash(&frame[..total - 4]) != crc {
            return Err("The response stream is damaged (message checksum).".into());
        }
        let headers = parse_headers(&frame[12..12 + headers_len])?;
        Ok(Some(Message { headers, payload: frame[12 + headers_len..total - 4].to_vec() }))
    }
}

fn parse_headers(mut b: &[u8]) -> Result<Vec<(String, String)>, String> {
    let bad = || "The response stream is damaged (headers).".to_string();
    let mut out = vec![];
    while !b.is_empty() {
        let n = *b.first().ok_or_else(bad)? as usize;
        let name = String::from_utf8_lossy(b.get(1..1 + n).ok_or_else(bad)?).into_owned();
        b = &b[1 + n..];
        let kind = *b.first().ok_or_else(bad)?;
        b = &b[1..];
        let take = |b: &mut &[u8], len: usize| -> Result<Vec<u8>, String> {
            let v = b.get(..len).ok_or_else(bad)?.to_vec();
            *b = &b[len..];
            Ok(v)
        };
        let value = match kind {
            0 => "true".to_string(),
            1 => "false".to_string(),
            2 => take(&mut b, 1)?[0].to_string(),
            3 => i16::from_be_bytes(take(&mut b, 2)?.try_into().unwrap_or_default()).to_string(),
            4 => i32::from_be_bytes(take(&mut b, 4)?.try_into().unwrap_or_default()).to_string(),
            5 | 8 => i64::from_be_bytes(take(&mut b, 8)?.try_into().unwrap_or_default()).to_string(),
            6 | 7 => {
                let len = u16::from_be_bytes(take(&mut b, 2)?.try_into().unwrap_or_default()) as usize;
                String::from_utf8_lossy(&take(&mut b, len)?).into_owned()
            }
            9 => crate::sigv4::hex(&take(&mut b, 16)?),
            _ => return Err(bad()),
        };
        out.push((name, value));
    }
    Ok(out)
}

/// Encodes a message with string headers (tests, and the mock servers in them).
#[cfg(test)]
pub fn encode(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut h = vec![];
    for (k, v) in headers {
        h.push(k.len() as u8);
        h.extend_from_slice(k.as_bytes());
        h.push(7);
        h.extend_from_slice(&(v.len() as u16).to_be_bytes());
        h.extend_from_slice(v.as_bytes());
    }
    let total = 16 + h.len() + payload.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(h.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32fast::hash(&out[..8]).to_be_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(payload);
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_in_pieces() {
        let a = encode(&[(":event-type", "messageStart"), (":message-type", "event")], br#"{"role":"assistant"}"#);
        let b = encode(&[(":event-type", "messageStop")], br#"{"stopReason":"end_turn"}"#);
        let all: Vec<u8> = a.iter().chain(&b).copied().collect();
        let mut d = Decoder::default();
        let mut got = vec![];
        for chunk in all.chunks(7) {
            d.push(chunk);
            while let Some(m) = d.next().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].header(":event-type"), Some("messageStart"));
        assert_eq!(got[1].payload, br#"{"stopReason":"end_turn"}"#);
    }

    #[test]
    fn a_damaged_message_is_an_error() {
        let mut a = encode(&[(":event-type", "x")], b"{}");
        let n = a.len();
        a[n - 6] ^= 1;
        let mut d = Decoder::default();
        d.push(&a);
        assert!(d.next().is_err());
    }
}
