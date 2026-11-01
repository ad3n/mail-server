// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use store::Store;
use store::{
    IterateParams, Subspace,
    write::{AnyClass, AnyKey, BatchBuilder, ValueClass, serialize::RawValue},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegratedApp {
    pub app_id: String,
    pub name: String,
    pub webhook_url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryEvent {
    pub event_id: String,
    pub app_id: String,
    pub external_id: String,
    pub queue_id: String,
    pub recipient: String,
    pub status: String,
    pub updated_at: u64,
    pub details: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

pub fn valid_id(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && s != "."
        && s != ".."
}

pub fn apps_prefix() -> String {
    "delivery/apps/".into()
}
pub fn app_key(id: &str) -> String {
    format!("delivery/apps/{id}")
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCredentials {
    #[serde(rename = "appKey")]
    pub api_key: String,
    #[serde(rename = "appToken")]
    pub token: String,
}
impl AppCredentials {
    pub fn verifies(&self, token: &str) -> bool {
        aws_lc_rs::constant_time::verify_slices_are_equal(self.token.as_bytes(), token.as_bytes())
            .is_ok()
    }
}
pub fn credential_key(id: &str) -> String {
    format!("delivery/credentials/{id}")
}
pub fn api_key_index(api_key: &str) -> String {
    format!("delivery/keys/{api_key}")
}

pub async fn authenticate_app(
    store: &Store,
    key: &str,
    token: &str,
) -> trc::Result<Option<String>> {
    if !valid_id(key) || !valid_id(token) {
        return Ok(None);
    }
    let Some(id) = delivery_get::<String>(store, &api_key_index(key)).await? else {
        return Ok(None);
    };
    let credentials = delivery_get::<AppCredentials>(store, &credential_key(&id)).await?;
    Ok(credentials
        .filter(|c| c.api_key == key && c.verifies(token))
        .map(|_| id))
}

pub fn result_prefix(app_id: &str, external: &str) -> String {
    format!("delivery/results/{external}/{app_id}/")
}
pub fn callback_key(due: u64, event_id: &str) -> String {
    format!("delivery/outbox/{due:016x}/{event_id}")
}

fn key(value: &str) -> AnyKey<Vec<u8>> {
    AnyKey {
        subspace: Subspace::System,
        key: value.as_bytes().to_vec(),
    }
}
pub fn class(value: &str) -> ValueClass {
    ValueClass::Any(AnyClass {
        subspace: Subspace::System,
        key: value.as_bytes().to_vec(),
    })
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> trc::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        trc::StoreEvent::DataCorruption
            .into_err()
            .details(e.to_string())
    })
}
pub fn put<T: Serialize>(batch: &mut BatchBuilder, k: &str, value: &T) {
    batch.set(
        class(k),
        serde_json::to_vec(value).expect("serializable delivery record"),
    );
}

pub async fn delivery_get<T: DeserializeOwned>(store: &Store, k: &str) -> trc::Result<Option<T>> {
    store
        .get_value::<RawValue>(key(k))
        .await?
        .map(|v| decode(&v.0))
        .transpose()
}
pub async fn delivery_range<T: DeserializeOwned + Send + Sync>(
    store: &Store,
    start: &str,
    end: &str,
    limit: usize,
) -> trc::Result<Vec<(String, T)>> {
    let mut items = Vec::new();
    store
        .iterate(IterateParams::new(key(start), key(end)), |k, v| {
            let k =
                std::str::from_utf8(k).map_err(|_| trc::StoreEvent::DataCorruption.into_err())?;
            items.push((k.to_owned(), decode(v)?));
            Ok(items.len() < limit)
        })
        .await?;
    Ok(items)
}
