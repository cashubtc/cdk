use bitcoin::secp256k1::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_bytes::ByteBuf;

use super::SpendInfo;
use crate::nuts::SecretKey;
use crate::util::hex;

#[derive(Serialize, Deserialize)]
struct Wire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    k: Option<ByteBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<ByteBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    i: Option<ByteBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<Vec<ByteBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    u: Option<ByteBuf>,
}

pub(crate) fn serialize<S>(info: &Option<SpendInfo>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let wire = info
        .as_ref()
        .map(|info| -> Result<Wire, S::Error> {
            let secret = |key: &SecretKey| {
                key.as_secp256k1()
                    .map(|k| ByteBuf::from(k.secret_bytes().to_vec()))
                    .map_err(serde::ser::Error::custom)
            };
            Ok(Wire {
                k: info.bearer_key.as_ref().map(secret).transpose()?,
                e: info
                    .ephemeral_key
                    .map(|key| ByteBuf::from(key.serialize().to_vec())),
                i: info
                    .internal_key
                    .map(|key| ByteBuf::from(key.serialize().to_vec())),
                t: info
                    .tree
                    .as_ref()
                    .map(|leaves| {
                        leaves
                            .iter()
                            .map(|leaf| {
                                hex::decode(leaf)
                                    .map(ByteBuf::from)
                                    .map_err(serde::ser::Error::custom)
                            })
                            .collect::<Result<_, _>>()
                    })
                    .transpose()?,
                u: info.nums_offset.as_ref().map(secret).transpose()?,
            })
        })
        .transpose()?;
    wire.serialize(serializer)
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<SpendInfo>, D::Error>
where
    D: Deserializer<'de>,
{
    let wire = Option::<Wire>::deserialize(deserializer)?;
    wire.map(|wire| {
        let point = |bytes: ByteBuf| {
            if bytes.len() != 33 {
                return Err(serde::de::Error::custom("Nutroot point must be compressed"));
            }
            PublicKey::from_slice(&bytes).map_err(serde::de::Error::custom)
        };
        Ok(SpendInfo {
            bearer_key: wire
                .k
                .map(|bytes| SecretKey::from_slice(&bytes).map_err(serde::de::Error::custom))
                .transpose()?,
            ephemeral_key: wire.e.map(point).transpose()?,
            internal_key: wire.i.map(point).transpose()?,
            tree: wire.t.map(|leaves| {
                leaves
                    .into_iter()
                    .map(|leaf| hex::encode(leaf.as_ref()))
                    .collect()
            }),
            nums_offset: wire
                .u
                .map(|bytes| SecretKey::from_slice(&bytes).map_err(serde::de::Error::custom))
                .transpose()?,
        })
    })
    .transpose()
}
