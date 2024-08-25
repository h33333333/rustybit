use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
pub struct Bytes<'a>(Cow<'a, [u8]>);

impl<'a> Serialize for Bytes<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BencodeValue<'a> {
    Int(i64),
    Bytes(Bytes<'a>),
    List(Vec<BencodeValue<'a>>),
    Dict(HashMap<Bytes<'a>, BencodeValue<'a>>),
}

impl<'a, 'de: 'a> serde::de::Deserialize<'de> for BencodeValue<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor<'a> {
            lifetime: PhantomData<BencodeValue<'a>>,
        }

        impl<'a, 'de: 'a> serde::de::Visitor<'de> for Visitor<'a> {
            type Value = BencodeValue<'a>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "a valid bencode value")
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(v.into())
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut v: Vec<BencodeValue<'a>> = Vec::new();
                while let Some(value) = seq.next_element()? {
                    v.push(value);
                }
                Ok(v.into())
            }

            fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(v.into())
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut hashmap = HashMap::new();
                while let Some(key) = map.next_key::<&'de [u8]>()? {
                    let value = map.next_value()?;
                    hashmap.insert(Bytes(Cow::Borrowed(key)), value);
                }
                Ok(hashmap.into())
            }
        }

        deserializer.deserialize_any(Visitor { lifetime: PhantomData })
    }
}

impl From<i64> for BencodeValue<'_> {
    fn from(value: i64) -> Self {
        BencodeValue::Int(value)
    }
}

impl<'a> From<&'a [u8]> for BencodeValue<'a> {
    fn from(value: &'a [u8]) -> Self {
        BencodeValue::Bytes(Bytes(Cow::Borrowed(value)))
    }
}

impl From<String> for BencodeValue<'_> {
    fn from(s: String) -> Self {
        BencodeValue::Bytes(Bytes(Cow::Owned(s.into_bytes())))
    }
}

impl<'a> From<&'a str> for BencodeValue<'a> {
    fn from(v: &'a str) -> Self {
        BencodeValue::Bytes(Bytes(Cow::Borrowed(v.as_bytes())))
    }
}

impl From<Vec<u8>> for BencodeValue<'_> {
    fn from(value: Vec<u8>) -> Self {
        BencodeValue::Bytes(Bytes(Cow::Owned(value)))
    }
}

impl<'a> From<Vec<BencodeValue<'a>>> for BencodeValue<'a> {
    fn from(value: Vec<BencodeValue<'a>>) -> Self {
        BencodeValue::List(value)
    }
}

