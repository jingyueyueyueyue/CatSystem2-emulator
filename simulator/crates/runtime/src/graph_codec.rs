//! Bounded, fixed-endian serde wire for the versioned host graph. Map keys
//! retain their actual type (including resource-name bytes and tuple keys).
//! Structs use declaration order; changing their layout requires a graph version.
use serde::{Deserialize, Serialize, de, ser};
use std::fmt;

const LIMIT: usize = super::MAX_GRAPH_BYTES;
#[derive(Debug)]
pub(super) struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for Error {}
impl ser::Error for Error { fn custom<T: fmt::Display>(value: T) -> Self { Self(value.to_string()) } }
impl de::Error for Error { fn custom<T: fmt::Display>(value: T) -> Self { Self(value.to_string()) } }
type Result<T> = std::result::Result<T, Error>;
fn bad() -> Error { Error("invalid or excessive host graph wire".into()) }

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut writer = Writer { bytes: Vec::new(), depth: 0 };
    value.serialize(&mut writer)?;
    Ok(writer.bytes)
}
pub(super) fn decode<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > LIMIT { return Err(bad()); }
    let mut reader = Reader { bytes, depth: 0, work: 0 };
    let value = T::deserialize(&mut reader)?;
    if !reader.bytes.is_empty() { return Err(bad()); }
    Ok(value)
}

struct Writer { bytes: Vec<u8>, depth: usize }
impl Writer {
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        if self.bytes.len().checked_add(bytes.len()).is_none_or(|len| len > LIMIT) { return Err(bad()); }
        self.bytes.extend_from_slice(bytes); Ok(())
    }
    fn count(&mut self, len: usize) -> Result<()> {
        if len > LIMIT { return Err(bad()); }
        self.put(&u32::try_from(len).map_err(|_| bad())?.to_le_bytes())
    }
    fn compound(&mut self, len: usize, map: bool) -> Result<Compound<'_>> {
        if self.depth >= 128 { return Err(bad()); }
        self.count(len)?; self.depth += 1;
        Ok(Compound { writer: self, remaining: len.checked_mul(if map { 2 } else { 1 }).ok_or_else(bad)? })
    }
}
struct Compound<'a> { writer: &'a mut Writer, remaining: usize }
impl Compound<'_> {
    fn field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(bad)?;
        value.serialize(&mut *self.writer)
    }
    fn finish(self) -> Result<()> { if self.remaining != 0 { return Err(bad()); } Ok(()) }
}
impl Drop for Compound<'_> { fn drop(&mut self) { self.writer.depth -= 1; } }
macro_rules! write_number {
    ($($name:ident($ty:ty)),* $(,)?) => { $(
        fn $name(self, value: $ty) -> Result<()> { self.put(&value.to_le_bytes()) }
    )* };
}
impl<'a> ser::Serializer for &'a mut Writer {
    type Ok = (); type Error = Error;
    type SerializeSeq = Compound<'a>; type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>; type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>; type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;
    write_number!(serialize_i8(i8), serialize_u8(u8), serialize_i16(i16), serialize_u16(u16),
        serialize_i32(i32), serialize_u32(u32), serialize_i64(i64), serialize_u64(u64),
        serialize_i128(i128), serialize_u128(u128), serialize_f32(f32), serialize_f64(f64));
    fn serialize_bool(self, value: bool) -> Result<()> { self.put(&[u8::from(value)]) }
    fn serialize_char(self, value: char) -> Result<()> { self.put(&(value as u32).to_le_bytes()) }
    fn serialize_str(self, value: &str) -> Result<()> { self.serialize_bytes(value.as_bytes()) }
    fn serialize_bytes(self, value: &[u8]) -> Result<()> { self.count(value.len())?; self.put(value) }
    fn serialize_none(self) -> Result<()> { self.put(&[0]) }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<()> { self.put(&[1])?; value.serialize(self) }
    fn serialize_unit(self) -> Result<()> { Ok(()) }
    fn serialize_unit_struct(self, _: &'static str) -> Result<()> { Ok(()) }
    fn serialize_unit_variant(self, _: &'static str, index: u32, _: &'static str) -> Result<()> { self.serialize_u32(index) }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, value: &T) -> Result<()> { value.serialize(self) }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _: &'static str, index: u32, _: &'static str, value: &T) -> Result<()> {
        self.put(&index.to_le_bytes())?; value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'a>> { self.compound(len.ok_or_else(bad)?, false) }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a>> { self.compound(len, false) }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Compound<'a>> { self.compound(len, false) }
    fn serialize_tuple_variant(self, _: &'static str, index: u32, _: &'static str, len: usize) -> Result<Compound<'a>> {
        self.put(&index.to_le_bytes())?; self.compound(len, false)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Compound<'a>> { self.compound(len.ok_or_else(bad)?, true) }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Compound<'a>> { self.compound(len, false) }
    fn serialize_struct_variant(self, _: &'static str, index: u32, _: &'static str, len: usize) -> Result<Compound<'a>> {
        self.put(&index.to_le_bytes())?; self.compound(len, false)
    }
    fn is_human_readable(&self) -> bool { false }
}
macro_rules! sequence_impl {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = (); type Error = Error;
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> { self.field(value) }
            fn end(self) -> Result<()> { self.finish() }
        }
    };
}
sequence_impl!(SerializeSeq, serialize_element);
sequence_impl!(SerializeTuple, serialize_element);
sequence_impl!(SerializeTupleStruct, serialize_field);
sequence_impl!(SerializeTupleVariant, serialize_field);
impl ser::SerializeMap for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> { self.field(key) }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> { self.field(value) }
    fn end(self) -> Result<()> { self.finish() }
}
impl ser::SerializeStruct for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, _: &'static str, value: &T) -> Result<()> { self.field(value) }
    fn end(self) -> Result<()> { self.finish() }
}
impl ser::SerializeStructVariant for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, _: &'static str, value: &T) -> Result<()> { self.field(value) }
    fn end(self) -> Result<()> { self.finish() }
}

