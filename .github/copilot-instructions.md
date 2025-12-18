<!-- concise, repo-specific guidance for AI coding agents -->
# copilot-instructions for proxy-scraper-checker

This repository is an async Rust CLI (optional TUI) that: load config, optionally download IPDBs, scrape proxy sources, check proxies concurrently, and write outputs.

Key files and where to start
- `src/main.rs`: program entry, cancellation setup and runtime feature selection. Look here to understand task orchestration.
- `src/raw_config.rs` → `src/config.rs`: TOML parsing and runtime config mapping (output path logic, Docker behavior).
- `src/scraper.rs`: source download + proxy extraction (uses `parsers::PROXY_REGEX`).
- `src/checker.rs`: worker pool and queueing (uses `tokio::task::JoinSet`, shared `Arc<parking_lot::Mutex<...>>`).
- `src/http.rs`: `create_reqwest_client`, retry middleware, and `HickoryDnsResolver` (custom DNS resolver used when building clients).
- `src/proxy.rs`: `Proxy` model and `check()` implementation (how checks build clients and measure latency).
- `src/output.rs`: JSON/TXT exporters and optional MaxMind lookups.

Architecture notes agents must preserve
- Concurrency model: `tokio` async runtime + `JoinSet` workers. Use short-held `parking_lot::Mutex` locks; avoid blocking async tasks.
- Cancellation: `tokio_util::sync::CancellationToken` is observed across scrapers/checkers—respect it for long-running loops.
- Shared state: `Arc<parking_lot::Mutex<...>>` for queues and counters. Acquire locks briefly and copy out data when possible.

Project-specific conventions
- Error handling: returns use `crate::Result` and `color_eyre` helpers. Preserve WrapErr/OptionExt style when adding errors.
- Feature flags: `tui` toggles terminal UI code paths (`#[cfg(feature = "tui")]`). `tokio-multi-thread` selects multi-thread runtime.
- Profiling allocators (`dhat`, `jemalloc`, `mimalloc_v2/3`) are gated behind features—do not enable them globally.

Build / run / debug
- Build (no TUI): `cargo build --release`
- Run with default config: `cargo run --release --manifest-path Cargo.toml`
- With TUI and multi-threaded tokio: `cargo build --release --features "tui tokio-multi-thread"`
- Docker dev: see README quick-start or run `docker compose build && docker compose up --no-log-prefix --remove-orphans`

Integration and extension guidance (concrete spots)
- Add new scraping source: update `config.toml` schema in `src/raw_config.rs`, map it in `src/config.rs`, and ensure `src/scraper.rs` reads from `config.scraping.sources`.
- Change HTTP retry/backoff or DNS: modify `src/http.rs` (retry middleware and `HickoryDnsResolver`).
- Add extra proxy checks or metrics: extend `src/proxy.rs` and update worker logic in `src/checker.rs`.
- MaxMind/IPDB logic: `src/output.rs` and `src/ipdb.rs` (optional, gated by config). Only enable when `config` asks for lookups.

Quick examples (where to edit)
- Worker concurrency tuning: [src/checker.rs](src/checker.rs#L1-L160) — change `max_concurrent_checks` usage in config.
- Regex extraction: [src/parsers.rs](src/parsers.rs#L1-L80) — update `PROXY_REGEX` and test with `scraper` parsing logic.

Testing and local validation
- Use a small `config.toml` (reduce `max_proxies_per_source` and `max_concurrent_checks`) for fast dev iterations.
- For TUI changes run with `--features "tui"` in an interactive terminal.

If something is missing or unclear, tell me which area to expand (e.g., middleware examples, adding a new scrape source, or how to test changes end-to-end).
