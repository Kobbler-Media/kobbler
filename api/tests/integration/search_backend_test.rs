use api::search::{
    BookDocument, BookSearchFilters, LivetvGuideDocument, LivetvGuideSearchFilters, MeiliState,
    SearchBackend, SearchBackendChoice, SearchError, SqlFallbackBackend,
};
use uuid::Uuid;

fn sample_book() -> BookDocument {
    BookDocument {
        id: Uuid::new_v4().to_string(),
        title: "The Long Way".to_string(),
        authors: vec!["Becky Chambers".to_string()],
        author_ids: vec![Uuid::new_v4().to_string()],
        series_name: None,
        series_id: None,
        series_index: None,
        genres: vec![],
        cover_url: None,
        is_in_library: true,
        want_status: "owned".to_string(),
        first_letter: "T".to_string(),
        ingest_status: None,
        has_audiobook: false,
        has_ebook: true,
        narrator: None,
        all_series_names: None,
        description: None,
        vectors: None,
    }
}

#[test]
fn unconfigured_meili_url_selects_sql_fallback() {
    for url in ["", "   ", "\t\n"] {
        assert!(
            matches!(
                SearchBackendChoice::from_config(url, "any-key"),
                SearchBackendChoice::SqlFallback
            ),
            "url {url:?} should select the SQL fallback backend"
        );
    }
}

#[test]
fn configured_meili_url_selects_meili() {
    assert!(matches!(
        SearchBackendChoice::from_config("http://meilisearch:7700", "key"),
        SearchBackendChoice::Meili(_)
    ));
}

#[test]
fn malformed_meili_url_degrades_to_sql_fallback() {
    assert!(matches!(
        SearchBackendChoice::from_config("not a url", "key"),
        SearchBackendChoice::SqlFallback
    ));
}

#[test]
fn meili_state_new_errors_on_malformed_url() {
    let err = MeiliState::new("not a url", "key")
        .err()
        .expect("malformed URL must not panic");
    assert!(matches!(err, SearchError::Connection(_)), "got {err:?}");
}

#[test]
fn meili_state_new_errors_on_master_key_with_invalid_header_bytes() {
    let err = MeiliState::new("http://localhost:7700", "bad\nkey")
        .err()
        .expect("master key with a newline must not panic");
    assert!(matches!(err, SearchError::Connection(_)), "got {err:?}");
}

#[test]
fn meili_state_new_accepts_valid_settings() {
    assert!(MeiliState::new("http://localhost:7700", "master-key").is_ok());
}

#[tokio::test]
async fn sql_fallback_entity_searches_signal_the_ilike_path() {
    let backend = SqlFallbackBackend::new();

    let results: Vec<Result<Vec<Uuid>, SearchError>> = vec![
        backend
            .search_books("q", BookSearchFilters::default(), 10, 0)
            .await,
        backend.search_authors("q", Default::default(), 10, 0).await,
        backend.search_series("q", Default::default(), 10, 0).await,
        backend.search_movies("q", Default::default(), 10, 0).await,
        backend
            .search_tv_shows("q", Default::default(), 10, 0)
            .await,
        backend.search_people("q", 10, 0).await,
    ];

    for result in results {
        let err = result.expect_err("entity search must error so routes fall back to ILIKE");
        assert!(matches!(err, SearchError::Backend(_)), "got {err:?}");
    }
}

#[tokio::test]
async fn sql_fallback_facets_and_letters_signal_the_sql_path() {
    let backend = SqlFallbackBackend::new();

    assert!(
        backend
            .get_available_book_letters(Default::default())
            .await
            .is_err()
    );
    assert!(backend.get_book_facets(Default::default()).await.is_err());
    assert!(backend.get_movie_facets(Default::default()).await.is_err());
    assert!(
        backend
            .get_tv_show_facets(Default::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sql_fallback_indexing_is_a_silent_no_op() {
    let backend = SqlFallbackBackend::new();

    assert!(backend.index_book(sample_book()).await.is_ok());
    assert!(backend.delete_book(Uuid::new_v4()).await.is_ok());
    assert_eq!(
        backend
            .reindex_all_books(vec![sample_book(), sample_book()])
            .await
            .unwrap(),
        2
    );
    assert_eq!(backend.reindex_all_authors(vec![]).await.unwrap(), 0);
    assert_eq!(backend.reindex_all_series(vec![]).await.unwrap(), 0);
    assert_eq!(backend.reindex_all_movies(vec![]).await.unwrap(), 0);
    assert_eq!(backend.reindex_all_tv_shows(vec![]).await.unwrap(), 0);
    assert_eq!(backend.reindex_all_people(vec![]).await.unwrap(), 0);
}

#[tokio::test]
async fn sql_fallback_status_is_reported_honestly() {
    let status = SqlFallbackBackend::new().status().await;

    assert_eq!(status.backend_type, "sql_fallback");
    assert!(status.connected);
    assert!(
        status.error.is_none(),
        "SQL fallback is a supported mode, not an error state"
    );
}

fn guide_doc(
    channel_id: &str,
    title: &str,
    episode_title: Option<&str>,
    synopsis: &str,
) -> LivetvGuideDocument {
    LivetvGuideDocument {
        id: Uuid::new_v4().to_string(),
        channel_id: channel_id.to_string(),
        channel_name: "Nine".to_string(),
        title: title.to_string(),
        episode_title: episode_title.map(str::to_string),
        synopsis: Some(synopsis.to_string()),
        genres: vec![],
        start_time: 0,
        end_time: 1,
        start_time_iso: String::new(),
        end_time_iso: String::new(),
        series_id: None,
        image_url: None,
        is_live: false,
    }
}

#[tokio::test]
async fn meili_guide_search_matches_programme_titles_not_episodes_or_synopses() {
    let url =
        std::env::var("MEILI_URL_TEST").unwrap_or_else(|_| "http://localhost:7701".to_string());
    let meili = MeiliState::new(&url, "test-master-key").unwrap();
    meili.init_indexes().await.unwrap();
    let channel = Uuid::new_v4().to_string();
    meili
        .index_livetv_guide(&[
            guide_doc(&channel, "Breakin'", None, "Street dancers."),
            guide_doc(
                &channel,
                "Frasier",
                Some("Breaking the Ice"),
                "Frasier and Martin go ice fishing.",
            ),
            guide_doc(
                &channel,
                "Below Deck",
                Some("The Captain"),
                "The crew are breaking down.",
            ),
        ])
        .await
        .unwrap();
    let filters = LivetvGuideSearchFilters {
        channel_id: Some(channel.clone()),
        ..Default::default()
    };

    let mut titles = Vec::new();
    for _ in 0..100 {
        let found = meili
            .search_livetv_guide("Frasier", &filters, 50, 0)
            .await
            .unwrap_or_default();
        if !found.is_empty() {
            let hits = meili
                .search_livetv_guide("breakin", &filters, 50, 0)
                .await
                .unwrap();
            titles = hits.into_iter().map(|d| d.title).collect();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(titles, vec!["Breakin'".to_string()]);
}