impl<'a> From<HashMap<Bytes<'a>, BencodeValue<'a>>> for BencodeValue<'a> {
    fn from(value: HashMap<Bytes<'a>, BencodeValue<'a>>) -> Self {
        BencodeValue::Dict(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_str, to_string};

    mod basic_types {
        use super::*;

        #[test]
        fn can_roundtrip_int() {
            let value: BencodeValue = 888i64.into();
            let serialized = to_string(&value).expect("failed to serialize an integer");
            assert_eq!(serialized, "i888e");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize an integer");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_negative_int() {
            let value: BencodeValue = (-888i64).into();
            let serialized = to_string(&value).expect("failed to serialize a neg integer");
            assert_eq!(serialized, "i-888e");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a neg integer");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_bytes() {
            let value: BencodeValue = "a nice value".into();
            let serialized = to_string(&value).expect("failed to serialize a string");
            assert_eq!(serialized, "12:a nice value");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a string");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_list_of_bytes() {
            let list: Vec<BencodeValue> = vec!["a nice value".into(), "another".into()];
            let value: BencodeValue = list.into();
            let serialized = to_string(&value).expect("failed to serialize a list of bytes");
            assert_eq!(serialized, "l12:a nice value7:anothere");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a list of bytes");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_list_of_ints() {
            let list: Vec<BencodeValue> = vec![10i64.into(), 11i64.into()];
            let value: BencodeValue = list.into();
            let serialized = to_string(&value).expect("failed to serialize a list of ints");
            assert_eq!(serialized, "li10ei11ee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a list of ints");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_list_of_lists() {
            let list: Vec<BencodeValue> = vec![
                BencodeValue::List(vec!["bytes".into()]),
                BencodeValue::List(vec![20i64.into()]),
            ];
            let value: BencodeValue = list.into();
            let serialized = to_string(&value).expect("failed to serialize a list of lists");
            assert_eq!(serialized, "ll5:byteseli20eee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a list of lists");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_list_of_dicts() {
            let dict_1 = HashMap::from([(Bytes(b"test".into()), BencodeValue::List(vec![21i64.into()]))]);
            let dict_2 = HashMap::from([(Bytes(b"another".into()), BencodeValue::Dict(HashMap::new()))]);

            let list: Vec<BencodeValue> = vec![dict_1.into(), dict_2.into()];
            let value: BencodeValue = list.into();
            let serialized = to_string(&value).expect("failed to serialize a list of dicts");
            assert_eq!(serialized, "ld4:testli21eeed7:anotherdeee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a list of dicts");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_dict_of_ints() {
            let dict: HashMap<Bytes, BencodeValue> = HashMap::from([(Bytes(b"key1".into()), 4i64.into())]);
            let value: BencodeValue = dict.into();
            let serialized = to_string(&value).expect("failed to serialize a dict of ints");
            assert_eq!(serialized, "d4:key1i4ee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a dict of ints");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_dict_of_bytes() {
            let dict: HashMap<Bytes, BencodeValue> = HashMap::from([(Bytes(b"key1".into()), "my value".into())]);
            let value: BencodeValue = dict.into();
            let serialized = to_string(&value).expect("failed to serialize a dict of bytes");
            assert_eq!(serialized, "d4:key18:my valuee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a dict of bytes");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_dict_of_lists() {
            let dict: HashMap<Bytes, BencodeValue> =
                HashMap::from([(Bytes(b"key1".into()), vec![BencodeValue::Dict(HashMap::new())].into())]);
            let value: BencodeValue = dict.into();
            let serialized = to_string(&value).expect("failed to serialize a dict of lists");
            assert_eq!(serialized, "d4:key1ldeee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a dict of lists");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_dict_of_dicts() {
            let dict: HashMap<Bytes, BencodeValue> = HashMap::from([(
                Bytes(b"key1".into()),
                HashMap::from([(Bytes(b"inner".into()), BencodeValue::Dict(HashMap::new()))]).into(),
            )]);
            let value: BencodeValue = dict.into();
            let serialized = to_string(&value).expect("failed to serialize a dict of dicts");
            assert_eq!(serialized, "d4:key1d5:innerdeee");
            let deserialized = from_str::<BencodeValue>(&serialized).expect("failed to deserialize a dict of dicts");
            assert_eq!(deserialized, value)
        }
    }

    mod option {
        use super::*;

        #[test]
        fn can_roundtrip_some() {
            let value: Option<i64> = Some(10);
            let serialized = to_string(&value).expect("failed to serialize an Option::Some");
            assert_eq!(serialized, "i10e");
            let deserialized = from_str::<Option<i64>>(&serialized).expect("failed to deserialize an Option::Some");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_none() {
            let value: Option<i64> = None;
            let serialized = to_string(&value).expect("failed to serialize an Option::None");
            assert_eq!(serialized, "0:");
            let deserialized = from_str::<Option<i64>>(&serialized).expect("failed to deserialize an Option::None");
            assert_eq!(deserialized, value)
        }
    }

    mod unit {
        use super::*;

        #[test]
        fn can_roundtrip_unit() {
            let value = ();
            let serialized = to_string(&value).expect("failed to serialize the unit type");
            assert_eq!(serialized, "0:");
            let deserialized = from_str::<()>(&serialized).expect("failed to deserialize the unit type");
            assert_eq!(deserialized, value)
        }
    }

    mod _struct {
        use super::*;

        #[test]
        fn can_roundtrip_unit_struct() {
            #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
            struct Test;

            let value = Test;
            let serialized = to_string(&value).expect("failed to serialize a unit struct");
            assert_eq!(serialized, "0:");
            let deserialized = from_str::<Test>(&serialized).expect("failed to deserialize a unit struct");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_newtype_struct() {
            #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
            struct Test(i64);

            let value = Test(11);
            let serialized = to_string(&value).expect("failed to serialize a newtype struct");
            assert_eq!(serialized, "i11e");
            let deserialized = from_str::<Test>(&serialized).expect("failed to deserialize a newtype struct");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_tuple_struct() {
            #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
            struct Test<'a>(i64, i64, &'a str);

            let value = Test(10, 11, "hey!");
            let serialized = to_string(&value).expect("failed to serialize a tuple struct");
            assert_eq!(serialized, "li10ei11e4:hey!e");
            let deserialized = from_str::<Test>(&serialized).expect("failed to deserialize a tuple struct");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_struct() {
            #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
            struct Test<'a> {
                field_1: &'a str,
                field_2: i64,
            }

            let value = Test {
                field_1: "test",
                field_2: 555,
            };
            let serialized = to_string(&value).expect("failed to serialize a struct");
            assert_eq!(serialized, "d7:field_14:test7:field_2i555ee");
            let deserialized = from_str::<Test>(&serialized).expect("failed to deserialize a struct");
            assert_eq!(deserialized, value)
        }
    }

    mod _enum {
        use super::*;

        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        enum Enumeration<'a> {
            Unit,
            NewType(i64),
            Tuple(i64, i64, &'a str),
            Struct { field: &'a str, integer: i64 },
        }

        #[test]
        fn can_roundtrip_unit_variant() {
            let value = Enumeration::Unit;
            let serialized = to_string(&value).expect("failed to serialize a unit variant");
            assert_eq!(serialized, "d4:Unit0:e");
            let deserialized = from_str::<Enumeration>(&serialized).expect("failed to deserialize a unit variant");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_newtype_variant() {
            let value = Enumeration::NewType(15);
            let serialized = to_string(&value).expect("failed to serialize a newtype variant");
            assert_eq!(serialized, "d7:NewTypei15ee");
            let deserialized = from_str::<Enumeration>(&serialized).expect("failed to deserialize a newtype variant");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_tuple_variant() {
            let value = Enumeration::Tuple(10, 12, "hi from enum");
            let serialized = to_string(&value).expect("failed to serialize a tuple variant");
            assert_eq!(serialized, "d5:Tupleli10ei12e12:hi from enumee");
            let deserialized = from_str::<Enumeration>(&serialized).expect("failed to deserialize a tuple variant");
            assert_eq!(deserialized, value)
        }

        #[test]
        fn can_roundtrip_struct_variant() {
            let value = Enumeration::Struct {
                field: "hi from enum",
                integer: 22,
            };
            let serialized = to_string(&value).expect("failed to serialize a struct variant");
            assert_eq!(serialized, "d6:Structd5:field12:hi from enum7:integeri22eee");
            let deserialized = from_str::<Enumeration>(&serialized).expect("failed to deserialize a struct variant");
            assert_eq!(deserialized, value)
        }
    }
}
