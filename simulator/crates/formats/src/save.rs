use crate::util::{MAX_BUFFER, MAX_ITEMS, u32_at};
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use std::io::Write;
use thiserror::Error;

const CSS_HEADER_SIZE: usize = 16;
const CSS_BLOCK_SIZE: usize = 0x80000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssRecord {
    pub id: u32,
    pub data: Vec<u8>,
}

/// The original loader does not use zlib's result as its success condition.
/// Preserve that known contract while making damaged/partial input visible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssInflationStatus {
    pub input_consumed: u32,
    pub output_written: u32,
    pub stream_end: bool,
    pub output_truncated: bool,
    pub limit_reached: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CssFile {
    /// Raw declared DWORD. Removal may wrap this independently of storage.
    pub used_length: u32,
    pub tag: u32,
    /// Compatibility view of complete records in logical capacity, excluding
    /// an all-zero allocation tail. Modify through append/remove/clear.
    /// The owned storage, rather than this view, is the container's source.
    pub records: Vec<CssRecord>,
    logical_capacity: u32,
    storage: Vec<u8>,
    inflation: Option<CssInflationStatus>,
}

impl PartialEq for CssFile {
    fn eq(&self, other: &Self) -> bool {
        // Inflation provenance is diagnostic, not container state.
        self.used_length == other.used_length && self.tag == other.tag
            && self.logical_capacity == other.logical_capacity && self.storage == other.storage
    }
}

impl Eq for CssFile {}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CssError {
    #[error("invalid CSS signature")]
    InvalidSignature,
    #[error("CSS file is truncated")]
    Truncated,
    #[error("CSS declared size exceeds the format reader limit")]
    TooLarge,
    #[error("CSS zlib stream is invalid")]
    InvalidCompression,
    #[error("CSS record is malformed")]
    InvalidRecord,
    #[error("CSS allocation failed")]
    AllocationFailed,
    #[error("CSS declared used length exceeds its allocated storage")]
    UsedOutOfBounds,
}

impl CssFile {
    pub fn new(tag: u32) -> Self {
        Self {
            used_length: 0,
            tag,
            records: Vec::new(),
            logical_capacity: CSS_BLOCK_SIZE as u32,
            storage: vec![0; CSS_BLOCK_SIZE],
            inflation: None,
        }
    }

    /// Signature-only optional recognition, matching the container loader.
    pub fn from_css(data: &[u8]) -> Result<Option<Self>, CssError> {
        if data.get(..4) != Some(b"CSS\0") { return Ok(None); }
        Self::parse(data).map(Some)
    }

    pub fn parse(data: &[u8]) -> Result<Self, CssError> {
        if data.len() > MAX_BUFFER {
            return Err(CssError::TooLarge);
        }
        if data.get(..4) != Some(b"CSS\0") {
            return Err(CssError::InvalidSignature);
        }
        if data.len() < CSS_HEADER_SIZE {
            return Err(CssError::Truncated);
        }
        let compressed_length = u32_at(data, 4).ok_or(CssError::Truncated)? as usize;
        let used_length = u32_at(data, 8).ok_or(CssError::Truncated)?;
        let tag = u32_at(data, 12).ok_or(CssError::Truncated)?;
        let capacity = (used_length as usize / CSS_BLOCK_SIZE + 1)
            .checked_mul(CSS_BLOCK_SIZE)
            .ok_or(CssError::TooLarge)?;
        if capacity > MAX_BUFFER {
            return Err(CssError::TooLarge);
        }
        let end = CSS_HEADER_SIZE
            .checked_add(compressed_length)
            .ok_or(CssError::TooLarge)?;
        if end > data.len() {
            return Err(CssError::Truncated);
        }
        // File bytes after the declared compressed range are not consumed.
        // Decode into capacity, keep bytes written before a checksum/data
        // error, and do not replace the declared used length with total_out.
        let mut storage = zero_storage(capacity)?;
        let inflation = inflate_container(&data[CSS_HEADER_SIZE..end], &mut storage);
        let records = record_view(&storage, capacity)?;
        Ok(Self {
            used_length,
            tag,
            records,
            logical_capacity: capacity as u32,
            storage,
            inflation: Some(inflation),
        })
    }

    pub fn capacity(&self) -> u32 { self.logical_capacity }

