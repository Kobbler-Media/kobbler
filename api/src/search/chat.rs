//! Meilisearch Chat (conversational AI search) configuration.
//!
//! Configures Meilisearch's built-in RAG proxy: enables the experimental feature,
//! sets per-index chat descriptions, and creates the "kobbler" chat workspace.

use meilisearch_sdk::chats::{ChatPrompts, ChatWorkspaceSettings};
use meilisearch_sdk::client::Client;
use serde_json::json;
use tracing::{info, warn};

use crate::runtime_config::RuntimeConfig;

/// Meilisearch workspace UID for Kobbler chat.
const WORKSPACE_UID: &str = "kobbler";

/// System prompt for the chat workspace.
const SYSTEM_PROMPT: &str = r#"You are a personal media library assistant for Kobbler. The user's library contains books (ebooks and audiobooks), movies, TV shows, and metadata about authors, series, genres, and people (actors/directors/narrators).

When searching, you have access to: books (with reading progress), movies (with watch state), TV shows (with episode watch progress), people, authors, series, genres, and full-text transcript segments from audiobooks and ebooks (with timestamps/chapter info).

For recommendations: search the library first to understand what the user likes, then suggest both items already in their library and new items they might enjoy.

For transcript searches: results include start_seconds (audiobook) or CFI (ebook) that can locate the exact passage.

Always cite specific titles and names from search results. If nothing matches, say so clearly."#;

/// Enable the chatCompletions experimental feature on Meilisearch.
pub async fn enable_chat_feature(meili_url: &str, master_key: &str) -> Result<(), String> {
    let client = crate::http::new_client();
    let resp = client
        .patch(format!("{}/experimental-features", meili_url))
        .header("Authorization", format!("Bearer {}", master_key))
        .json(&json!({"chatCompletions": true}))
        .send()
        .await
        .map_err(|e| format!("Failed to enable chat feature: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Failed to enable chat feature: {} {}",
            status, body
        ));
    }

    info!("Meilisearch chatCompletions experimental feature enabled");
    Ok(())
}

/// Per-index chat configuration.
struct IndexChatConfig {
    index_uid: &'static str,
    description: &'static str,
    document_template: &'static str,
}

/// Get chat configurations for all content indexes.
fn chat_index_configs() -> Vec<IndexChatConfig> {
    use super::indexes;
    vec![
        IndexChatConfig {
            index_uid: indexes::BOOKS,
            description: "Books and audiobooks in the user's library and want list. Includes title, authors, series, genres, narrator, library/want status, whether it has an audiobook version, and reading progress percentage.",
            document_template: "Title: {{doc.title}}\nAuthors: {{doc.authors}}\nSeries: {{doc.series_name}} #{{doc.series_index}}\nGenres: {{doc.genres}}\nNarrator: {{doc.narrator}}\nIn Library: {{doc.is_in_library}}\nWant Status: {{doc.want_status}}\nHas Audiobook: {{doc.has_audiobook}}\nAbout: {{doc.description}}",
        },
        IndexChatConfig {
            index_uid: indexes::AUTHORS,
            description: "Authors with book counts and monitoring status.",
            document_template: "Name: {{doc.name}}\nBooks: {{doc.book_count}}\nMonitored: {{doc.is_monitored}}",
        },
        IndexChatConfig {
            index_uid: indexes::SERIES,
            description: "Book series with book counts and monitoring status.",
            document_template: "Name: {{doc.name}}\nBooks: {{doc.book_count}}\nMonitored: {{doc.is_monitored}}",
        },
        IndexChatConfig {
            index_uid: indexes::MOVIES,
            description: "Movies in the user's library with title, year, directors, genres, runtime, content rating, library/want status, rating and a short synopsis.",
            document_template: "Title: {{doc.title}} ({{doc.year}})\nDirectors: {{doc.directors}}\nGenres: {{doc.genres}}\nRuntime: {{doc.runtime}} min\nRated: {{doc.content_rating}}\nIn Library: {{doc.is_in_library}}\nWant Status: {{doc.want_status}}\nRating: {{doc.rating}}\nAbout: {{doc.description}}",
        },
        IndexChatConfig {
            index_uid: indexes::TV_SHOWS,
            description: "TV shows with title, year, genres, network, status, content rating, monitoring state, how many episodes are actually available, and a short synopsis.",
            document_template: "Title: {{doc.title}} ({{doc.year}})\nGenres: {{doc.genres}}\nNetwork: {{doc.network}}\nStatus: {{doc.status}}\nRated: {{doc.content_rating}}\nMonitored: {{doc.is_monitored}}\nEpisodes Available: {{doc.available_episode_count}}/{{doc.total_episodes}}\nRating: {{doc.rating}}\nAbout: {{doc.description}}",
        },
        IndexChatConfig {
            index_uid: indexes::PEOPLE,
            description: "Actors, directors, authors, and narrators. Includes bio, movie count, and TV show count.",
            document_template: "Name: {{doc.name}}\nBio: {{doc.bio}}\nMovies: {{doc.movie_count}}\nTV Shows: {{doc.tv_show_count}}",
        },
        IndexChatConfig {
            index_uid: indexes::TRANSCRIPT_SEGMENTS,
            description: "Full text from audiobook transcriptions and ebook content. Each segment has book title, chapter title, and timestamp (start_seconds) for audiobooks or CFI for ebooks. Use for finding specific scenes, quotes, or passages.",
            document_template: "Text: {{doc.text}}\nBook: {{doc.book_title}}\nChapter: {{doc.chapter_title}}\nTimestamp: {{doc.start_seconds}}s",
        },
        IndexChatConfig {
            index_uid: indexes::GENRES,
            description: "Genres with book counts.",
            document_template: "Name: {{doc.name}}\nBooks: {{doc.book_count}}",
        },
    ]
}

