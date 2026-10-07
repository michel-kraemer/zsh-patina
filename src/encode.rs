use std::borrow::Cow;

/// Decode a path that was encoded by our Zsh script with percent-encoding for
/// ASCII whitespace characters
#[deprecated = "Protocol version 1 will be removed in one of the next releases"]
pub fn decode_string_v1(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let decoded = match &bytes[i + 1..i + 3] {
                // the same characters are used by Rust's is_ascii_whitespace()
                b"20" => Some(b' '),
                b"09" => Some(b'\t'),
                b"0A" => Some(b'\n'),
                b"0D" => Some(b'\r'),
                b"0C" => Some(b'\x0C'),
                b"25" => Some(b'%'),
                _ => None,
            };
            if let Some(c) = decoded {
                out.push(c);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    // SAFETY: Because we copy the input string and only transform ASCII codes
    // into more ASCII codes, it's guaranteed to be UTF-8
    unsafe { String::from_utf8_unchecked(out) }
}

#[deprecated = "Protocol version 1 will be removed in one of the next releases"]
pub fn encode_string_v1(input: String) -> String {
    // Fast path: no encoding needed
    if !input
        .bytes()
        .any(|b| matches!(b, b'%' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
    {
        return input;
    }

    let mut out = Vec::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'%' => out.extend_from_slice(b"%25"),
            b' ' => out.extend_from_slice(b"%20"),
            b'\t' => out.extend_from_slice(b"%09"),
            b'\n' => out.extend_from_slice(b"%0A"),
            b'\r' => out.extend_from_slice(b"%0D"),
            b'\x0C' => out.extend_from_slice(b"%0C"),
            _ => out.push(b),
        }
    }

    // SAFETY: Because we copy the input string and only transform ASCII codes
    // into more ASCII codes, it's guaranteed to be UTF-8
    unsafe { String::from_utf8_unchecked(out) }
}

// Intentionally without an `unsafe` block, so that it's required at call-site
macro_rules! write_byte_unchecked {
    ($vec:ident, $byte:expr) => {
        let len = $vec.len();
        *$vec.as_mut_ptr().add(len) = $byte;
        $vec.set_len(len + 1);
    };
    ($vec:ident, $byte1:expr, $byte2:expr, $byte3:expr) => {
        let len = $vec.len();
        *$vec.as_mut_ptr().add(len) = $byte1;
        *$vec.as_mut_ptr().add(len + 1) = $byte2;
        *$vec.as_mut_ptr().add(len + 2) = $byte3;
        $vec.set_len(len + 3);
    };
}

pub fn decode_string<'a>(s: &'a str) -> Cow<'a, str> {
    let mut i = 0;
    let bytes = s.as_bytes();

    // check if we can return the string as is, but if not, at least count the
    // number of bytes we can copy verbatim
    'noop: {
        while i < bytes.len() {
            if bytes[i] == b'%' {
                break 'noop;
            }
            i += 1;
        }

        // fast path: nothing to decode
        return Cow::Borrowed(s);
    }

    // Allocate a vec to skip all the Unicode machinery of Rust which we don't
    // need.
    // IMPORTANT FOR SAFETY: Make `out` at least as large as the input string,
    // so we have enough room to copy bytes, and we will never need to
    // reallocate it.
    let mut out = Vec::<u8>::with_capacity(s.len());

    // SAFETY: `out` is a freshly allocated Vec with capacity `s.len()`, and
    // `bytes` points to the input slice from a different allocation. We copy
    // exactly the unchanged prefix `[0..i]` before any mutation, so the source
    // and destination do not overlap. The copied prefix length is at most the
    // input length, so it fits in the reserved capacity.
    unsafe {
        out.as_mut_ptr().copy_from_nonoverlapping(bytes.as_ptr(), i);
        out.set_len(i);
    }

    let mut bytes = bytes[i..].iter();
    while let Some(&byte) = bytes.next() {
        if byte == b'%' && bytes.len() >= 2 {
            // SAFETY: We only enter this branch when the current byte is `%`
            // and at least two bytes remain in the iterator. Advancing the
            // iterator twice here is therefore valid, and `unwrap_unchecked()`
            // avoids the redundant panic path.
            let a = *unsafe { bytes.next().unwrap_unchecked() };
            let b = *unsafe { bytes.next().unwrap_unchecked() };
            match (a, b) {
                (b'0', b'A') => {
                    // SAFETY: This replaces the three-byte escape sequence
                    // `%0A` with a single decoded byte, so the output is
                    // strictly shorter than the consumed input and cannot
                    // overflow the preallocated buffer.
                    unsafe {
                        write_byte_unchecked!(out, b'\n');
                    }
                }
                (b'2', b'5') => {
                    // SAFETY: This replaces the three-byte escape sequence
                    // `%25` with a single decoded byte, so the output is
                    // strictly shorter than the consumed input and cannot
                    // overflow the preallocated buffer.
                    unsafe {
                        write_byte_unchecked!(out, b'%');
                    }
                }
                (a, b) => {
                    // SAFETY: This replaces a three-byte `%XX` sequence with
                    // the same three output bytes, so the total output length
                    // never exceeds the input length.
                    unsafe {
                        write_byte_unchecked!(out, b'%', a, b);
                    }
                }
            }
        } else {
            // SAFETY: This branch only writes one byte for an input byte that
            // is copied verbatim. Therefore the current length is always
            // strictly less than the reserved capacity here.
            unsafe {
                write_byte_unchecked!(out, byte);
            }
        }
    }

    // SAFETY: Because we copy the input string and only transform ASCII codes
    // into more ASCII codes, it's guaranteed to be UTF-8
    let str = unsafe { String::from_utf8_unchecked(out) };
    Cow::Owned(str)
}

