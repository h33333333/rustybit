use core::str;

use serde::{Deserialize, Deserializer};

use crate::{Error, ErrorKind};

pub struct BencodeDeserializer<'de> {
    input: &'de [u8],
    position: usize,
}

impl<'de> BencodeDeserializer<'de> {
    pub fn from_str(input: &'de str) -> Self {
        BencodeDeserializer {
            input: input.as_bytes(),
            position: 0,
        }
    }

    pub fn from_bytes(input: &'de [u8]) -> Self {
        BencodeDeserializer { input, position: 0 }
    }

    pub fn error(&self, kind: ErrorKind) -> Error {
        let err: Error = kind.into();
        err.set_position(self.position)
    }

    pub(crate) fn move_cursor(&mut self, by: usize) {
        self.position += by;
        self.input = &self.input[by..]
    }
}

impl<'de> BencodeDeserializer<'de> {
    fn parse_integer(&mut self) -> Result<i64, Error> {
        match self.input.iter().position(|byte| *byte == b'e') {
            Some(pos) => {
                let integer = &self.input[..pos];
                let parsed_integer: i64 = std::str::from_utf8(integer)
                    .map_err(|_| self.error(ErrorKind::BadInputData("Bad integer")))?
                    .parse()
                    .map_err(|_| {
                        self.error(ErrorKind::BadInputData(
                            "Unnable to parse integer from the provided data",
                        ))
                    })?;
                self.move_cursor(pos + 1);
                Ok(parsed_integer)
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected closing delimiter for integer"))),
        }
    }

    fn parse_bytes(&mut self) -> Result<&'de [u8], Error> {
        match self.input.iter().position(|byte| *byte == b':') {
            Some(delim_pos) => {
                let raw_bytes_len = &self.input[..delim_pos];
                let bytes_len: usize = std::str::from_utf8(raw_bytes_len)
                    .map_err(|_| self.error(ErrorKind::BadInputData("bytes length is not valid utf8")))?
                    .parse()
                    .map_err(|_| self.error(ErrorKind::BadInputData("expected valid bytes length")))?;
                // Skip the delimiter as well
                self.move_cursor(delim_pos + 1);
                let raw_bytes = &self.input[..bytes_len];
                self.move_cursor(bytes_len);
                Ok(raw_bytes)
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected bytes delimiter ':'"))),
        }
    }

    fn parse_bytes_checked(&mut self) -> Result<&'de [u8], Error> {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("bytes")))?
        {
            b'0'..=b'9' => self.parse_bytes(),
            _ => Err(self.error(ErrorKind::BadInputData("expected bytes length"))),
        }
    }
}

