//! KCDB fixed-field database container. Header: "KCDB", packed length,
//! expanded length; body: field count, 24-byte descriptors, row count/stride,
//! then fixed-stride rows. Field kinds remain raw DWORDs.
use thiserror::Error;
use crate::util::{MAX_BUFFER, MAX_ITEMS, inflate_exact, range, u32_at};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseField {
    pub name: Vec<u8>,
    pub kind: u32,
    pub length: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseFile {
    pub fields: Vec<DatabaseField>,
    pub row_stride: u32,
    rows: Vec<u8>,
    count: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DatabaseError {
    #[error("invalid KCDB signature")]
    InvalidSignature,
    #[error("KCDB container or field is truncated")]
    Truncated,
    #[error("KCDB database exceeds its storage limit")]
    TooLarge,
    #[error("KCDB compression is invalid")]
    InvalidCompression,
    #[error("KCDB fixed-field layout is invalid")]
    InvalidLayout,
}

impl DatabaseFile {
    pub fn parse(data: &[u8]) -> Result<Self, DatabaseError> {
        if data.len() > MAX_BUFFER { return Err(DatabaseError::TooLarge); }
        if data.get(..4) != Some(b"KCDB") { return Err(DatabaseError::InvalidSignature); }
        let packed = u32_at(data, 4).ok_or(DatabaseError::Truncated)? as usize;
        let expanded = u32_at(data, 8).ok_or(DatabaseError::Truncated)? as usize;
        if expanded > MAX_BUFFER { return Err(DatabaseError::TooLarge); }
        let compressed = range(data, 12, packed).ok_or(DatabaseError::Truncated)?;
        let body = inflate_exact(compressed, expanded).ok_or(DatabaseError::InvalidCompression)?;
        let count = u32_at(&body, 0).ok_or(DatabaseError::Truncated)? as usize;
        if count > MAX_ITEMS { return Err(DatabaseError::TooLarge); }
        let mut cursor = 4usize;
        let mut field_end = 0u32;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            let field = range(&body, cursor, 24).ok_or(DatabaseError::Truncated)?;
            let end = field[..16].iter().position(|&byte| byte == 0)
                .ok_or(DatabaseError::InvalidLayout)?;
            let length = u32_at(field, 20).unwrap();
            fields.push(DatabaseField { name: field[..end].to_vec(),
                kind: u32_at(field, 16).unwrap(), length, offset: field_end });
            field_end = field_end.checked_add(length).ok_or(DatabaseError::TooLarge)?;
            cursor += 24;
        }
        let count = u32_at(&body, cursor).ok_or(DatabaseError::Truncated)? as usize;
        let row_stride = u32_at(&body, cursor + 4).ok_or(DatabaseError::Truncated)?;
        cursor += 8;
        if count > MAX_ITEMS { return Err(DatabaseError::TooLarge); }
        if field_end > row_stride { return Err(DatabaseError::InvalidLayout); }
        let length = count.checked_mul(row_stride as usize)
            .filter(|&length| length <= MAX_BUFFER).ok_or(DatabaseError::TooLarge)?;
        let rows = range(&body, cursor, length).ok_or(DatabaseError::Truncated)?.to_vec();
        Ok(Self { fields, row_stride, rows, count })
    }

    pub fn row_count(&self) -> usize { self.count }

    /// Original lookup uses the first exact, case-sensitive field name.
    pub fn field(&self, row: usize, name: &[u8]) -> Option<&[u8]> {
        if row >= self.count { return None; }
        let field = self.fields.iter().find(|field| field.name == name)?;
        let offset = row.checked_mul(self.row_stride as usize)?.checked_add(field.offset as usize)?;
        self.rows.get(offset..offset.checked_add(field.length as usize)?)
    }
}