pub fn encode_string(input: &str) -> Cow<'_, str> {
    let mut i = 0;
    let bytes = input.as_bytes();

    'noop: {
        while i < bytes.len() {
            if matches!(bytes[i], b'%' | b'\n') {
                break 'noop;
            }

            i += 1;
        }

        // fast path: nothing to decode
        return Cow::Borrowed(input);
    }

    // Allocate a vec to skip all the Unicode machinery of Rust which we don't
    // need.
    // IMPORTANT FOR SAFETY: Make `out` at least as large as the input string,
    // so we have enough room to copy bytes.
    let mut out = Vec::<u8>::with_capacity(input.len());

    // SAFETY: `out` is a freshly allocated Vec with capacity `s.len()`, and
    // `bytes` points to the input slice from a different allocation. We copy
    // exactly the unchanged prefix `[0..i]` before any mutation, so the source
    // and destination do not overlap. The copied prefix length is at most the
    // input length, so it fits in the reserved capacity.
    unsafe {
        out.as_mut_ptr().copy_from_nonoverlapping(bytes.as_ptr(), i);
        out.set_len(i);
    }

    for &byte in &bytes[i..] {
        match byte {
            b'%' => out.extend_from_slice(b"%25"),
            b'\n' => out.extend_from_slice(b"%0A"),
            _ => out.push(byte),
        }
    }

    // SAFETY: Because we copy the input string and only transform ASCII codes
    // into more ASCII codes, it's guaranteed to be UTF-8
    let str = unsafe { String::from_utf8_unchecked(out) };
    Cow::from(str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_decode() {
        assert_eq!(decode_string("simple string"), "simple string");
        assert_eq!(
            decode_string("not simple string %25 %0A"),
            "not simple string % \n"
        );
    }

    #[test]
    fn string_decode_unicode() {
        // ensure the function doesn't split/affect code-point bytes
        assert_eq!(decode_string("simple 😺 string"), "simple 😺 string");
        assert_eq!(
            decode_string("not 😺 simple 😺 string %25 %0A"),
            "not 😺 simple 😺 string % \n"
        );
    }

    #[test]
    fn string_encode() {
        assert_eq!(encode_string("simple string"), "simple string");
        assert_eq!(
            encode_string("not simple string % \n"),
            "not simple string %25 %0A"
        );
    }

    #[test]
    fn string_encode_unicode() {
        // ensure the function doesn't split/affect code-point bytes
        assert_eq!(encode_string("simple 😺 string"), "simple 😺 string");
        assert_eq!(
            encode_string("not 😺 simple 😺 string % \n"),
            "not 😺 simple 😺 string %25 %0A"
        );
    }
}
