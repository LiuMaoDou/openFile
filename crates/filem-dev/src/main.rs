use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use filem_core::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
mod directory_picker;

#[derive(Clone)]
struct App {
    engine: Engine,
    token: Arc<String>,
}
async fn authorize(State(app): State<App>, request: Request, next: Next) -> Response {
    use axum::response::IntoResponse;
    if request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(format!("Bearer {}", app.token).as_str())
    {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    next.run(request).await
}
#[derive(Deserialize)]
struct Command {
    command: String,
    #[serde(default)]
    args: Value,
}
async fn command(
    State(app): State<App>,
    Json(input): Json<Command>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tokio::task::spawn_blocking(move || {
        if input.command == "pick_directory" {
            return Ok(serde_json::to_value(directory_picker::pick()?)?);
        }
        app.engine.dispatch(&input.command, input.args)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":format!("{e:#}")})),
        )
    })?
    .map(Json)
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":format!("{e:#}")})),
        )
    })
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if filem_core::run_helper_if_requested() {
        return Ok(());
    }
    let token =
        std::env::var("FILEM_DEV_TOKEN").expect("请通过 npm run dev 启动，以创建本次会话令牌。");
    let dir = PathBuf::from(std::env::var("FILEM_DATA_DIR").unwrap_or_else(|_| ".filem".into()));
    let engine = Engine::open_managed(dir)?;
    let state = App {
        engine,
        token: Arc::new(token),
    };
    let app = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({"ok":true,"mode":"local-development"})) }),
        )
        .route("/api/command", post(command))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4318").await?;
    println!("FileM Rust 索引服务已启动（仅本机）：127.0.0.1:4318");
    axum::serve(listener, app).await?;
    Ok(())
}
