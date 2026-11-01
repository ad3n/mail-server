// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL

use common::{
    Server,
    auth::{AccessToken, oauth::scope::OAuthScopes},
    delivery_tracking::*,
};
use http_proto::{HttpResponse, JsonResponse, ToHttpResponse};
use hyper::{Method, StatusCode};
use registry::schema::enums::Permission;
use serde::Deserialize;
use store::{
    rand::{RngExt, distr::Alphanumeric, rng},
    write::{BatchBuilder, assert::AssertValue},
};
use utils::outbound::OutboundPolicy;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppInput {
    name: String,
    webhook_url: String,
}

impl AppInput {
    fn parse(body: Option<&[u8]>) -> Option<Self> {
        let mut input: Self = serde_json::from_slice(body?).ok()?;
        input.name = input.name.trim().to_owned();
        if input.name.is_empty()
            || input.name.chars().count() > 128
            || input.name.chars().any(char::is_control)
        {
            return None;
        }
        (input.webhook_url.len() <= 2048
            && OutboundPolicy::PublicOnly
                .parse_url(&input.webhook_url)
                .is_ok())
        .then_some(input)
    }
}

fn generate_id(prefix: &str) -> String {
    let suffix: String = rng()
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    format!("{prefix}{suffix}")
}

fn issue_credentials(batch: &mut BatchBuilder, app_id: &str) -> AppCredentials {
    let credentials = AppCredentials {
        api_key: generate_id("key_"),
        token: generate_id("tok_"),
    };
    let index = api_key_index(&credentials.api_key);
    batch.assert_value(class(&index), AssertValue::None);
    put(batch, &index, &app_id);
    put(batch, &credential_key(app_id), &credentials);
    credentials
}

fn revoke_credentials(batch: &mut BatchBuilder, credentials: &AppCredentials) {
    let index = class(&api_key_index(&credentials.api_key));
    batch
        .assert_value(index.clone(), AssertValue::Some)
        .clear(index);
}

pub async fn lookup(
    server: &Server,
    external_id: &str,
    api_key: &str,
    token: &str,
) -> trc::Result<HttpResponse> {
    let Some(app_id) = authenticate_app(server.store(), api_key, token).await? else {
        return Ok(HttpResponse::new(StatusCode::UNAUTHORIZED).with_no_store());
    };
    if !valid_id(external_id) {
        return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
    }
    let prefix = result_prefix(&app_id, external_id);
    let results: Vec<DeliveryEvent> =
        delivery_range(server.store(), &prefix, &format!("{prefix}~"), usize::MAX)
            .await?
            .into_iter()
            .map(|(_, event)| event)
            .collect();
    if results.is_empty() {
        return Ok(HttpResponse::new(StatusCode::NOT_FOUND).with_no_store());
    }
    Ok(JsonResponse::new(results).no_cache().into_http_response())
}

pub(super) fn is_admin(token: AccessToken) -> trc::Result<bool> {
    let token = token.assert_allows_oauth_scopes(OAuthScopes::MAIL)?;
    let admin = token.has_permission(Permission::SysSystemSettingsUpdate);
    #[cfg(feature = "enterprise")]
    let admin = admin && token.tenant_id().is_none();
    Ok(admin)
}

