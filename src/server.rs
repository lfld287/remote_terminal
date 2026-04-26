use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::Html,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use crate::auth::verify_token;
use crate::pty::PtyManager;

#[derive(Clone)]
pub struct AppState {
    pub pty_manager: Arc<PtyManager>,
    pub token: Arc<String>,
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub shell: Option<String>,
}

#[derive(Deserialize)]
pub struct ResizePayload {
    pub cols: u16,
    pub rows: u16,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<AppState>,
) -> axum::response::Response {
    let token = query.token.clone().unwrap_or_default();
    if !verify_token(&token, &state.token) {
        return axum::response::Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }

    ws.on_upgrade(move |socket| handle_ws(socket, state, query))
}

async fn handle_ws(socket: WebSocket, state: AppState, query: WsQuery) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let cols = query.cols.unwrap_or(80);
    let rows = query.rows.unwrap_or(24);

    let result = state
        .pty_manager
        .create_session(cols, rows, query.shell)
        .await;

    let (session_id, output_tx) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = ws_sender
                .send(Message::Text(format!("Error: {e}").into()))
                .await;
            return;
        }
    };

    let mut output_rx = output_tx.subscribe();

    let id_for_task = session_id.clone();
    let pty = state.pty_manager.clone();
    let read_task = tokio::spawn(async move {
        while let Ok(data) = output_rx.recv().await {
            let msg = Message::Binary(data.into());
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
        let _ = pty.kill_session(&id_for_task).await;
    });

    let id_for_write = session_id.clone();
    let pty_for_write = state.pty_manager.clone();
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Binary(data) => {
                if pty_for_write
                    .write_to_session(&id_for_write, &data)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Text(text) => {
                if let Ok(resize) = serde_json::from_str::<ResizePayload>(&text) {
                    let _ = pty_for_write
                        .resize_session(&id_for_write, resize.cols, resize.rows)
                        .await;
                } else {
                    let data = text.as_bytes();
                    if pty_for_write
                        .write_to_session(&id_for_write, data)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    read_task.abort();
    let _ = state.pty_manager.kill_session(&session_id).await;
}
