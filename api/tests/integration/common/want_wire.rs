//! Reading what a want job published for Kobload, off the JOBS stream.

use super::TestServer;
use serde_json::{Value, json};

/// Point Kobload config at nothing real — the want handlers only need
/// `kobload_ingest_url` set to get as far as publishing.
pub async fn configure_kobload(server: &TestServer) {
    configure_kobload_at(server, "http://localhost:9999").await;
}

pub async fn configure_kobload_at(server: &TestServer, base_url: &str) {
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": base_url,
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token),
            }),
        )
        .await
        .assert_ok();
}

/// The JOBS stream is a work queue, so a second filtered consumer on
/// `jobs.kobload.want_request` is rejected outright and anything acked is gone.
/// `allow_direct` is on, so these read raw messages by sequence instead: note
/// the tip, publish, then walk the new sequences. That also keeps the two tests
/// here from reading each other's messages when the suite runs in parallel.
pub async fn jobs_stream(server: &TestServer) -> async_nats::jetstream::stream::Stream {
    server
        .app_state
        .nats
        .client
        .jetstream()
        .get_stream("JOBS")
        .await
        .expect("JOBS stream")
}

pub async fn stream_tip(server: &TestServer) -> u64 {
    jobs_stream(server)
        .await
        .info()
        .await
        .expect("JOBS stream info")
        .state
        .last_sequence
}

pub async fn want_requests_since(
    server: &TestServer,
    after_seq: u64,
    source_id: &str,
) -> Vec<Value> {
    kobload_jobs_since(server, after_seq, "want_request", source_id).await
}

pub async fn kobload_jobs_since(
    server: &TestServer,
    after_seq: u64,
    job_type: &str,
    source_id: &str,
) -> Vec<Value> {
    kobload_jobs_of_type_since(server, after_seq, job_type)
        .await
        .into_iter()
        .filter(|payload| payload["source_id"] == json!(source_id))
        .collect()
}

pub async fn kobload_jobs_of_type_since(
    server: &TestServer,
    after_seq: u64,
    job_type: &str,
) -> Vec<Value> {
    let subject = format!("jobs.kobload.{job_type}");
    let mut stream = jobs_stream(server).await;
    let tip = stream
        .info()
        .await
        .expect("JOBS stream info")
        .state
        .last_sequence;

    let mut out = Vec::new();
    for seq in (after_seq + 1)..=tip {
        // A sequence a worker already consumed is gone from a work queue; that
        // is a hole to step over, not a failure.
        let Ok(raw) = stream.get_raw_message(seq).await else {
            continue;
        };
        if raw.subject.as_str() != subject {
            continue;
        }
        let Ok(job) = serde_json::from_slice::<Value>(&raw.payload) else {
            continue;
        };
        out.push(job["payload"].clone());
    }
    out
}
