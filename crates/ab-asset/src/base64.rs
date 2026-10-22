//! Standard base64 (RFC 4648) decoding for glTF data URIs.

fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Decodes base64 text, ignoring whitespace and accepting the URL-safe alphabet.
/// Returns `None` on invalid characters or truncated input.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    let mut padding = 0;
    for c in text.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None; // data after padding
        }
        buffer = (buffer << 6) | value(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    // Leftover bits must be zero padding of a final partial group.
    if bits >= 6 || padding > 2 {
        return None;
    }
    Some(out)
}

/// Splits a `data:[<mime>][;base64],<payload>` URI and decodes the payload.
pub(crate) fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix("data:")?;
    let (header, payload) = rest.split_once(',')?;
    if header.ends_with(";base64") {
        decode(payload)
    } else {
        Some(payload.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rfc4648_vectors() {
        for (encoded, decoded) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
            ("Zm9v\nYmFy", "foobar"),
        ] {
            assert_eq!(
                decode(encoded).as_deref(),
                Some(decoded.as_bytes()),
                "{encoded}"
            );
        }
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(decode("Zm9v!"), None);
        assert_eq!(decode("Zg==Zg"), None);
        assert_eq!(decode("Z"), None);
    }

    #[test]
    fn decodes_data_uris() {
        assert_eq!(
            decode_data_uri("data:application/octet-stream;base64,AAEC").as_deref(),
            Some(&[0u8, 1, 2][..])
        );
        assert_eq!(
            decode_data_uri("data:text/plain,hi").as_deref(),
            Some(&b"hi"[..])
        );
        assert_eq!(decode_data_uri("file.bin"), None);
    }
}
