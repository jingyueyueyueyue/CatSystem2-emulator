//! Independently implemented OAuth 1.0a wire encoding (RFC 5849).
//! No reference engine or third-party OAuth implementation is used.
use anyhow::{Result, bail};

pub(super) fn escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(char::from(b)); }
        else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

pub(super) fn form(fields: &[(String, String)]) -> String {
    fields.iter().map(|(k, v)| format!("{}={}", escape(k.as_bytes()), escape(v.as_bytes())))
        .collect::<Vec<_>>().join("&")
}

pub(super) fn unform(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    fn decode(bytes: &[u8]) -> Result<String> {
        let mut out = Vec::new(); let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' => {
                    let pair = bytes.get(i + 1..i + 3).ok_or_else(|| anyhow::anyhow!("truncated OAuth form escape"))?;
                    let pair = std::str::from_utf8(pair)?;
                    out.push(u8::from_str_radix(pair, 16)?); i += 2;
                }
                c => out.push(c),
            }
            i += 1;
        }
        Ok(String::from_utf8(out)?)
    }
    bytes.split(|&b| b == b'&').filter(|part| !part.is_empty()).map(|part| {
        let i = part.iter().position(|&b| b == b'=').ok_or_else(|| anyhow::anyhow!("OAuth form has no value"))?;
        Ok((decode(&part[..i])?, decode(&part[i + 1..])?))
    }).collect()
}

pub(super) fn value<'a>(fields: &'a [(String, String)], key: &str) -> Result<&'a str> {
    let mut values = fields.iter().filter(|(k, _)| k == key);
    let (_, value) = values.next().ok_or_else(|| anyhow::anyhow!("OAuth response lacks {key}"))?;
    if value.is_empty() || values.next().is_some() { bail!("OAuth response has an invalid {key}"); }
    Ok(value)
}

pub(super) struct Credentials {
    pub key: String, pub secret: String, pub token: String, pub token_secret: String,
}

impl Credentials {
    pub fn header(&self, method: &str, url: &str, body: &[(String, String)],
        extra: &[(String, String)], nonce: &str, timestamp: u64) -> String {
        let mut oauth = vec![
            ("oauth_consumer_key".into(), self.key.clone()),
            ("oauth_nonce".into(), nonce.to_owned()),
            ("oauth_signature_method".into(), "HMAC-SHA1".into()),
            ("oauth_timestamp".into(), timestamp.to_string()),
            ("oauth_version".into(), "1.0".into()),
        ];
        if !self.token.is_empty() { oauth.push(("oauth_token".into(), self.token.clone())); }
        oauth.extend_from_slice(extra);
        let mut parameters = oauth.iter().chain(body.iter())
            .map(|(k, v)| (escape(k.as_bytes()), escape(v.as_bytes()))).collect::<Vec<_>>();
        parameters.sort();
        let normalized = parameters.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let base = format!("{}&{}&{}", method, escape(url.as_bytes()), escape(normalized.as_bytes()));
        let key = format!("{}&{}", escape(self.secret.as_bytes()), escape(self.token_secret.as_bytes()));
        oauth.push(("oauth_signature".into(), base64(&hmac(key.as_bytes(), base.as_bytes()))));
        oauth.sort();
        format!("OAuth {}", oauth.iter().map(|(k, v)| format!("{}=\"{}\"", escape(k.as_bytes()), escape(v.as_bytes())))
            .collect::<Vec<_>>().join(", "))
    }
}

pub(super) fn digest(bytes: &[u8]) -> [u8; 20] {
    let mut input = bytes.to_vec();
    input.push(0x80);
    while input.len() % 64 != 56 { input.push(0); }
    input.extend_from_slice(&(bytes.len() as u64).wrapping_mul(8).to_be_bytes());
    let mut h = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    for block in input.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, chunk) in block.chunks_exact(4).enumerate() { w[i] = u32::from_be_bytes(chunk.try_into().unwrap()); }
        for i in 16..80 { w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1); }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.into_iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let next = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(word);
            e = d; d = c; c = b.rotate_left(30); b = a; a = next;
        }
        for (value, add) in h.iter_mut().zip([a,b,c,d,e]) { *value = value.wrapping_add(add); }
    }
    let mut out = [0; 20];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) { chunk.copy_from_slice(&word.to_be_bytes()); }
    out
}

fn hmac(key: &[u8], bytes: &[u8]) -> [u8; 20] {
    let mut block = [0; 64];
    if key.len() > 64 { block[..20].copy_from_slice(&digest(key)); }
    else { block[..key.len()].copy_from_slice(key); }
    let mut inner = block.map(|b| b ^ 0x36).to_vec(); inner.extend_from_slice(bytes);
    let mut outer = block.map(|b| b ^ 0x5c).to_vec(); outer.extend_from_slice(&digest(&inner));
    digest(&outer)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for b in bytes.chunks(3) {
        let word = (u32::from(b[0]) << 16) | (u32::from(*b.get(1).unwrap_or(&0)) << 8) | u32::from(*b.get(2).unwrap_or(&0));
        for i in 0..4 { out.push(if i > b.len() { '=' } else { char::from(TABLE[((word >> (18-i*6)) & 63) as usize]) }); }
    }
    out
}

