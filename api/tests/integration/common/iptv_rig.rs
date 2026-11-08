use super::TestServer;
use api::repositories::{iptv_channels, iptv_providers, iptv_sources};
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A channel whose one source is a provider serving `stream` for every live URL.
pub async fn iptv_channel(server: &TestServer, provider: &MockServer, stream: &[u8]) -> Uuid {
    Mock::given(method("GET"))
        .and(path_regex("/live/.*\\.ts$"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream.to_vec(), "video/mp2t"))
        .mount(provider)
        .await;
    let provider_row = iptv_providers::create(
        &server.pool,
        iptv_providers::CreateProvider {
            name: format!("p-{}", Uuid::new_v4()),
            url: provider.uri(),
            username: Uuid::new_v4().to_string(),
            password: "p".into(),
            preferred_format: Some("ts".into()),
            sync_interval_secs: None,
        },
    )
    .await
    .unwrap();
    let channel = iptv_channels::create(
        &server.pool,
        iptv_channels::CreateChannel {
            name: "Fields".into(),
            normalized_name: format!("fields {}", Uuid::new_v4()),
            external_id: None,
            language: None,
            country: None,
            category: None,
            icon_url: None,
            epg_channel_id: None,
        },
    )
    .await
    .unwrap();
    iptv_sources::upsert(
        &server.pool,
        iptv_sources::UpsertSource {
            channel_id: channel.id,
            provider_id: provider_row.id,
            remote_stream_id: 1,
            remote_category_id: None,
            remote_category_name: None,
            name: "Fields".into(),
            icon_url: None,
            epg_channel_id: None,
            stream_type: None,
            quality: None,
            priority: None,
            match_method: None,
            match_confidence: None,
            metadata: None,
        },
    )
    .await
    .unwrap();
    channel.id
}
