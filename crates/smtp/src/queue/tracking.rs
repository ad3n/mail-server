// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL

use super::{Message, MessageWrapper, Metadata, Status};
use common::Server;
use common::config::smtp::queue::QueueName;
use common::delivery_tracking::{DeliveryEvent, callback_key, put, result_prefix};
use store::write::{BatchBuilder, now};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    rkyv::Serialize,
    rkyv::Deserialize,
    rkyv::Archive,
    serde::Deserialize,
)]
pub struct Tracking {
    pub app_id: String,
    pub external_id: String,
}

impl Message {
    pub fn tracking(&self) -> Option<&Tracking> {
        self.metadata.iter().find_map(|item| match item {
            Metadata::Tracking(t) => Some(t.as_ref()),
            _ => None,
        })
    }

    pub fn delivery_snapshot(&self) -> Vec<(QueueName, &'static str)> {
        if self.tracking().is_none() {
            return Vec::new();
        }
        self.recipients
            .iter()
            .map(|r| (r.queue, label(&r.status)))
            .collect()
    }
}

fn label<T, E>(status: &Status<T, E>) -> &'static str {
    match status {
        Status::Scheduled => "queued",
        Status::TemporaryFailure(_) => "deferred",
        Status::PermanentFailure(_) => "failed",
        Status::Completed(_) => "accepted",
    }
}

impl MessageWrapper {
    pub fn record_tracking(
        &self,
        server: &Server,
        batch: &mut BatchBuilder,
        removing: bool,
        queue: Option<QueueName>,
    ) {
        let Some(t) = self.message.tracking() else {
            return;
        };
        let timestamp = now();
        for (i, rcpt) in self.message.recipients.iter().enumerate() {
            let mut status = label(&rcpt.status);
            if removing && matches!(status, "queued" | "deferred") {
                status = "removed";
            }
            if let Some(&(original_queue, previous)) = self.previous_status.get(i)
                && (previous == status || queue.is_some_and(|q| q != original_queue))
            {
                continue;
            }
            let (smtp_code, hostname) = match &rcpt.status {
                Status::Completed(r) => (Some(r.response.code), Some(r.hostname.to_string())),
                Status::TemporaryFailure(e) | Status::PermanentFailure(e) => {
                    let code = match &e.details {
                        super::Error::UnexpectedResponse(r) => Some(r.response.code),
                        _ => None,
                    };
                    (code, None)
                }
                Status::Scheduled => (None, None),
            };
            let event = DeliveryEvent {
                smtp_code,
                hostname,
                event_id: format!(
                    "{:x}-{i}-{:016x}",
                    self.queue_id,
                    server.inner.data.queue_id_gen.generate()
                ),
                app_id: t.app_id.clone(),
                external_id: t.external_id.clone(),
                queue_id: format!("{:x}", self.queue_id),
                recipient: rcpt.address.to_string(),
                updated_at: timestamp,
                status: status.into(),
                details: match &rcpt.status {
                    Status::Completed(r) => format!("{} {}", r.response.code, r.response.message),
                    Status::TemporaryFailure(e) | Status::PermanentFailure(e) => {
                        e.details.to_string()
                    }
                    Status::Scheduled => String::new(),
                },
            };
            let result = format!(
                "{}{:016x}/{i}",
                result_prefix(&t.app_id, &t.external_id),
                self.queue_id
            );
            put(batch, &result, &event);
            put(batch, &callback_key(timestamp, &event.event_id), &event);
        }
    }
}
