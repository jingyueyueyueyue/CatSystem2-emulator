use flate2::{Decompress, FlushDecompress, Status};

pub(crate) const MAX_BUFFER: usize = 64 * 1024 * 1024;
pub(crate) const MAX_ITEMS: usize = 1_000_000;

pub(crate) fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

pub(crate) fn range(data: &[u8], offset: usize, length: usize) -> Option<&[u8]> {
    data.get(offset..offset.checked_add(length)?)
}

pub(crate) fn c_string(data: &[u8], offset: usize) -> Option<&[u8]> {
    let rest = data.get(offset..)?;
    let end = rest.iter().position(|&byte| byte == 0)?;
    Some(&rest[..end])
}

pub(crate) fn inflate_exact(data: &[u8], expected: usize) -> Option<Vec<u8>> {
    let output = inflate_at_most(data, expected)?;
    (output.len() == expected).then_some(output)
}

/// Inflates a complete zlib stream whose output must not exceed `limit`.
///
/// Unlike [`inflate_exact`], this preserves the actual output length. This is
/// required for old CST files whose header records a larger scratch-buffer
/// length than the stream actually produces.
pub(crate) fn inflate_at_most(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    if limit > MAX_BUFFER {
        return None;
    }

    let capacity = limit.checked_add(1)?;
    let mut output = vec![0; capacity];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(data, &mut output, FlushDecompress::Finish)
        .ok()?;

    if status != Status::StreamEnd || decoder.total_in() as usize != data.len() {
        return None;
    }

    let actual = decoder.total_out() as usize;
    if actual > limit {
        return None;
    }
    output.truncate(actual);
    Some(output)
}

/// Reads the evidenced CST variant missing a final zero Adler32 byte.
///
/// This is deliberately separate from the strict readers used by other
/// formats. The original input must be completely consumed, already emit
/// exactly `expected` bytes, and finish without emitting any more bytes when
/// the single zero is supplied. The zlib decoder still verifies the checksum.
pub(crate) fn inflate_missing_adler32_zero(data: &[u8], expected: usize) -> Option<Vec<u8>> {
    if expected > MAX_BUFFER {
        return None;
    }
    let mut output = vec![0; expected.checked_add(1)?];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(data, &mut output, FlushDecompress::None)
        .ok()?;
    if status == Status::StreamEnd
        || decoder.total_in() as usize != data.len()
        || decoder.total_out() as usize != expected
    {
        return None;
    }
    let status = decoder
        .decompress(&[0], &mut output[expected..], FlushDecompress::Finish)
        .ok()?;
    if status != Status::StreamEnd
        || decoder.total_in() as usize != data.len().checked_add(1)?
        || decoder.total_out() as usize != expected
    {
        return None;
    }
    output.truncate(expected);
    Some(output)
}