impl<'de, 'a> serde::de::Deserializer<'de> for &'a mut BencodeDeserializer<'de> {
    type Error = Error;

    fn deserialize_bool<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        Err(self.error(ErrorKind::Unsupported("bool")))
    }

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("any bencode value")))?
        {
            b'd' => self.deserialize_map(visitor),
            b'l' => self.deserialize_seq(visitor),
            b'i' => self.deserialize_i64(visitor),
            _ => self.deserialize_bytes(visitor),
        }
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("integer")))?
        {
            b'i' => {
                // Skip int label
                self.move_cursor(1);
                visitor.visit_i64(self.parse_integer()?)
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected integer label 'i'"))),
        }
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_f32<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        Err(self.error(ErrorKind::Unsupported("f32")))
    }

    fn deserialize_f64<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        Err(self.error(ErrorKind::Unsupported("f64")))
    }

    fn deserialize_char<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        Err(self.error(ErrorKind::Unsupported("char")))
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        let str = str::from_utf8(self.parse_bytes_checked()?)
            .map_err(|_| self.error(ErrorKind::BadInputData("expected valid utf8 string")))?;
        visitor.visit_borrowed_str(str)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_borrowed_bytes(self.parse_bytes_checked()?)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("null string")))?
        {
            b'0' => {
                let _ = self.parse_bytes()?;
                visitor.visit_none()
            }
            _ => visitor.visit_some(&mut *self),
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        let bytes = self.parse_bytes_checked()?;
        if !bytes.is_empty() {
            return Err(self.error(ErrorKind::BadInputData("expected bencode string of length 0")));
        }
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("bencode list")))?
        {
            b'l' => {
                // Skip list label
                self.move_cursor(1);
                let value = visitor.visit_seq(BencodeAccessor { de: self });
                match self
                    .input
                    .first()
                    .ok_or(self.error(ErrorKind::UnexpectedEof("bencode list end")))?
                {
                    b'e' => {
                        // Skip list end
                        self.move_cursor(1);
                        value
                    }
                    _ => Err(self.error(ErrorKind::BadInputData("expected bencode list end"))),
                }
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected bencode list"))),
        }
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(self, _name: &'static str, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("bencode dictionary")))?
        {
            b'd' => {
                // Skip dict label
                self.move_cursor(1);
                let value = visitor.visit_map(BencodeAccessor { de: self });
                match self
                    .input
                    .first()
                    .ok_or(self.error(ErrorKind::UnexpectedEof("bencode dictionary end")))?
                {
                    b'e' => {
                        // Skip dict end
                        self.move_cursor(1);
                        value
                    }
                    _ => Err(self.error(ErrorKind::BadInputData("expected bencode dictionary end"))),
                }
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected bencode dictionary"))),
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self
            .input
            .first()
            .ok_or(self.error(ErrorKind::UnexpectedEof("bencode dictionary")))?
        {
            b'd' => {
                // Skip dict label
                self.move_cursor(1);
                let value = visitor.visit_enum(BencodeAccessor { de: self });
                match self
                    .input
                    .first()
                    .ok_or(self.error(ErrorKind::UnexpectedEof("bencode dictionary end")))?
                {
                    b'e' => {
                        // Skip dict end
                        self.move_cursor(1);
                        value
                    }
                    _ => Err(self.error(ErrorKind::BadInputData("expected bencode dictionary end"))),
                }
            }
            _ => Err(self.error(ErrorKind::BadInputData("expected bencode dictionary"))),
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
}

struct BencodeAccessor<'a, 'de> {
    de: &'a mut BencodeDeserializer<'de>,
}

impl<'a, 'de> serde::de::SeqAccess<'de> for BencodeAccessor<'a, 'de> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        if *self
            .de
            .input
            .first()
            .ok_or(self.de.error(ErrorKind::UnexpectedEof("next seq element or end")))?
            == b'e'
        {
            return Ok(None);
        }
        seed.deserialize(&mut *self.de).map(Some)
    }
}

impl<'a, 'de> serde::de::MapAccess<'de> for BencodeAccessor<'a, 'de> {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: serde::de::DeserializeSeed<'de>,
    {
        if *self
            .de
            .input
            .first()
            .ok_or(self.de.error(ErrorKind::UnexpectedEof("next dict key or end")))?
            == b'e'
        {
            return Ok(None);
        }
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.de)
    }
}

impl<'de, 'a> serde::de::EnumAccess<'de> for BencodeAccessor<'a, 'de> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        Ok((seed.deserialize(&mut *self.de)?, self))
    }
}

impl<'de, 'a> serde::de::VariantAccess<'de> for BencodeAccessor<'a, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        let bytes = self.de.parse_bytes_checked()?;
        if !bytes.is_empty() {
            return Err(self
                .de
                .error(ErrorKind::BadInputData("expected bencode string of length 0")));
        }
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.de)
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.de.deserialize_seq(visitor)
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.de.deserialize_map(visitor)
    }
}

pub fn from_bytes<'de, T: Deserialize<'de>>(input: &'de [u8]) -> Result<T, Error> {
    let mut deserializer = BencodeDeserializer::from_bytes(input);
    let deserialized = T::deserialize(&mut deserializer)?;
    if !deserializer.input.is_empty() {
        return Err(ErrorKind::Custom(format!(
            "Trailing bytes after deserialization: {}",
            deserializer.input.len()
        ))
        .into());
    }
    Ok(deserialized)
}

pub fn from_str<'de, T: Deserialize<'de>>(input: &'de str) -> Result<T, Error> {
    from_bytes(input.as_bytes())
}