    pub fn used_len(&self) -> u32 { self.used_length }

    pub fn tag(&self) -> u32 { self.tag }

    /// Complete retained allocation. Removal shrinks logical capacity but
    /// leaves this allocation and its inaccessible tail intact.
    pub fn storage(&self) -> &[u8] { &self.storage }

    pub fn used_bytes(&self) -> Result<&[u8], CssError> {
        self.storage.get(..self.used_length as usize).ok_or(CssError::UsedOutOfBounds)
    }

    pub fn inflation_status(&self) -> Option<&CssInflationStatus> { self.inflation.as_ref() }

    /// Restore an application's complete object save, preserving the backing
    /// allocation separately from logical capacity and raw declared usage.
    /// This is deliberately distinct from native nested-container snapshots,
    /// whose unwritten capacity tail is reconstructed as zero.
    ///
    /// `used_length` may be greater than capacity/allocation after native
    /// wrapping removal. Preserve that state: used-byte/file operations must
    /// keep returning their original checked diagnostic until clear/reset.
    pub fn from_storage_state(
        tag: u32, logical_capacity: u32, used_length: u32, storage: Vec<u8>,
    ) -> Result<Self, CssError> {
        if storage.len() > MAX_BUFFER || logical_capacity as usize > MAX_BUFFER {
            return Err(CssError::TooLarge);
        }
        if logical_capacity as usize > storage.len() {
            return Err(CssError::Truncated);
        }
        let records = record_view(&storage, logical_capacity as usize)?;
        Ok(Self { used_length, tag, records, logical_capacity, storage, inflation: None })
    }

    /// Native 390 allocates and copies logical capacity only. `Clone` instead
    /// preserves the complete backing allocation for internal transactions.
    pub fn try_clone_container(&self) -> Result<Self, CssError> {
        let capacity = self.logical_capacity as usize;
        let mut storage = zero_storage(capacity)?;
        storage.copy_from_slice(self.storage.get(..capacity).ok_or(CssError::InvalidRecord)?);
        let records = record_view(&storage, capacity)?;
        Ok(Self {
            used_length: self.used_length,
            tag: self.tag,
            records,
            logical_capacity: self.logical_capacity,
            storage,
            inflation: None,
        })
    }

    pub fn clear(&mut self) {
        self.storage = vec![0; CSS_BLOCK_SIZE];
        self.logical_capacity = CSS_BLOCK_SIZE as u32;
        self.used_length = 0;
        self.records.clear();
        self.inflation = None;
    }

    pub fn record(&self, id: u32) -> Option<&[u8]> {
        // All constructors and mutations validate the private scan range.
        // Public compatibility fields cannot mutate that private storage.
        self.try_record(id).expect("CSS private storage was validated")
    }

    pub fn try_record(&self, id: u32) -> Result<Option<&[u8]>, CssError> {
        Ok(find_record(&self.storage, self.logical_capacity as usize, id)?
            .map(|(_, body, end)| &self.storage[body..end]))
    }

    pub fn append(&mut self, id: u32, data: &[u8]) -> Result<(), CssError> {
        let length = u32::try_from(data.len()).map_err(|_| CssError::TooLarge)?;
        if data.len() > MAX_BUFFER { return Err(CssError::TooLarge); }
        let mut candidate = self.clone();
        let mut header = [0; 8];
        header[..4].copy_from_slice(&id.to_le_bytes());
        header[4..].copy_from_slice(&length.to_le_bytes());
        // 0x6eac10 invokes the grow/write helper twice. Its block arithmetic
        // depends on used-length transitions, not round_up(new used).
        candidate.append_bytes(&header)?;
        candidate.append_bytes(data)?;
        candidate.records = record_view(&candidate.storage, candidate.logical_capacity as usize)?;
        candidate.inflation = None;
        *self = candidate;
        Ok(())
    }

    pub fn remove(&mut self, id: u32) -> bool {
        self.try_remove(id).expect("CSS private storage was validated")
    }

