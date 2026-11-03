// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL

use super::{Message, MessageWrapper, Metadata, Status};
use common::Server;
use common::config::smtp::queue::QueueName;
use common::delivery_tracking::{
    DeliveryEvent, DeliveryEventData, callback_key, class, encode, result_prefix,
};
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
    ) -> trc::Result<()> {
        let Some(t) = self.message.tracking() else {
            return Ok(());
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
            let event = DeliveryEvent {
                created_at: Some(self.message.created),
                updated_at: timestamp,
                delivery: DeliveryEventData {
                    event_id: format!(
                        "{:x}-{i}-{:016x}",
                        self.queue_id,
                        server.inner.data.queue_id_gen.generate()
                    ),
                    app_id: t.app_id.clone(),
                    external_id: t.external_id.clone(),
                    queue_id: format!("{:x}", self.queue_id),
                    recipient: rcpt.address.to_string(),
                    status: status.into(),
                },
            };
            let result = format!(
                "{}{:016x}/{i}",
                result_prefix(&t.app_id, &t.external_id),
                self.queue_id
            );
            let encoded = encode(&event)?;
            batch.set(class(&result), encoded.clone());
            batch.set(
                class(&callback_key(timestamp, &event.delivery.event_id)),
                encoded,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[derive(rkyv::Serialize, rkyv::Archive)]
    enum LegacyMetadata {
        QueueSize { key: u128, id: u64 },
        QueueCount { key: u128, id: u64 },
        Headers { value: Box<[u8]>, id: u64 },
        Submission { account_id: u32 },
        Tracking(Box<Tracking>),
    }

    #[test]
    fn reads_metadata_archived_before_recipient_blobs_were_added() {
        let old = vec![
            LegacyMetadata::QueueSize {
                key: u128::MAX,
                id: 5,
            },
            LegacyMetadata::QueueCount { key: 20, id: 6 },
            LegacyMetadata::Headers {
                value: b"Subject: Legacy\r\n".to_vec().into_boxed_slice(),
                id: 7,
            },
            LegacyMetadata::Submission { account_id: 42 },
            LegacyMetadata::Tracking(Box::new(Tracking {
                app_id: "legacy-app".into(),
                external_id: "legacy-id".into(),
            })),
        ];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&old).unwrap();
        let restored = rkyv::from_bytes::<Vec<Metadata>, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(
            restored,
            vec![
                Metadata::QueueSize {
                    key: u128::MAX,
                    id: 5
                },
                Metadata::QueueCount { key: 20, id: 6 },
                Metadata::Headers {
                    value: b"Subject: Legacy\r\n".to_vec().into_boxed_slice(),
                    id: 7
                },
                Metadata::Submission { account_id: 42 },
                Metadata::Tracking(Box::new(Tracking {
                    app_id: "legacy-app".into(),
                    external_id: "legacy-id".into(),
                })),
            ]
        );
    }
}
