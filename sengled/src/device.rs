use std::{collections::HashMap, fmt};

use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};

/// A device provided by the Sengled API.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Device {
    pub category: String,

    #[serde(rename = "deviceUuid")]
    pub mac: String,

    #[serde(rename = "typeCode")]
    pub type_code: String,

    #[serde(
        rename = "attributeList",
        deserialize_with = "deserialize_attribute_list"
    )]
    pub attributes: HashMap<String, String>,
}

impl Device {
    /// Get an attribute on the device.
    pub fn get_attribute(&self, attribute: &str) -> Option<&str> {
        self.attributes.get(attribute).map(String::as_str)
    }

    /// Get an attribute on the device, or fallback to a default.
    pub fn get_attribute_or<'a>(&'a self, attribute: &str, default: &'a str) -> &'a str {
        self.attributes
            .get(attribute)
            .map(String::as_str)
            .unwrap_or(default)
    }

    // /// Set an attribute on this device. This function will call [`Client::set_device_attribute`]
    // /// and update the attribute locally on this object.
    // pub async fn set_attribute(
    //     &mut self,
    //     client: &Client,
    //     attribute: impl Into<String>,
    //     value: impl Into<String>,
    // ) -> Result<(), Error> {
    //     let att = attribute.into();
    //     let val = value.into();

    //     client.set_device_attribute(&self.mac, &att, &val).await?;
    //     self.attributes.insert(att, val);

    //     Ok(())
    // }
}

fn deserialize_attribute_list<'de, D: Deserializer<'de>>(
    deserialize: D,
) -> Result<HashMap<String, String>, D::Error> {
    #[derive(Deserialize)]
    struct Attribute {
        name: String,
        value: String,
    }

    struct AttributeVisitor;

    impl<'de> Visitor<'de> for AttributeVisitor {
        type Value = HashMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> std::fmt::Result {
            write!(formatter, "an attribute, an object with a name and value")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut map = HashMap::new();

            while let Some(Attribute { name, value }) = seq.next_element()? {
                map.insert(name, value);
            }

            Ok(map)
        }
    }

    deserialize.deserialize_seq(AttributeVisitor)
}

pub trait AsDeviceMac {
    fn as_device_mac(&self) -> &str;
}

impl AsDeviceMac for Device {
    fn as_device_mac(&self) -> &str {
        &self.mac
    }
}

impl AsDeviceMac for &Device {
    fn as_device_mac(&self) -> &str {
        &self.mac
    }
}

impl AsDeviceMac for &mut Device {
    fn as_device_mac(&self) -> &str {
        &self.mac
    }
}

impl<T> AsDeviceMac for T
where
    T: AsRef<str>,
{
    fn as_device_mac(&self) -> &str {
        self.as_ref()
    }
}
