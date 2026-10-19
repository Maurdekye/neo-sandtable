// Parse complete event JSON without silently selecting one duplicate member.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;
struct Checked(Value);
impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Checked;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Checked, E> {
                Ok(Checked(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Checked, E> {
                Ok(Checked(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Checked, E> {
                Ok(Checked(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Checked, E> {
                Number::from_f64(v)
                    .map(|n| Checked(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Checked, E> {
                Ok(Checked(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Checked, E> {
                Ok(Checked(Value::String(v)))
            }
            fn visit_none<E: de::Error>(self) -> Result<Checked, E> {
                Ok(Checked(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Checked, E> {
                Ok(Checked(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Checked, A::Error> {
                let mut out = Vec::new();
                while let Some(Checked(v)) = seq.next_element()? {
                    out.push(v);
                }
                Ok(Checked(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Checked, A::Error> {
                let mut out = Map::new();
                while let Some(k) = map.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom("duplicate member"));
                    }
                    let Checked(v) = map.next_value()?;
                    out.insert(k, v);
                }
                Ok(Checked(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(V)
    }
}
pub(super) fn parse(bytes: &[u8]) -> Result<Value, ()> {
    serde_json::from_slice::<Checked>(bytes)
        .map(|v| v.0)
        .map_err(|_| ())
}
