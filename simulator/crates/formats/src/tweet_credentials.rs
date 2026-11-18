//! Yukikoi kcTaskTweet cache: two LE lengths followed by Blowfish-LE blocks.
//! Recovered from immutable PE bytes; uses the existing RustCrypto dependency.
//! This unauthenticated original format cannot detect a wrong machine key.
use blowfish::{BlowfishLE, cipher::{BlockDecrypt, BlockEncrypt, KeyInit}};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TweetCredentialError {
    #[error("invalid Tweet credential header, block alignment or length")]
    InvalidContainer,
    #[error("Tweet credential exceeds the bounded host capacity")]
    TooLarge,
    #[error("Tweet credentials contain an embedded terminator")]
    EmbeddedNul,
}

// Deliberately no Debug: callers must not accidentally log token contents.
pub struct TweetCredentials { pub token: Vec<u8>, pub secret: Vec<u8> }
impl TweetCredentials {
    pub fn parse(bytes: &[u8], key: &[u8; 16]) -> Result<Self, TweetCredentialError> {
        if bytes.len() > 32768 { return Err(TweetCredentialError::TooLarge); }
        if bytes.len() < 8 || (bytes.len()-8) % 8 != 0 { return Err(TweetCredentialError::InvalidContainer); }
        let token = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let secret = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let used = token.checked_add(secret).ok_or(TweetCredentialError::InvalidContainer)?;
        // Original only checks sum < whole file length, which can overread the
        // decrypted payload. Diagnose that case rather than borrowing stack tail.
        if used > bytes.len()-8 { return Err(TweetCredentialError::InvalidContainer); }
        let cipher = BlowfishLE::new_from_slice(key).map_err(|_| TweetCredentialError::InvalidContainer)?;
        let mut clear = bytes[8..].to_vec();
        for block in clear.chunks_exact_mut(8) { cipher.decrypt_block(block.into()); }
        let result = Self { token: clear[..token].to_vec(), secret: clear[token..used].to_vec() };
        clear.fill(0);
        Ok(result)
    }
    /// Encodes the actual header/block layout. Unspecified original heap padding
    /// is safely zeroed; byte equality with those uninitialized bytes is not claimed.
    pub fn encode(&self, key: &[u8; 16]) -> Result<Vec<u8>, TweetCredentialError> {
        if self.token.contains(&0) || self.secret.contains(&0) { return Err(TweetCredentialError::EmbeddedNul); }
        let used = self.token.len().checked_add(self.secret.len()).ok_or(TweetCredentialError::TooLarge)?;
        let size = used.checked_add(7).ok_or(TweetCredentialError::TooLarge)? & !7;
        if size > 32760 { return Err(TweetCredentialError::TooLarge); }
        let cipher = BlowfishLE::new_from_slice(key).map_err(|_| TweetCredentialError::InvalidContainer)?;
        let mut output = Vec::with_capacity(size+8);
        output.extend((self.token.len() as u32).to_le_bytes()); output.extend((self.secret.len() as u32).to_le_bytes());
        output.extend(&self.token); output.extend(&self.secret); output.resize(size+8, 0);
        for block in output[8..].chunks_exact_mut(8) { cipher.encrypt_block(block.into()); }
        Ok(output)
    }
}

/// 58e061: Windows-directory drive's volume serial seeds the legacy MT.
/// Failure to obtain the directory/serial leaves all sixteen bytes zero.
pub fn volume_key(serial: Option<u32>) -> [u8; 16] {
    let mut key = [0; 16];
    if let Some(serial) = serial {
        let mut random = crate::archive::LegacyRandom::seeded(serial);
        for byte in &mut key { *byte = random.next() as u8; }
    }
    key
}

/// 58f140 count, with the already-decoded CP932 body and eight suffix fields.
/// Prefixes intentionally include the preceding ASCII space. A leading URL at
/// offset0 is ordinary text; URLs after a space cost24 including that space.
pub fn count_text(body: &[u8], suffix: &[u8]) -> Result<usize, TweetCredentialError> {
    let mut bytes = body.to_vec(); bytes.extend(suffix);
    let mut i = 0; let mut count = 0;
    while i < bytes.len() && bytes[i] != 0 {
        if matches!(bytes[i], 0x81..=0x9f | 0xe0..=0xfc) {
            if i+1 >= bytes.len() || bytes[i+1] == 0 { return Err(TweetCredentialError::InvalidContainer); }
            i += 2;
        } else if [b" http://".as_slice(), b" https://".as_slice(), b" ftp://".as_slice()]
            .iter().any(|prefix| bytes[i..].starts_with(prefix)) {
            i += 1;
            while i < bytes.len() && bytes[i] != 0 && bytes[i] != b' '
                && !bytes[i..].starts_with(b"\r\n") {
                if matches!(bytes[i], 0x81..=0x9f | 0xe0..=0xfc) {
                    if i+1 >= bytes.len() || bytes[i+1] == 0 { return Err(TweetCredentialError::InvalidContainer); }
                    i += 2;
                } else { i += 1; }
            }
            count += 24; continue;
        } else if bytes[i..].starts_with(b"\r\n") { i += 2; }
        else { i += 1; }
        count += 1;
    }
    Ok(count)
}
