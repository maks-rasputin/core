use serde::{Deserialize, Deserializer, de::Error as _};

use super::TonAddress;

pub mod hex_or_base64 {
    use super::*;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TonAddress, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        TonAddress::try_parse_hex(&value)
            .or_else(|| TonAddress::try_parse_base64(&value))
            .ok_or_else(|| D::Error::custom("invalid TON address"))
    }
}
