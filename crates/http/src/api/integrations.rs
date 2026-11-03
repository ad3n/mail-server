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
    #[serde(default)]
    webhook_auth: Option<WebhookAuth>,
    #[serde(default)]
    webhook_secrets: WebhookSecrets,
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

    fn into_record(self, id: &str, current: Option<StoredApp>) -> Option<StoredApp> {
        let (old_auth, old_secrets, same_url) = match current {
            Some(old) => (
                old.app.webhook_auth,
                old.webhook_secrets,
                old.app.webhook_url == self.webhook_url,
            ),
            None => (WebhookAuth::None, WebhookSecrets::default(), false),
        };
        let auth = self.webhook_auth.unwrap_or_else(|| old_auth.clone());
        let mut secrets = self.webhook_secrets;
        if same_url && std::mem::discriminant(&auth) == std::mem::discriminant(&old_auth) {
            if secrets.password.is_empty() {
                secrets.password = old_secrets.password;
            }
            if secrets.api_key.is_empty() {
                secrets.api_key = old_secrets.api_key;
            }
            if secrets.token.is_empty() {
                secrets.token = old_secrets.token;
            }
        }
        if (!matches!(auth, WebhookAuth::Basic { .. }) && !secrets.password.is_empty())
            || (!matches!(
                auth,
                WebhookAuth::ApiKey { .. } | WebhookAuth::ApiKeyToken { .. }
            ) && !secrets.api_key.is_empty())
            || (!matches!(auth, WebhookAuth::Bearer | WebhookAuth::ApiKeyToken { .. })
                && !secrets.token.is_empty())
        {
            return None;
        }
        auth.headers(&secrets)?;
        Some(StoredApp {
            app: IntegratedApp {
                app_id: id.into(),
                name: self.name,
                webhook_url: self.webhook_url,
                webhook_auth: auth,
            },
            webhook_secrets: secrets,
        })
    }
}

