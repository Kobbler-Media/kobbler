//! EmbeddingGemma-2: one vector space for video frames, artwork, summaries and
//! search queries.
//!
//! Every service that embeds goes through [`EmbeddingClient`], which speaks the
//! OpenAI `/v1/embeddings` shape that both llama-server and koblem answer. Wire
//! types and the model's text prefixes come from `koblem_client::embed`; the
//! numbers that depend on the model's score scale live in [`profile`].

pub mod client;
pub mod profile;

pub use client::EmbeddingClient;
pub use koblem_client::embed::{EmbedTask, cosine};

/// The model every stored vector comes from. Recorded next to the vectors (the
/// media-cache manifest, `search_embeddings.model`) so a model change reads as
/// stale instead of mixing two spaces.
pub const EMBED_MODEL: &str = koblem_client::embed::EG2_MODEL;
pub const EMBED_DIM: usize = koblem_client::embed::EG2_DIM;
/// Vision tokens per image. A server flag (`--image-max-tokens`), recorded in the
/// manifest so frames embedded under a different budget can be told apart.
pub const IMAGE_TOKENS: u32 = 70;
