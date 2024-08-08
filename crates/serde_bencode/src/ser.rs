use std::io::Write;

use serde::Serialize;

use crate::{error::Error, ErrorKind};

pub struct Serializer<'a, W: Write> {
    output: &'a mut W,
}

pub struct SerializeMap<'a, 'w, W: Write> {
    serializer: &'a mut Serializer<'w, W>,
}

impl<'a, 'w, W: Write> serde::ser::SerializeMap for SerializeMap<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        key.serialize(&mut *self.serializer)
    }

    fn serialize_value<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer
            .output
            .write(b"e")
            .map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(())
    }
}

pub struct SerializeStruct<'a, 'w, W: Write> {
    serializer: &'a mut Serializer<'w, W>,
}

impl<'a, 'w, W: Write> serde::ser::SerializeStruct for SerializeStruct<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        key.serialize(&mut *self.serializer)?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer
            .output
            .write(b"e")
            .map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(())
    }
}

pub struct SerializeSeq<'a, 'w, W: Write> {
    serializer: &'a mut Serializer<'w, W>,
}

impl<'a, 'w, W: Write> serde::ser::SerializeSeq for SerializeSeq<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer
            .output
            .write(b"e")
            .map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(())
    }
}

impl<'a, 'w, W: Write> serde::ser::SerializeTuple for SerializeSeq<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize,
    {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeSeq::end(self)
    }
}

impl<'a, 'w, W: Write> serde::ser::SerializeTupleStruct for SerializeSeq<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize,
    {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeSeq::end(self)
    }
}

pub struct SerializeComplexVariants<'a, 'w, W: Write> {
    serializer: &'a mut Serializer<'w, W>,
}

impl<'a, 'w, W: Write> serde::ser::SerializeTupleVariant for SerializeComplexVariants<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize,
    {
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.serializer
            .output
            .write(b"ee")
            .map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(())
    }
}

impl<'a, 'w, W: Write> serde::ser::SerializeStructVariant for SerializeComplexVariants<'a, 'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize,
    {
        key.serialize(&mut *self.serializer)?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeTupleVariant::end(self)
    }
}

impl<'a, 'w, W: Write> serde::ser::Serializer for &'a mut Serializer<'w, W> {
    type Ok = ();
    type Error = Error;

    type SerializeTuple = SerializeSeq<'a, 'w, W>;
    type SerializeTupleStruct = SerializeSeq<'a, 'w, W>;
    type SerializeTupleVariant = SerializeComplexVariants<'a, 'w, W>;
    type SerializeSeq = SerializeSeq<'a, 'w, W>;
    type SerializeMap = SerializeMap<'a, 'w, W>;
    type SerializeStruct = SerializeStruct<'a, 'w, W>;
    type SerializeStructVariant = SerializeComplexVariants<'a, 'w, W>;

    fn serialize_bool(self, _v: bool) -> Result<Self::Ok, Self::Error> {
        Err(ErrorKind::Unsupported("bool").into())
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.output
            .write_fmt(format_args!("i{v}e"))
            .map_err(|e| ErrorKind::Custom(e.to_string()).into())
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.output
            .write_fmt(format_args!("i{v}e"))
            .map_err(|e| ErrorKind::Custom(e.to_string()).into())
    }

    fn serialize_f32(self, _v: f32) -> Result<Self::Ok, Self::Error> {
        Err(ErrorKind::Unsupported("f32").into())
    }

    fn serialize_f64(self, _v: f64) -> Result<Self::Ok, Self::Error> {
        Err(ErrorKind::Unsupported("f64").into())
    }

    fn serialize_char(self, _v: char) -> Result<Self::Ok, Self::Error> {
        Err(ErrorKind::Unsupported("char").into())
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        let len = v.len();
        self.output
            .write_fmt(format_args!("{len}:{v}"))
            .map_err(|e| ErrorKind::Custom(e.to_string()).into())
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        let len = v.len();
        self.output
            .write_fmt(format_args!("{len}:"))
            .map_err(|e| ErrorKind::Custom(e.to_string()))?;
        self.output.write(v).map_err(|e| ErrorKind::Custom(e.to_string()))?;

        Ok(())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.serialize_str("")
    }

    fn serialize_some<T: ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.serialize_str("")
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.serialize_str("")
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        let mut map_serializer = self.serialize_map(None)?;
        serde::ser::SerializeMap::serialize_entry(&mut map_serializer, variant, "")?;
        serde::ser::SerializeMap::end(map_serializer)
    }

    fn serialize_newtype_struct<T: ?Sized>(self, _name: &'static str, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        let mut map_serializer = self.serialize_map(None)?;
        serde::ser::SerializeMap::serialize_entry(&mut map_serializer, variant, value)?;
        serde::ser::SerializeMap::end(map_serializer)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.output.write(b"l").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(SerializeSeq { serializer: self })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.output.write(b"d").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        variant.serialize(&mut *self)?;
        self.output.write(b"l").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(SerializeComplexVariants { serializer: self })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.output.write(b"d").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(SerializeMap { serializer: self })
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Self::Error> {
        self.output.write(b"d").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(SerializeStruct { serializer: self })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.output.write(b"d").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        variant.serialize(&mut *self)?;
        self.output.write(b"d").map_err(|e| ErrorKind::Custom(e.to_string()))?;
        Ok(SerializeComplexVariants { serializer: self })
    }
}

pub fn to_string<T: serde::Serialize>(value: &T) -> Result<String, Error> {
    let mut buff = Vec::new();
    let mut serializer = Serializer { output: &mut buff };
    value.serialize(&mut serializer)?;
    String::from_utf8(buff).map_err(|e| ErrorKind::Custom(e.to_string()).into())
}

pub fn to_writer<T: serde::Serialize, W: Write>(value: &T, writer: &mut W) -> Result<(), Error> {
    let mut serializer = Serializer { output: writer };
    value.serialize(&mut serializer)?;
    Ok(())
}