fn generate_id() -> String {
    rng()
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn issue_credentials(batch: &mut BatchBuilder, app_id: &str) -> trc::Result<AppCredentials> {
    let credentials = AppCredentials {
        api_key: generate_id(),
        token: generate_id(),
    };
    let index = api_key_index(&credentials.api_key);
    batch.assert_value(class(&index), AssertValue::None);
    put(batch, &index, &app_id)?;
    put(batch, &credential_key(app_id), &credentials)?;
    Ok(credentials)
}

fn revoke_credentials(batch: &mut BatchBuilder, credentials: &AppCredentials) {
    let index = class(&api_key_index(&credentials.api_key));
    batch
        .assert_value(index.clone(), AssertValue::Some)
        .clear(index);
}

pub(super) fn apps_page_origin(headers: &hyper::HeaderMap, authority: &str) -> Option<String> {
    if headers.get("sec-fetch-site")?.to_str().ok()? != "same-origin" {
        return None;
    }
    let page: hyper::Uri = headers
        .get(hyper::header::REFERER)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    let scheme = page.scheme_str()?;
    let path = page.path().strip_prefix("/admin/Management/x:App")?;
    if !matches!(scheme, "http" | "https")
        || !page.authority()?.as_str().eq_ignore_ascii_case(authority)
        || !(path.is_empty() || path.strip_prefix('/').is_some_and(valid_id))
    {
        return None;
    }
    let origin = format!("{scheme}://{}", page.authority()?);
    if let Some(source) = headers.get(hyper::header::ORIGIN)
        && source.to_str().ok()? != origin
    {
        return None;
    }
    Some(origin)
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
    let results: Vec<DeliveryStatus> =
        delivery_range::<DeliveryEvent>(server.store(), &prefix, &format!("{prefix}~"), usize::MAX)
            .await?
            .into_iter()
            .map(|(_, event)| event.into_status())
            .collect::<trc::Result<_>>()?;
    if results.is_empty() {
        return Ok(HttpResponse::new(StatusCode::NOT_FOUND).with_no_store());
    }
    Ok(JsonResponse::new(results).no_cache().into_http_response())
}

fn is_admin(token: AccessToken) -> trc::Result<bool> {
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
            let id = generate_id();
            let Some(record) = input.into_record(&id, None) else {
                return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
            };
            let key = app_key(&id);
            let mut batch = BatchBuilder::new();
            batch.assert_value(class(&key), AssertValue::None);
            let credentials = issue_credentials(&mut batch, &id)?;
            put(&mut batch, &key, &record)?;
            server.store().write_batch(&mut batch).await?;
            return Ok(JsonResponse::with_status(StatusCode::CREATED, record.app)
                .no_cache()
                .into_http_response()
                .with_header("app-key", credentials.api_key)
                .with_header("app-token", credentials.token));
        }
        if method != Method::GET {
            return Ok(HttpResponse::new(StatusCode::METHOD_NOT_ALLOWED));
        }
        let prefix = apps_prefix();
        let apps: Vec<IntegratedApp> = delivery_range::<IntegratedApp>(
            server.store(),
            &prefix,
            &format!("{prefix}~"),
            usize::MAX,
        )
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
    let Some(current) = delivery_get::<StoredApp>(server.store(), &key).await? else {
        return Ok(missing());
    };
    if rest == ["credentials"] && method == Method::GET {
        let credentials =
            delivery_get::<AppCredentials>(server.store(), &credential_key(id)).await?;
        return Ok(match credentials {
            Some(c) => JsonResponse::new(c).no_cache().into_http_response(),
            None => missing(),
        });
    }
    if rest == ["credentials", "regenerate"] && method == Method::POST {
        let Some(previous) =
            delivery_get::<AppCredentials>(server.store(), &credential_key(id)).await?
        else {
            return Ok(missing());
        };
        let mut batch = BatchBuilder::new();
        batch.assert_value(class(&key), AssertValue::Some);
        revoke_credentials(&mut batch, &previous);
        let credentials = issue_credentials(&mut batch, id)?;
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
    batch.assert_value(class(&key), AssertValue::Some);
    match *method {
        Method::GET => {
            return Ok(JsonResponse::new(current.app)
                .no_cache()
                .into_http_response());
        }
        Method::DELETE if admin => {
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
            let Some(record) = input.into_record(id, Some(current)) else {
                return Ok(HttpResponse::new(StatusCode::BAD_REQUEST));
            };
            put(&mut batch, &key, &record)?;
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

    #[test]
    fn management_requires_same_origin_apps_page() {
        use hyper::header::{HeaderMap, HeaderValue, ORIGIN, REFERER};
        let mut headers = HeaderMap::new();
        let host = "localhost:8080";
        headers.insert(
            REFERER,
            HeaderValue::from_static("http://localhost:8080/admin/Management/x:App?edit=1"),
        );
        assert!(apps_page_origin(&headers, host).is_none());
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert_eq!(
            apps_page_origin(&headers, host).as_deref(),
            Some("http://localhost:8080")
        );
        for path in ["/new", "/app-123"] {
            headers.insert(
                REFERER,
                format!("http://localhost:8080/admin/Management/x:App{path}")
                    .parse()
                    .unwrap(),
            );
            assert!(apps_page_origin(&headers, host).is_some());
        }
        for path in ["Extra", "/id/more", "/.."] {
            headers.insert(
                REFERER,
                format!("http://localhost:8080/admin/Management/x:App{path}")
                    .parse()
                    .unwrap(),
            );
            assert!(apps_page_origin(&headers, host).is_none());
        }
        headers.insert(
            REFERER,
            HeaderValue::from_static("http://localhost:8080/admin/Management/x:App/new"),
        );
        assert!(apps_page_origin(&headers, "elsewhere.example").is_none());
        headers.insert(ORIGIN, HeaderValue::from_static("http://elsewhere.example"));
        assert!(apps_page_origin(&headers, host).is_none());
        headers.remove(ORIGIN);
        headers.insert(
            REFERER,
            HeaderValue::from_static("http://localhost:8080/admin/Management/x:Role"),
        );
        assert!(apps_page_origin(&headers, host).is_none());
    }

    #[test]
    fn webhook_auth_headers_and_write_only_secrets() {
        let configured = |auth: serde_json::Value, secrets: serde_json::Value| {
            let body = serde_json::json!({"name":"Billing","webhookUrl":"https://example.com/hook","webhookAuth":auth,"webhookSecrets":secrets});
            AppInput::parse(Some(&serde_json::to_vec(&body).unwrap()))
                .unwrap()
                .into_record("id", None)
        };
        for (auth, secrets, key, val) in [
            (
                serde_json::json!({"type":"basic","username":"user"}),
                serde_json::json!({"password":"pass"}),
                "authorization",
                "Basic dXNlcjpwYXNz",
            ),
            (
                serde_json::json!({"type":"bearer"}),
                serde_json::json!({"token":"hook-secret"}),
                "authorization",
                "Bearer hook-secret",
            ),
            (
                serde_json::json!({"type":"apiKey","keyHeader":"X-Receiver"}),
                serde_json::json!({"apiKey":"receiver-secret"}),
                "x-receiver",
                "receiver-secret",
            ),
        ] {
            let record = configured(auth, secrets).unwrap();
            assert_eq!(
                record
                    .app
                    .webhook_auth
                    .headers(&record.webhook_secrets)
                    .unwrap()[key],
                val
            );
            let view = serde_json::to_value(&record.app).unwrap();
            assert!(view.get("webhookSecrets").is_none());
            let unchanged = AppInput::parse(Some(
                br#"{"name":"New","webhookUrl":"https://example.com/hook"}"#,
            ))
            .unwrap()
            .into_record("id", Some(record))
            .unwrap();
            assert_eq!(
                unchanged
                    .app
                    .webhook_auth
                    .headers(&unchanged.webhook_secrets)
                    .unwrap()[key],
                val
            );
            assert!(
                AppInput::parse(Some(
                    br#"{"name":"New","webhookUrl":"https://another.example.com/hook"}"#
                ))
                .unwrap()
                .into_record("id", Some(unchanged))
                .is_none()
            );
        }
        for name in ["", "Host", "Content-Length", "bad header", "X-Key\r\nOther"] {
            assert!(
                configured(
                    serde_json::json!({"type":"apiKey","keyHeader":name}),
                    serde_json::json!({"apiKey":"abc"})
                )
                .is_none()
            );
        }
        assert!(
            configured(
                serde_json::json!({"type":"apiKeyToken","keyHeader":"X-Key","tokenHeader":"x-key"}),
                serde_json::json!({"apiKey":"k","token":"t"})
            )
            .is_none()
        );
    }

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
        let mut untracked = server.new_message("from@example.com", MessageSource::Authenticated, 0);
        let untracked_id = untracked.queue_id;
        untracked.message.recipients = vec![Recipient::new("recipient@example.com")];
        assert!(untracked.save_changes(&server, None, None).await);
        let untracked = server
            .read_message(untracked_id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert!(untracked.message.tracking().is_none());
        assert!(untracked.previous_status.is_empty());
        assert!(untracked.remove(&server, None).await);
        assert!(
            delivery_range::<serde_json::Value>(server.store(), "delivery/", "delivery/~", 1)
                .await
                .unwrap()
                .is_empty()
        );

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
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric()));
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
        for value in [api_key, api_token, &fresh.api_key, &fresh.token] {
            assert_eq!(value.len(), 32);
            assert!(value.bytes().all(|b| b.is_ascii_alphanumeric()));
        }
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
                .map(|(_, e)| e.delivery.status.as_str())
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
            jobs.iter().all(|(_, c)| c.delivery.external_id == "00042"
                && c.delivery.app_id == registered.app_id)
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
