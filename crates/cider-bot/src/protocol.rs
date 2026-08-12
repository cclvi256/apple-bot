use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use serde_json::{Value, json};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Id(String);

impl Id {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl Visitor<'_> for IdVisitor {
            type Value = Id;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-negative QQ identifier encoded as a number or string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(Id(value.to_string()))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u64::try_from(value)
                    .map(|value| Id(value.to_string()))
                    .map_err(|_| E::custom("identifier must not be negative"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Id::new(value)
                    .ok_or_else(|| E::custom("identifier must contain only decimal digits"))
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub post_type: String,
    pub message_type: Option<String>,
    pub sub_type: Option<String>,
    pub self_id: Id,
    pub message_id: Option<Value>,
    pub user_id: Option<Id>,
    pub group_id: Option<Id>,
    #[serde(default)]
    pub message: Vec<MessageSegment>,
    pub sender: Option<Sender>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Sender {
    pub role: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MessageSegment {
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

impl MessageSegment {
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            kind: "text".into(),
            data: json!({ "text": value.into() }),
        }
    }

    pub fn at(qq: &Id) -> Self {
        Self {
            kind: "at".into(),
            data: json!({ "qq": qq.as_str() }),
        }
    }

    pub fn text_value(&self) -> Option<&str> {
        (self.kind == "text")
            .then(|| self.data.get("text")?.as_str())
            .flatten()
    }

    pub fn at_value(&self) -> Option<Id> {
        if self.kind != "at" {
            return None;
        }
        serde_json::from_value(self.data.get("qq")?.clone()).ok()
    }

    pub fn dice_result(&self) -> Option<u8> {
        if self.kind != "dice" {
            return None;
        }
        let result = self.data.get("result")?;
        let result = match result {
            Value::String(value) => value.parse().ok()?,
            Value::Number(value) => u8::try_from(value.as_u64()?).ok()?,
            _ => return None,
        };
        (1..=6).contains(&result).then_some(result)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct QuickOperation {
    pub reply: Vec<MessageSegment>,
    pub at_sender: bool,
}

impl QuickOperation {
    pub fn reply(reply: Vec<MessageSegment>) -> Self {
        Self {
            reply,
            at_sender: false,
        }
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self::reply(vec![MessageSegment::text(value)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_accepts_numbers_and_strings() {
        let number: Id = serde_json::from_str("123456").unwrap();
        let string: Id = serde_json::from_str(r#""123456""#).unwrap();
        assert_eq!(number, string);
        assert_eq!(serde_json::to_string(&number).unwrap(), r#""123456""#);
    }

    #[test]
    fn dice_requires_a_result_from_one_through_six() {
        let valid: MessageSegment =
            serde_json::from_value(json!({"type":"dice", "data":{"result":"6"}})).unwrap();
        let invalid: MessageSegment =
            serde_json::from_value(json!({"type":"dice", "data":{"result":"7"}})).unwrap();
        assert_eq!(valid.dice_result(), Some(6));
        assert_eq!(invalid.dice_result(), None);
    }
}
