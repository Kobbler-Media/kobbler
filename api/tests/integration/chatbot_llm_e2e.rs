//! Direct-LLM e2e as a `cargo test` target: the same scenarios the deployed
//! binary runs (`api/src/bin/chatbot_e2e.rs`), see `api/src/chatbot/e2e.rs`
//! for what is exercised.
//!
//! Skipped unless `CHATBOT_E2E=1`, so `make` and CI never hit a live model:
//!
//! ```sh
//! CHATBOT_E2E=1 cargo test -p api --test integration chatbot_llm_e2e:: -- --nocapture
//! ```
//!
//! The model comes from `config/kobbler.toml` (override the path with
//! `CHATBOT_E2E_CONFIG`), so the test asks the same model the bot would use.

use api::chatbot::e2e;
use api::chatbot::llm;

#[tokio::test]
async fn the_chatbot_behaves_on_the_configured_model() {
    if !std::env::var("CHATBOT_E2E").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")) {
        eprintln!("skipped: CHATBOT_E2E not set");
        return;
    }
    let path =
        std::env::var("CHATBOT_E2E_CONFIG").unwrap_or_else(|_| "config/kobbler.toml".to_string());
    let file = api::file_config::FileConfig::load(&path).expect("load kobbler.toml");
    let cfg = llm::resolve(&file.to_runtime_config()).expect("resolve chat LLM config");
    println!(
        "model: {} ({})",
        cfg.model,
        if cfg.is_koblem { "koblem" } else { "external" }
    );

    let results = e2e::run_all(&cfg).await;
    let mut failed = 0;
    for r in &results {
        println!("[{}] {}", if r.passed { "PASS" } else { "FAIL" }, r.name);
        if !r.passed {
            println!("    {}", r.detail);
            failed += 1;
        }
    }
    assert_eq!(failed, 0, "chatbot e2e scenarios failed");
}