pub async fn handle(
    server: &Server,
    token: AccessToken,
    path: &[&str],
    method: &Method,
    body: Option<&[u8]>,
) -> trc::Result<HttpResponse> {
    let admin = is_admin(token)?;
    let forbidden = || HttpResponse::new(StatusCode::FORBIDDEN);
    let missing = || HttpResponse::new(StatusCode::NOT_FOUND);
    if !admin {
        return Ok(forbidden());
    }
    if path == ["apps"] {
        if method == Method::POST {
            let Some(input) = AppInput::parse(body) else {
                return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
            };
            let app = IntegratedApp {
                app_id: generate_id("app_"),
                name: input.name,
                webhook_url: input.webhook_url,
            };
            let key = app_key(&app.app_id);
            let mut batch = BatchBuilder::new();
            batch.assert_value(class(&key), AssertValue::None);
            let credentials = issue_credentials(&mut batch, &app.app_id);
            put(&mut batch, &key, &app);
            server.store().write_batch(&mut batch).await?;
            return Ok(JsonResponse::with_status(StatusCode::CREATED, app)
                .no_cache()
                .into_http_response()
                .with_header("app-key", credentials.api_key)
                .with_header("app-token", credentials.token));
        }
        if method != Method::GET {
            return Ok(HttpResponse::new(StatusCode::METHOD_NOT_ALLOWED));
        }
        let prefix = apps_prefix();
        let apps: Vec<IntegratedApp> =
            delivery_range(server.store(), &prefix, &format!("{prefix}~"), usize::MAX)
                .await?
                .into_iter()
                .map(|(_, v)| v)
                .collect();
        return Ok(JsonResponse::new(apps).no_cache().into_http_response());
    }
    let ["apps", id, rest @ ..] = path else {
        return Ok(missing());
    };
    if !valid_id(id) {
        return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
    }
    let key = app_key(id);
    let current: Option<IntegratedApp> = delivery_get(server.store(), &key).await?;
    if rest == ["credentials"] && method == Method::GET && current.is_some() {
        let credentials =
            delivery_get::<AppCredentials>(server.store(), &credential_key(id)).await?;
        return Ok(match credentials {
            Some(c) => JsonResponse::new(c).no_cache().into_http_response(),
            None => missing(),
        });
    }
    if rest == ["credentials", "regenerate"] && method == Method::POST && current.is_some() {
        let Some(previous) =
            delivery_get::<AppCredentials>(server.store(), &credential_key(id)).await?
        else {
            return Ok(missing());
        };
        let mut batch = BatchBuilder::new();
        batch.assert_value(class(&key), AssertValue::Some);
        revoke_credentials(&mut batch, &previous);
        let credentials = issue_credentials(&mut batch, id);
        return match server.store().write_batch(&mut batch).await {
            Ok(_) => Ok(JsonResponse::new(credentials)
                .no_cache()
                .into_http_response()),
            Err(e) if e.is_assertion_failure() => Ok(HttpResponse::new(StatusCode::CONFLICT)),
            Err(e) => Err(e),
        };
    }
    if !rest.is_empty() {
        return Ok(missing());
    }
    let mut batch = BatchBuilder::new();
    if current.is_some() {
        batch.assert_value(class(&key), AssertValue::Some);
    }
    match *method {
        Method::GET => {
            return Ok(match current {
                Some(app) => JsonResponse::new(app).no_cache().into_http_response(),
                None => missing(),
            });
        }
        Method::DELETE if admin => {
            if current.is_none() {
                return Ok(missing());
            }
            if let Some(credentials) =
                delivery_get::<AppCredentials>(server.store(), &credential_key(id)).await?
            {
                revoke_credentials(&mut batch, &credentials);
            }
            batch.clear(class(&key)).clear(class(&credential_key(id)));
        }
        Method::PUT => {
            let Some(input) = AppInput::parse(body) else {
                return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
            };
            let Some(mut app) = current else {
                return Ok(missing());
            };
            app.name = input.name;
            app.webhook_url = input.webhook_url;
            put(&mut batch, &key, &app);
        }
        Method::POST => return Ok(HttpResponse::new(StatusCode::METHOD_NOT_ALLOWED)),
        _ => return Ok(forbidden()),
    }
    match server.store().write_batch(&mut batch).await {
        Ok(_) => Ok(HttpResponse::new(StatusCode::NO_CONTENT)),
        Err(e) if e.is_assertion_failure() => Ok(HttpResponse::new(StatusCode::CONFLICT)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{Caches, Core, Data, Inner, config::storage::Storage, manager::boot::build_ipc};
    use smtp::queue::{
        Error, ErrorDetails, MessageSource, Metadata, Recipient, Status, spool::SmtpSpool,
        tracking::Tracking,
    };
    use std::sync::Arc;
    use store::{RegistryStore, registry::bootstrap::Bootstrap};

    #[tokio::test]
    async fn tracked_delivery_survives_queue_removal_and_is_scoped_to_app() {
        let registry = RegistryStore::init(
            std::env::temp_dir().join(format!("tracking-test-{}-absent.json", std::process::id())),
            false,
        )
        .await
        .unwrap();
        let mut bootstrap = Bootstrap::new(registry).await;
        let storage = Storage::parse(&mut bootstrap).await;
        let core: Arc<Core> = Core::parse(&mut bootstrap, storage).await.into();
        let data = Data::parse(&mut bootstrap).await;
        let (cache, _rx) = Caches::parse(&mut bootstrap, &core.storage).await;
        let (ipc, _receivers) = build_ipc(false);
        let server = Server {
            inner: Arc::new(Inner {
                shared_core: core.clone().into(),
                data,
                cache,
                ipc,
            }),
            core,
        };
        let admin = || AccessToken::from_permissions(1, [Permission::SysSystemSettingsUpdate]);
        let other = || AccessToken::from_permissions(3, []);
        let app = br#"{"name":"Billing","webhookUrl":"https://billing.example.com/callback"}"#;
        let create = || handle(&server, admin(), &["apps"], &Method::POST, Some(app));
        let response = create().await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let http_proto::HttpResponseBody::Text(body) = response.body() else {
            panic!("expected JSON")
        };
        let registered: IntegratedApp = serde_json::from_str(body).unwrap();
        let id = registered.app_id.as_str();
        assert!(id.starts_with("app_") && valid_id(id));
        let app_path = ["apps", id];
        let headers = response.headers().unwrap();
        let api_key = headers["app-key"].to_str().unwrap();
        let api_token = headers["app-token"].to_str().unwrap();
        assert_eq!(
            handle(&server, admin(), &app_path, &Method::POST, Some(app))
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            handle(&server, other(), &app_path, &Method::PUT, Some(app))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            handle(&server, admin(), &app_path, &Method::PUT, Some(app))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );

        let credentials_path = ["apps", registered.app_id.as_str(), "credentials"];
        let revealed = handle(&server, admin(), &credentials_path, &Method::GET, None)
            .await
            .unwrap();
        let http_proto::HttpResponseBody::Text(json) = revealed.body() else {
            panic!("expected credentials")
        };
        let reread: AppCredentials = serde_json::from_str(json).unwrap();
        assert_eq!(reread.api_key, api_key);
        assert_eq!(reread.token, api_token);
        assert_eq!(
            handle(&server, other(), &credentials_path, &Method::GET, None)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut msg = server.new_message("from@example.com", MessageSource::Authenticated, 0);
        let id = msg.queue_id;
        msg.message.metadata = vec![Metadata::Tracking(Box::new(Tracking {
            app_id: registered.app_id.clone(),
            external_id: "00042".into(),
        }))]
        .into_boxed_slice();
        msg.message.recipients = vec![
            Recipient::new("one@example.com"),
            Recipient::new("two@example.com"),
        ];
        assert!(msg.save_changes(&server, None, None).await);
        let mut msg = server
            .read_message(id, Default::default())
            .await
            .unwrap()
            .unwrap();
        msg.message.recipients[0].status = Status::PermanentFailure(Box::new(ErrorDetails {
            entity: "mx".into(),
            details: Error::Io("unknown recipient".into()),
        }));
        assert!(msg.save_changes(&server, None, None).await);
        let msg = server
            .read_message(id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert!(msg.remove(&server, None).await);
        assert!(
            server
                .read_message(id, Default::default())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            lookup(&server, "00042", api_key, "wrong")
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let before_rotation = lookup(&server, "00042", api_key, api_token).await.unwrap();
        assert_eq!(before_rotation.status(), StatusCode::OK);
        let http_proto::HttpResponseBody::Text(original_results) = before_rotation.body() else {
            panic!("expected delivery results");
        };
        let second = create().await.unwrap();
        let headers = second.headers().unwrap();
        let other_key = headers["app-key"].to_str().unwrap();
        let other_token = headers["app-token"].to_str().unwrap();
        assert_eq!(
            lookup(&server, "00042", other_key, other_token)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            lookup(&server, "00042", other_key, api_token)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let rotation_path = [
            "apps",
            registered.app_id.as_str(),
            "credentials",
            "regenerate",
        ];
        assert_eq!(
            handle(&server, other(), &rotation_path, &Method::POST, None)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let rotated = handle(&server, admin(), &rotation_path, &Method::POST, None)
            .await
            .unwrap();
        assert_eq!(rotated.status(), StatusCode::OK);
        let http_proto::HttpResponseBody::Text(json) = rotated.body() else {
            panic!("expected regenerated credentials")
        };
        let fresh: AppCredentials = serde_json::from_str(json).unwrap();
        assert_ne!(fresh.api_key, api_key);
        assert_ne!(fresh.token, api_token);
        for (key, token) in [
            (api_key, api_token),
            (&fresh.api_key, api_token),
            (api_key, &fresh.token),
        ] {
            assert_eq!(
                lookup(&server, "00042", key, token).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            authenticate_app(server.store(), &fresh.api_key, &fresh.token)
                .await
                .unwrap()
                .as_deref(),
            Some(registered.app_id.as_str())
        );
        let after_rotation = lookup(&server, "00042", &fresh.api_key, &fresh.token)
            .await
            .unwrap();
        assert_eq!(after_rotation.status(), StatusCode::OK);
        let http_proto::HttpResponseBody::Text(current_results) = after_rotation.body() else {
            panic!("expected delivery results after credential rotation");
        };
        assert_eq!(current_results, original_results);
        let reread: AppCredentials =
            delivery_get(server.store(), &credential_key(&registered.app_id))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            (reread.api_key, reread.token),
            (fresh.api_key.clone(), fresh.token.clone())
        );
        let prefix = result_prefix(&registered.app_id, "00042");
        let rows =
            delivery_range::<DeliveryEvent>(server.store(), &prefix, &format!("{prefix}~"), 10)
                .await
                .unwrap();
        assert_eq!(
            rows.iter()
                .map(|(_, e)| e.status.as_str())
                .collect::<Vec<_>>(),
            ["failed", "removed"]
        );
        let jobs = delivery_range::<DeliveryEvent>(
            server.store(),
            "delivery/outbox/",
            "delivery/outbox/~",
            10,
        )
        .await
        .unwrap();
        assert_eq!(jobs.len(), 4);
        assert!(
            jobs.iter()
                .all(|(_, c)| c.external_id == "00042" && c.app_id == registered.app_id)
        );
        assert_eq!(
            handle(&server, admin(), &app_path, &Method::DELETE, None)
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let response = create().await.unwrap();
        let http_proto::HttpResponseBody::Text(body) = response.body() else {
            panic!("expected JSON")
        };
        assert_ne!(
            serde_json::from_str::<IntegratedApp>(body).unwrap().app_id,
            registered.app_id
        );
        assert_eq!(
            lookup(&server, "00042", &fresh.api_key, &fresh.token)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
