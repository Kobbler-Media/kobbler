//! chatbot_e2e — "given this data, what does the LLM do."
//!
//! The real system prompt and tool schemas, the box's configured chat LLM,
//! fixture answers for every tool call. Shipped in the api image so a
//! deployed box can be asked without any source tree, API mode change, or
//! download:
//!
//! ```sh
//! docker exec kobbler-api-1 /app/chatbot_e2e
//! ```
//!
//! Reads the same config the api does (`CONFIG_PATH`, default
//! `/config/kobbler.toml`; override with `CHATBOT_E2E_CONFIG`). Exit code 0
//! means every scenario passed.

#[tokio::main]
async fn main() {
    let path = std::env::var("CHATBOT_E2E_CONFIG")
        .or_else(|_| std::env::var("CONFIG_PATH"))
        .unwrap_or_else(|_| "/config/kobbler.toml".to_string());
    let file = match api::file_config::FileConfig::load(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("could not load {path}: {e}");
            std::process::exit(2);
        }
    };
    let cfg = match api::chatbot::llm::resolve(&file.to_runtime_config()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chat LLM is not configured: {e}");
            std::process::exit(2);
        }
    };
    println!(
        "model: {} ({})",
        cfg.model,
        if cfg.is_koblem { "koblem" } else { "external" }
    );

    let results = api::chatbot::e2e::run_all(&cfg).await;
    let mut failed = 0;
    for r in &results {
        println!("[{}] {}", if r.passed { "PASS" } else { "FAIL" }, r.name);
        if !r.passed {
            println!("    {}", r.detail);
            failed += 1;
        }
    }
    println!(
        "\n{}/{} scenarios passed",
        results.len() - failed,
        results.len()
    );
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
