// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL

use common::{BuildServer, Inner, delivery_tracking::*};
use std::{sync::Arc, time::Duration};
use store::write::{BatchBuilder, assert::AssertValue, now};
use utils::{
    http::http_client_builder,
    outbound::{OutboundClient, OutboundPolicy, Redirects},
};

pub fn spawn(inner: Arc<Inner>) {
    tokio::spawn(async move {
        let client = OutboundClient::new(
            http_client_builder(false).pool_max_idle_per_host(0),
            OutboundPolicy::PublicOnly,
            Redirects::None,
        );
        loop {
            let server = inner.build_server();
            let end = callback_key(now().saturating_add(1), "");
            match delivery_range::<DeliveryEvent>(server.store(), "delivery/outbox/", &end, 32)
                .await
            {
                Ok(callbacks) if !callbacks.is_empty() => {
                    let full_batch = callbacks.len() == 32;
                    let mut workers = tokio::task::JoinSet::new();
                    for (key, callback) in callbacks {
                        let server = server.clone();
                        let client = client.clone();
                        workers.spawn(async move {
                            deliver(server.store(), &client, key, callback).await
                        });
                    }
                    while let Some(result) = workers.join_next().await {
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => {
                                trc::error!(err);
                            }
                            Err(err) => {
                                trc::error!(
                                    trc::EventType::Server(trc::ServerEvent::ThreadError)
                                        .reason(err)
                                );
                            }
                        }
                    }
                    if full_batch {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    trc::error!(e);
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

async fn deliver(
    store: &store::Store,
    client: &OutboundClient,
    key: String,
    callback: DeliveryEvent,
) -> trc::Result<()> {
    // Moving the job grants this worker a lease. Another node can retry after a crash.
    let payload = encode(&callback)?;
    let lease = callback_key(now().saturating_add(60), &callback.event_id);
    let mut claim = BatchBuilder::new();
    claim
        .assert_value(class(&key), AssertValue::Some)
        .clear(class(&key));
    claim.set(class(&lease), payload.clone());
    match store.write_batch(&mut claim).await {
        Err(e) if e.is_assertion_failure() => return Ok(()),
        result => {
            result?;
        }
    }

    drop(claim);

    if let Some(app) = delivery_get::<StoredApp>(store, &app_key(&callback.app_id)).await? {
        let headers = app
            .app
            .webhook_auth
            .headers(&app.webhook_secrets)
            .ok_or_else(|| {
                trc::StoreEvent::DataCorruption
                    .into_err()
                    .details("Invalid webhook authentication settings")
            })?;
        let sent = match client.request(reqwest::Method::POST, &app.app.webhook_url) {
            Ok(request) => request
                .headers(headers)
                .timeout(Duration::from_secs(10))
                .header("Content-Type", "application/json")
                .body(payload)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success()),
            Err(_) => false,
        };
        if !sent {
            trc::event!(
                Telemetry(trc::TelemetryEvent::WebhookError),
                Details = "App delivery callback failed; retry scheduled",
                Id = callback.event_id,
                NextRetry = trc::Value::Timestamp(now().saturating_add(60))
            );
            return Ok(());
        }
    }
    let mut batch = BatchBuilder::new();
    batch.clear(class(&lease));
    store.write_batch(&mut batch).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::backend::ephemeral::EphemeralStore;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    #[tokio::test]
    async fn retry_posts_the_same_single_event_until_acknowledged() {
        let store = EphemeralStore::open();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/status", listener.local_addr().unwrap());
        let receiver = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for status in ["503 Service Unavailable", "200 OK"] {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut length = 0;
                let mut custom_headers = (None, None);
                loop {
                    let mut line = String::new();
                    assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    assert!(!line.contains("test-key"));
                    assert!(!line.contains("test-token"));
                    if let Some((name, value)) = line.split_once(':') {
                        match name.to_ascii_lowercase().as_str() {
                            "x-tenant-key" => custom_headers.0 = Some(value.trim().to_owned()),
                            "x-hook-token" => custom_headers.1 = Some(value.trim().to_owned()),
                            _ => {}
                        }
                        assert!(!name.eq_ignore_ascii_case("app-key"));
                        assert!(!name.eq_ignore_ascii_case("app-token"));
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                if status.starts_with("200") {
                    assert_eq!(
                        custom_headers,
                        (Some("receiver-key".into()), Some("receiver-token".into()))
                    );
                } else {
                    assert_eq!(custom_headers, (None, None));
                }
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                bodies.push(body);
                socket
                    .get_mut()
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 1073741824\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
            bodies
        });
        let app = IntegratedApp {
            app_id: "sales".into(),
            name: "Sales".into(),
            webhook_url: url,
            webhook_auth: WebhookAuth::None,
        };
        let callback = DeliveryEvent {
            event_id: "unique".into(),
            app_id: "sales".into(),
            external_id: "00042".into(),
            queue_id: "abcd".into(),
            recipient: "a@example.com".into(),
            status: "accepted".into(),
            updated_at: 1,
            details: "OK".into(),
            smtp_code: Some(250),
            hostname: None,
        };
        let mut batch = BatchBuilder::new();
        put(&mut batch, &app_key("sales"), &app).unwrap();
        put(
            &mut batch,
            &credential_key("sales"),
            &AppCredentials {
                api_key: "test-key".into(),
                token: "test-token".into(),
            },
        )
        .unwrap();
        let key = callback_key(0, "unique");
        put(&mut batch, &key, &callback).unwrap();
        store.write_batch(&mut batch).await.unwrap();
        let client = OutboundClient::new(
            http_client_builder(false).no_proxy(),
            OutboundPolicy::Trusted,
            Redirects::None,
        );
        let (first, second) = tokio::join!(
            deliver(&store, &client, key.clone(), callback.clone()),
            deliver(&store, &client, key, callback),
        );
        first.unwrap();
        second.unwrap();
        let mut pending =
            delivery_range::<DeliveryEvent>(&store, "delivery/outbox/", "delivery/outbox/~", 10)
                .await
                .unwrap();
        assert_eq!(pending.len(), 1);
        let (key, job) = pending.pop().unwrap();
        let configured = StoredApp {
            app: IntegratedApp {
                webhook_auth: WebhookAuth::ApiKeyToken {
                    key_header: "X-Tenant-Key".into(),
                    token_header: "X-Hook-Token".into(),
                },
                ..app
            },
            webhook_secrets: WebhookSecrets {
                api_key: "receiver-key".into(),
                token: "receiver-token".into(),
                ..Default::default()
            },
        };
        let mut batch = BatchBuilder::new();
        put(&mut batch, &app_key("sales"), &configured).unwrap();
        store.write_batch(&mut batch).await.unwrap();
        deliver(&store, &client, key, job).await.unwrap();
        let bodies = tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bodies[0], bodies[1]);
        let payload: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
        assert_eq!(payload["externalId"], "00042");
        assert_eq!(payload["eventId"], "unique");
        assert!(payload.get("events").is_none());
        let body = std::str::from_utf8(&bodies[0]).unwrap();
        assert!(!body.contains("test-key"));
        assert!(!body.contains("test-token"));
        assert!(
            delivery_range::<DeliveryEvent>(&store, "delivery/outbox/", "delivery/outbox/~", 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
