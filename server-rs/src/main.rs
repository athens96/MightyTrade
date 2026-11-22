use mighty_trade_server::build_router;
use mighty_trade_server::config::Config;
use mighty_trade_server::state::AppState;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();
    let bind_addr = cfg.bind_addr.clone();
    let state = AppState::new(cfg)?;
    mighty_trade_server::autotrade::scheduler::maybe_start(state.clone());
    let app = build_router(state);

    let listener = TcpListener::bind(&bind_addr).await?;
    tracing::info!("MightyTrade Rust API listening on http://{bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