    pub fn try_remove(&mut self, id: u32) -> Result<bool, CssError> {
        let capacity = self.logical_capacity as usize;
        let Some((start, _, end)) = find_record(&self.storage, capacity, id)? else { return Ok(false); };
        if end >= capacity { return Ok(false); }
        let length = end - start;
        let mut candidate = self.clone();
        candidate.storage.copy_within(end..capacity, start);
        candidate.logical_capacity -= length as u32;
        candidate.used_length = candidate.used_length.wrapping_sub(length as u32);
        candidate.records = record_view(&candidate.storage, candidate.logical_capacity as usize)?;
        candidate.inflation = None;
        *self = candidate;
        Ok(true)
    }

    pub fn to_css(&self) -> Result<Vec<u8>, CssError> {
        if self.used_length > self.logical_capacity { return Err(CssError::UsedOutOfBounds); }
        let body = self.used_bytes()?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(body).map_err(|_| CssError::TooLarge)?;
        let compressed = encoder.finish().map_err(|_| CssError::TooLarge)?;
        let compressed_length = u32::try_from(compressed.len()).map_err(|_| CssError::TooLarge)?;
        if CSS_HEADER_SIZE.checked_add(compressed.len()).is_none_or(|length| length > MAX_BUFFER) {
            return Err(CssError::TooLarge);
        }
        let mut output = Vec::with_capacity(16 + compressed.len());
        output.extend_from_slice(b"CSS\0");
        output.extend_from_slice(&compressed_length.to_le_bytes());
        output.extend_from_slice(&self.used_length.to_le_bytes());
        output.extend_from_slice(&self.tag.to_le_bytes());
        output.extend_from_slice(&compressed);
        Ok(output)
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, CssError> {
        if self.used_length > self.logical_capacity { return Err(CssError::UsedOutOfBounds); }
        let body = self.used_bytes()?;
        if CSS_HEADER_SIZE.checked_add(body.len()).is_none_or(|length| length > MAX_BUFFER) {
            return Err(CssError::TooLarge);
        }
        let mut output = Vec::with_capacity(16 + body.len());
        output.extend_from_slice(&0u32.to_le_bytes());
        output.extend_from_slice(&self.logical_capacity.to_le_bytes());
        output.extend_from_slice(&self.used_length.to_le_bytes());
        output.extend_from_slice(&self.tag.to_le_bytes());
        output.extend_from_slice(body);
        Ok(output)
    }

    pub fn from_snapshot(data: &[u8]) -> Result<Self, CssError> {
        if data.len() < 16 {
            return Err(CssError::Truncated);
        }
        let capacity = u32_at(data, 4).ok_or(CssError::Truncated)? as usize;
        let used_length = u32_at(data, 8).ok_or(CssError::Truncated)?;
        let tag = u32_at(data, 12).ok_or(CssError::Truncated)?;
        let used = used_length as usize;
        if capacity > MAX_BUFFER { return Err(CssError::TooLarge); }
        if used > capacity { return Err(CssError::UsedOutOfBounds); }
        let end = CSS_HEADER_SIZE.checked_add(used).ok_or(CssError::TooLarge)?;
        if data.len() < end { return Err(CssError::Truncated); }
        let mut storage = zero_storage(capacity)?;
        // The original restore copies exactly used bytes, ignoring any
        // containing-record suffix as well as the old pointer DWORD.
        storage[..used].copy_from_slice(&data[CSS_HEADER_SIZE..end]);
        let records = record_view(&storage, capacity)?;
        Ok(Self {
            used_length,
            tag,
            records,
            logical_capacity: capacity as u32,
            storage,
            inflation: None,
        })
    }

    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), CssError> {
        let used = self.used_length as usize;
        if used > self.storage.len() { return Err(CssError::UsedOutOfBounds); }
        let new_used = used.checked_add(bytes.len()).filter(|&used| used <= MAX_BUFFER)
            .ok_or(CssError::TooLarge)?;
        let crossed = new_used / CSS_BLOCK_SIZE - used / CSS_BLOCK_SIZE;
        if crossed != 0 {
            let capacity = (self.logical_capacity as usize).checked_add(
                crossed.checked_mul(CSS_BLOCK_SIZE).ok_or(CssError::TooLarge)?
            ).filter(|&capacity| capacity <= MAX_BUFFER).ok_or(CssError::TooLarge)?;
            let mut replacement = zero_storage(capacity)?;
            // Reallocation copies logical capacity, not retained allocation.
            let previous = self.logical_capacity as usize;
            replacement[..previous].copy_from_slice(&self.storage[..previous]);
            self.storage = replacement;
            self.logical_capacity = capacity as u32;
        }
        self.storage.get_mut(used..new_used).ok_or(CssError::UsedOutOfBounds)?
            .copy_from_slice(bytes);
        self.used_length = new_used as u32;
        Ok(())
    }
}