/// Parse the numeric/string ID field returned by Twitter media/upload. This
/// intentionally does not accept a substring inside a quoted JSON string.
pub(super) fn media_id(bytes: &[u8]) -> Result<String> {
    api_reply(bytes)?;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' { i += 1; continue; }
        i += 1; let start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' { i += 1; }
            i += 1;
        }
        let key = bytes.get(start..i).ok_or_else(|| anyhow::anyhow!("truncated media JSON"))?;
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) { i += 1; }
        if bytes.get(i) != Some(&b':') { continue; }
        i += 1;
        if key != b"media_id_string" { continue; }
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) { i += 1; }
        if bytes.get(i) != Some(&b'"') { bail!("media_id_string is not a JSON string"); }
        i += 1; let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) { i += 1; }
        if i == start || i - start > 32 || bytes.get(i) != Some(&b'"') { bail!("invalid media ID"); }
        return Ok(std::str::from_utf8(&bytes[start..i])?.to_owned());
    }
    bail!("Twitter media upload did not return media_id_string")
}

/// Validate the complete UTF8 JSON response and the actual root error fields.
/// A 2xx HTTP receipt containing Twitter's `errors` is still a failed call.
pub(super) fn api_reply(bytes: &[u8]) -> Result<()> {
    std::str::from_utf8(bytes)?;
    struct Cursor<'a> { b: &'a [u8], i: usize }
    impl Cursor<'_> {
        fn space(&mut self) { while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) { self.i += 1; } }
        fn string(&mut self) -> Result<&[u8]> {
            if self.b.get(self.i) != Some(&b'"') { bail!("expected JSON string"); }
            self.i += 1; let start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c == b'"' { let end = self.i; self.i += 1; return Ok(&self.b[start..end]); }
                if c < 32 { bail!("control byte in JSON string"); }
                if c == b'\\' {
                    self.i += 1;
                    match self.b.get(self.i) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {},
                        Some(b'u') => for _ in 0..4 {
                            self.i += 1;
                            if !self.b.get(self.i).is_some_and(u8::is_ascii_hexdigit) { bail!("invalid JSON Unicode escape"); }
                        },
                        _ => bail!("invalid JSON escape"),
                    }
                }
                self.i += 1;
            }
            bail!("unterminated JSON string")
        }
        fn value(&mut self, depth: usize) -> Result<()> {
            if depth > 32 { bail!("Tweet JSON nesting exceeds32"); }
            self.space();
            match self.b.get(self.i).copied() {
                Some(b'"') => { self.string()?; }
                Some(open @ (b'{' | b'[')) => {
                    self.i += 1; self.space(); let close = if open == b'{' { b'}' } else { b']' };
                    if self.b.get(self.i) == Some(&close) { self.i += 1; return Ok(()); }
                    loop {
                        if open == b'{' {
                            self.string()?; self.space();
                            if self.b.get(self.i) != Some(&b':') { bail!("JSON object lacks colon"); } self.i += 1;
                        }
                        self.value(depth + 1)?; self.space();
                        if self.b.get(self.i) == Some(&close) { self.i += 1; break; }
                        if self.b.get(self.i) != Some(&b',') { bail!("JSON container lacks separator"); }
                        self.i += 1; self.space();
                    }
                }
                Some(b't' | b'f' | b'n') => {
                    let text: &[u8] = match self.b[self.i] { b't' => b"true", b'f' => b"false", _ => b"null" };
                    if !self.b[self.i..].starts_with(text) { bail!("invalid JSON literal"); } self.i += text.len();
                }
                Some(b'-' | b'0'..=b'9') => {
                    if self.b.get(self.i) == Some(&b'-') { self.i += 1; }
                    if self.b.get(self.i) == Some(&b'0') { self.i += 1; }
                    else {
                        let start = self.i;
                        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) { self.i += 1; }
                        if self.i == start { bail!("invalid JSON integer"); }
                    }
                    if self.b.get(self.i) == Some(&b'.') {
                        self.i += 1; let start = self.i;
                        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) { self.i += 1; }
                        if self.i == start { bail!("invalid JSON fraction"); }
                    }
                    if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
                        self.i += 1;
                        if matches!(self.b.get(self.i), Some(b'+' | b'-')) { self.i += 1; }
                        let start = self.i;
                        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) { self.i += 1; }
                        if self.i == start { bail!("invalid JSON exponent"); }
                    }
                }
                _ => bail!("invalid JSON value"),
            }
            Ok(())
        }
    }
    let mut c = Cursor { b: bytes, i: 0 }; c.space();
    if c.b.get(c.i) != Some(&b'{') { bail!("Twitter API response is not a JSON object"); }
    c.i += 1; c.space(); let mut error = false;
    if c.b.get(c.i) != Some(&b'}') {
        loop {
            let key = c.string()?.to_vec(); c.space();
            if c.b.get(c.i) != Some(&b':') { bail!("Twitter JSON member lacks colon"); }
            c.i += 1; c.space(); let start = c.i;
            c.value(1)?; let end = c.i; c.space();
            if matches!(key.as_slice(), b"error" | b"errors") {
                let mut value = c.b[start..end].iter().copied().filter(|b| !b.is_ascii_whitespace()).collect::<Vec<_>>();
                if !matches!(value.as_slice(), b"null" | b"[]" | b"\"\"") { error = true; }
                value.clear();
            }
            if c.b.get(c.i) == Some(&b'}') { break; }
            if c.b.get(c.i) != Some(&b',') { bail!("Twitter JSON member lacks separator"); } c.i += 1; c.space();
        }
    }
    c.i += 1; c.space();
    if c.i != bytes.len() { bail!("trailing Twitter JSON data"); }
    if error { bail!("Twitter API returned an error object"); }
    Ok(())
}
