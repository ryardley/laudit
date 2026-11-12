use axum::{routing::get, Router};

fn main() {
    let _app = Router::new().route("/health", get(health));
}

async fn health() -> &'static str {
    "ok"
}
