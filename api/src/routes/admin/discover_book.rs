use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use serde_json::json;

use crate::metadata::{MetadataResult, ProviderError, ProviderRegistry};
use crate::routes::admin::library_status::book_status;

#[derive(Serialize)]
struct BookDetailView {
    #[serde(flatten)]
    result: MetadataResult,
    year: Option<i32>,
    release_date: Option<String>,
}

impl BookDetailView {
    fn new(mut result: MetadataResult) -> Self {
        result.identifiers = serde_json::Value::Object(result.all_identifiers());
        let date = result.publish_date.as_deref().unwrap_or_default();
        let year = date.get(..4).and_then(|y| y.parse().ok());
        let release_date = time::Date::parse(
            date.get(..10).unwrap_or_default(),
            &time::format_description::well_known::Iso8601::DATE,
        )
        .is_ok()
        .then(|| date.to_string());
        Self {
            result,
            year,
            release_date,
        }
    }
}

pub(super) async fn get_book_detail(
    pool: &sqlx::PgPool,
    provider: &str,
    external_id: &str,
) -> axum::response::Response {
    let registry = ProviderRegistry::new(pool.clone());
    let result = match registry.fetch_by_id(provider, external_id).await {
        Ok(Some(result)) => result,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("{provider} has no book {external_id}") })),
            )
                .into_response();
        }
        Err(e @ ProviderError::UnknownProvider(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("{provider} error: {e}") })),
            )
                .into_response();
        }
    };

    let id = result.external_id.as_deref().unwrap_or(external_id);
    let status = book_status(pool, provider, id).await;
    Json(json!({
        "media_type": "book",
        "provider": provider,
        "details": BookDetailView::new(result),
        "library_id": status.as_ref().map(|s| s.id),
        "want_status": status.as_ref().and_then(|s| s.want_status.clone()),
        "is_in_library": status.and_then(|s| s.is_in_library),
    }))
    .into_response()
}
