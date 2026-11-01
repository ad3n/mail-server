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
            http_client_builder(false),
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
                        if let Ok(Err(err)) = result {
                            trc::error!(err);
                        }
                    }
                    if full_batch {
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
    let lease = callback_key(now() + 60, &callback.event_id);
    let mut claim = BatchBuilder::new();
    claim
        .assert_value(class(&key), AssertValue::Some)
        .clear(class(&key));
    put(&mut claim, &lease, &callback);
    match store.write_batch(&mut claim).await {
        Err(e) if e.is_assertion_failure() => return Ok(()),
        result => {
            result?;
        }
    }

    if let Some(app) = delivery_get::<IntegratedApp>(store, &app_key(&callback.app_id)).await? {
        let credentials = delivery_get::<AppCredentials>(store, &credential_key(&app.app_id))
            .await?
            .ok_or_else(|| {
                trc::StoreEvent::DataCorruption
                    .into_err()
                    .details("Delivery webhook credentials missing")
            })?;
        let sent = match client.request(reqwest::Method::POST, &app.webhook_url) {
            Ok(request) => request
                .timeout(Duration::from_secs(10))
                .header("Content-Type", "application/json")
                .header("app-key", &credentials.api_key)
                .header("app-token", &credentials.token)
                .body(serde_json::to_vec(&callback).expect("serializable delivery event"))
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
                NextRetry = trc::Value::Timestamp(now() + 60)
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
            for (status, key, token) in [
                ("503 Service Unavailable", "test-key", "test-token"),
                ("204 No Content", "rotated-key", "rotated-token"),
            ] {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut length = 0;
                let mut authenticated = (false, false);
                loop {
                    let mut line = String::new();
                    assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    match line.trim_end().split_once(": ") {
                        Some((k, v)) if k.eq_ignore_ascii_case("app-key") && v == key => {
                            authenticated.0 = true
                        }
                        Some((k, v)) if k.eq_ignore_ascii_case("app-token") && v == token => {
                            authenticated.1 = true
                        }
                        _ => {}
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                assert_eq!(authenticated, (true, true));
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                bodies.push(body);
                socket
                    .get_mut()
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
        put(&mut batch, &app_key("sales"), &app);
        put(
            &mut batch,
            &credential_key("sales"),
            &AppCredentials {
                api_key: "test-key".into(),
                token: "test-token".into(),
            },
        );
        let key = callback_key(0, "unique");
        put(&mut batch, &key, &callback);
        store.write_batch(&mut batch).await.unwrap();
        let client = OutboundClient::new(
            http_client_builder(false).no_proxy(),
            OutboundPolicy::Trusted,
            Redirects::None,
        );
        deliver(&store, &client, key, callback).await.unwrap();
        let mut pending =
            delivery_range::<DeliveryEvent>(&store, "delivery/outbox/", "delivery/outbox/~", 10)
                .await
                .unwrap();
        assert_eq!(pending.len(), 1);
        let (key, job) = pending.pop().unwrap();
        let mut rotation = BatchBuilder::new();
        put(
            &mut rotation,
            &credential_key("sales"),
            &AppCredentials {
                api_key: "rotated-key".into(),
                token: "rotated-token".into(),
            },
        );
        store.write_batch(&mut rotation).await.unwrap();
        deliver(&store, &client, key, job).await.unwrap();
        let bodies = receiver.await.unwrap();
        assert_eq!(bodies[0], bodies[1]);
        let payload: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
        assert_eq!(payload["externalId"], "00042");
        assert_eq!(payload["eventId"], "unique");
        assert!(payload.get("events").is_none());
        assert!(
            delivery_range::<DeliveryEvent>(&store, "delivery/outbox/", "delivery/outbox/~", 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