/// Configure per-index chat settings (descriptions and document templates).
pub async fn configure_chat_index_settings(
    meili_url: &str,
    master_key: &str,
) -> Result<(), String> {
    let client = crate::http::new_client();

    for config in chat_index_configs() {
        let resp = client
            .patch(format!(
                "{}/indexes/{}/settings",
                meili_url, config.index_uid
            ))
            .header("Authorization", format!("Bearer {}", master_key))
            .json(&json!({
                "chat": {
                    "description": config.description,
                    "documentTemplate": config.document_template,
                    "documentTemplateMaxBytes": 800
                }
            }))
            .send()
            .await
            .map_err(|e| {
                format!(
                    "Failed to configure chat for index {}: {}",
                    config.index_uid, e
                )
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(
                index = config.index_uid,
                "Failed to configure chat index settings: {} {}", status, body
            );
        }
    }

    info!("Chat index settings configured for all content indexes");
    Ok(())
}

/// Resolve the base URL for a known provider.
fn resolve_base_url(provider: Option<&str>, explicit_url: Option<&str>) -> Option<String> {
    if let Some(url) = explicit_url
        && !url.is_empty()
    {
        return Some(url.to_string());
    }
    match provider {
        Some("gemini") => {
            Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string())
        }
        Some("openai") => Some("https://api.openai.com/v1/".to_string()),
        _ => None,
    }
}

/// Configure the "kobbler" chat workspace with LLM provider settings.
///
/// When `koblem_service_url` is configured, koblem is used as the LLM backend
/// (via vLlm source with a dummy API key). Otherwise falls back to the
/// configured external provider (Gemini, OpenAI, etc).
pub async fn configure_chat_workspace(
    client: &Client,
    config: &RuntimeConfig,
) -> Result<(), String> {
    let mut settings = ChatWorkspaceSettings::new();

    // Check if koblem is configured — use it as the LLM provider
    let koblem_url = config
        .koblem_service_url
        .as_deref()
        .filter(|s| !s.is_empty());

    if let Some(url) = koblem_url {
        // koblem exposes OpenAI-compatible /v1/chat/completions
        let base_url = format!("{}/v1", url.trim_end_matches('/'));
        settings.set_source("vLlm");
        settings.set_base_url(&base_url);
        // MeiliSearch requires a non-empty API key even if the backend doesn't check it
        settings.set_api_key("koblem");
        info!("Chat workspace using koblem at {}", url);
    } else {
        // Fall back to external LLM provider
        let base_url = resolve_base_url(
            config.chat_llm_provider.as_deref(),
            config.chat_llm_base_url.as_deref(),
        );

        let api_key = config.chat_llm_api_key.as_deref().unwrap_or_default();
        if api_key.is_empty() {
            return Err(
                "Chat LLM API key is required (or configure koblem_service_url)".to_string(),
            );
        }

        // Meilisearch 1.40+ openAi source honors baseUrl, so Gemini (OpenAI-compatible)
        // can use openAi source with a custom base URL. Only custom/unknown providers use vLlm.
        let source = match config.chat_llm_provider.as_deref() {
            Some("openai") | Some("gemini") => "openAi",
            Some("mistral") => "mistral",
            _ => "vLlm",
        };
        settings.set_source(source);
        // Set base_url for sources that need it (vLlm always, openAi when overridden for Gemini).
        if let Some(ref url) = base_url
            && (source == "vLlm" || config.chat_llm_provider.as_deref() == Some("gemini"))
        {
            settings.set_base_url(url);
        }
        settings.set_api_key(api_key);
    }

    let mut prompts = ChatPrompts::new();
    prompts.set_system(SYSTEM_PROMPT);
    settings.set_prompts(prompts);

    client
        .update_chat_workspace_settings(WORKSPACE_UID, &settings)
        .await
        .map_err(|e| format!("Failed to configure chat workspace: {}", e))?;

    info!("Chat workspace '{}' configured", WORKSPACE_UID);
    Ok(())
}

/// Full chat setup: enable feature, configure indexes, configure workspace.
pub async fn setup_chat(
    client: &Client,
    meili_url: &str,
    master_key: &str,
    config: &RuntimeConfig,
) -> Result<(), String> {
    if !config.chat_enabled {
        return Ok(());
    }

    let has_koblem = config
        .koblem_service_url
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let has_api_key = !config
        .chat_llm_api_key
        .as_deref()
        .unwrap_or_default()
        .is_empty();

    if !has_koblem && !has_api_key {
        info!("Chat enabled but no API key or koblem configured, skipping setup");
        return Ok(());
    }

    enable_chat_feature(meili_url, master_key).await?;
    configure_chat_index_settings(meili_url, master_key).await?;
    configure_chat_workspace(client, config).await?;

    info!("Meilisearch chat fully configured");
    Ok(())
}