struct Reader<'de> { bytes: &'de [u8], depth: usize, work: usize }
impl<'de> Reader<'de> {
    fn take(&mut self, length: usize) -> Result<&'de [u8]> {
        self.work = self.work.checked_add(1).filter(|&work| work <= LIMIT).ok_or_else(bad)?;
        let (head, tail) = self.bytes.split_at_checked(length).ok_or_else(bad)?;
        self.bytes = tail; Ok(head)
    }
    fn count(&mut self) -> Result<usize> {
        let len = u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| bad())?) as usize;
        if len > LIMIT { return Err(bad()); } Ok(len)
    }
    fn sequence<V: de::Visitor<'de>>(&mut self, visitor: V, expected: Option<usize>, map: bool) -> Result<V::Value> {
        let len = self.count()?;
        if expected.is_some_and(|expected| expected != len) || self.depth >= 128 { return Err(bad()); }
        self.depth += 1;
        let mut access = Access { reader: self, left: len.checked_mul(if map { 2 } else { 1 }).ok_or_else(bad)? };
        let result = if map { visitor.visit_map(&mut access) } else { visitor.visit_seq(&mut access) };
        let left = access.left;
        self.depth -= 1;
        let value = result?;
        if left != 0 { return Err(bad()); } Ok(value)
    }
}
struct Access<'a, 'de> { reader: &'a mut Reader<'de>, left: usize }
impl<'de> de::SeqAccess<'de> for &mut Access<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.left == 0 { return Ok(None); } self.left -= 1;
        seed.deserialize(&mut *self.reader).map(Some)
    }
    // No size_hint: never let an untrusted count preallocate arbitrary memory.
}
impl<'de> de::MapAccess<'de> for &mut Access<'_, 'de> {
    type Error = Error;
    fn next_key_seed<T: de::DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.left == 0 { return Ok(None); }
        if self.left % 2 != 0 { return Err(bad()); } self.left -= 1;
        seed.deserialize(&mut *self.reader).map(Some)
    }
    fn next_value_seed<T: de::DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value> {
        if self.left % 2 != 1 { return Err(bad()); } self.left -= 1;
        seed.deserialize(&mut *self.reader)
    }
}
macro_rules! read_number {
    ($($name:ident($ty:ty, $visit:ident)),* $(,)?) => { $(
        fn $name<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let raw = self.take(std::mem::size_of::<$ty>())?.try_into().map_err(|_| bad())?;
            visitor.$visit(<$ty>::from_le_bytes(raw))
        }
    )* };
}
impl<'de> de::Deserializer<'de> for &mut Reader<'de> {
    type Error = Error;
    read_number!(deserialize_i8(i8, visit_i8), deserialize_u8(u8, visit_u8),
        deserialize_i16(i16, visit_i16), deserialize_u16(u16, visit_u16),
        deserialize_i32(i32, visit_i32), deserialize_u32(u32, visit_u32),
        deserialize_i64(i64, visit_i64), deserialize_u64(u64, visit_u64),
        deserialize_i128(i128, visit_i128), deserialize_u128(u128, visit_u128),
        deserialize_f32(f32, visit_f32), deserialize_f64(f64, visit_f64));
    fn deserialize_any<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> { Err(bad()) }
    fn deserialize_bool<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.take(1)?[0] { 0 => visitor.visit_bool(false), 1 => visitor.visit_bool(true), _ => Err(bad()) }
    }
    fn deserialize_char<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let raw = u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| bad())?);
        visitor.visit_char(char::from_u32(raw).ok_or_else(bad)?)
    }
    fn deserialize_str<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let len = self.count()?;
        visitor.visit_borrowed_str(std::str::from_utf8(self.take(len)?).map_err(|_| bad())?)
    }
    fn deserialize_string<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { self.deserialize_str(visitor) }
    fn deserialize_bytes<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let len = self.count()?; visitor.visit_borrowed_bytes(self.take(len)?)
    }
    fn deserialize_byte_buf<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { self.deserialize_bytes(visitor) }
    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.take(1)?[0] { 0 => visitor.visit_none(), 1 => visitor.visit_some(self), _ => Err(bad()) }
    }
    fn deserialize_unit<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { visitor.visit_unit() }
    fn deserialize_unit_struct<V: de::Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value> { visitor.visit_unit() }
    fn deserialize_newtype_struct<V: de::Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value> { visitor.visit_newtype_struct(self) }
    fn deserialize_seq<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { self.sequence(visitor, None, false) }
    fn deserialize_tuple<V: de::Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> { self.sequence(visitor, Some(len), false) }
    fn deserialize_tuple_struct<V: de::Visitor<'de>>(self, _: &'static str, len: usize, visitor: V) -> Result<V::Value> { self.sequence(visitor, Some(len), false) }
    fn deserialize_map<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { self.sequence(visitor, None, true) }
    fn deserialize_struct<V: de::Visitor<'de>>(self, _: &'static str, fields: &'static [&'static str], visitor: V) -> Result<V::Value> { self.sequence(visitor, Some(fields.len()), false) }
    fn deserialize_enum<V: de::Visitor<'de>>(self, _: &'static str, _: &'static [&'static str], visitor: V) -> Result<V::Value> {
        let index = u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| bad())?);
        visitor.visit_enum(Enum { reader: self, index })
    }
    fn deserialize_identifier<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> { self.deserialize_u32(visitor) }
    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, _: V) -> Result<V::Value> { Err(bad()) }
    fn is_human_readable(&self) -> bool { false }
}
struct Enum<'a, 'de> { reader: &'a mut Reader<'de>, index: u32 }
impl<'a, 'de> de::EnumAccess<'de> for Enum<'a, 'de> {
    type Error = Error; type Variant = &'a mut Reader<'de>;
    fn variant_seed<V: de::DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let index = seed.deserialize(de::value::U32Deserializer::<Error>::new(self.index))?;
        Ok((index, self.reader))
    }
}
impl<'de> de::VariantAccess<'de> for &mut Reader<'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> { Ok(()) }
    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> { seed.deserialize(self) }
    fn tuple_variant<V: de::Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> { self.sequence(visitor, Some(len), false) }
    fn struct_variant<V: de::Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> { self.sequence(visitor, Some(fields.len()), false) }
}