fn zero_storage(capacity: usize) -> Result<Vec<u8>, CssError> {
    if capacity > MAX_BUFFER { return Err(CssError::TooLarge); }
    let mut storage = Vec::new();
    storage.try_reserve_exact(capacity).map_err(|_| CssError::AllocationFailed)?;
    storage.resize(capacity, 0);
    Ok(storage)
}

fn inflate_container(input: &[u8], storage: &mut [u8]) -> CssInflationStatus {
    let mut decoder = Decompress::new(true);
    let mut result = CssInflationStatus::default();
    let mut scratch = [0u8; 0x1000];
    while (decoder.total_in() as usize) < input.len() {
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let status = decoder.decompress(
            &input[before_in as usize..], &mut scratch, FlushDecompress::None,
        );
        let produced = (decoder.total_out() - before_out) as usize;
        let written = result.output_written as usize;
        let length = produced.min(storage.len() - written);
        storage[written..written + length].copy_from_slice(&scratch[..length]);
        result.output_written += length as u32;
        result.input_consumed = decoder.total_in() as u32;
        result.output_truncated |= length != produced;
        // The original helper continues consuming input after capacity is
        // full. Keep that behavior for bounded streams, with an explicit
        // diagnostic when discarded expansion would exceed the reader limit.
        if decoder.total_out() > MAX_BUFFER as u64 {
            result.limit_reached = true;
            break;
        }
        match status {
            Ok(Status::StreamEnd) => { result.stream_end = true; break; }
            Err(error) => { result.error = Some(error.to_string()); break; }
            _ => {}
        }
        if produced == 0 || (decoder.total_in() == before_in && decoder.total_out() == before_out) { break; }
    }
    result
}

fn record_end(storage: &[u8], offset: usize) -> Result<usize, CssError> {
    let length = u32_at(storage, offset + 4).ok_or(CssError::InvalidRecord)? as usize;
    offset.checked_add(8).and_then(|body| body.checked_add(length))
        .filter(|&end| end <= storage.len()).ok_or(CssError::InvalidRecord)
}

fn find_record(storage: &[u8], capacity: usize, id: u32) -> Result<Option<(usize, usize, usize)>, CssError> {
    if capacity > storage.len() { return Err(CssError::InvalidRecord); }
    let zero_tail = zero_tail_start(&storage[..capacity]);
    let mut offset = 0;
    for _ in 0..=MAX_ITEMS {
        // Safe boundary for a partial header at the logical allocation end.
        if capacity.saturating_sub(offset) < 8 { return Ok(None); }
        let found = u32_at(storage, offset).ok_or(CssError::InvalidRecord)?;
        let end = record_end(storage, offset)?;
        if found == id { return Ok(Some((offset, offset + 8, end))); }
        if found == 0 && end == offset + 8 && offset >= zero_tail {
            return Ok(None);
        }
        offset = end;
    }
    Err(CssError::TooLarge)
}

fn record_view(data: &[u8], capacity: usize) -> Result<Vec<CssRecord>, CssError> {
    if capacity > data.len() { return Err(CssError::InvalidRecord); }
    let zero_tail = zero_tail_start(&data[..capacity]);
    let mut records = Vec::new();
    let mut offset = 0usize;
    while capacity.saturating_sub(offset) >= 8 {
        if records.len() >= MAX_ITEMS { return Err(CssError::TooLarge); }
        let id = u32_at(data, offset).ok_or(CssError::InvalidRecord)?;
        let end = record_end(data, offset)?;
        // Deterministic zero fill makes ID zero queryable at the first empty
        // tail header. Avoid materializing millions of synthetic view rows.
        if id == 0 && end == offset + 8 && offset >= zero_tail {
            break;
        }
        records.push(CssRecord {
            id,
            data: data[offset + 8..end].to_vec(),
        });
        offset = end;
    }
    Ok(records)
}

fn zero_tail_start(data: &[u8]) -> usize {
    data.iter().rposition(|&byte| byte != 0).map_or(0, |offset| offset + 1)
}
