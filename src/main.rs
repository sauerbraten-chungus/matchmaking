use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, Utf8Bytes, WebSocket},
    },
    response::Response,
    routing::{any, get},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;

enum JoinQueueError {
    AlreadyInQueue,
    LockPoisoned,
}

struct Player {
    id: String,
    tx: mpsc::UnboundedSender<Message>,
    joined_at: std::time::Instant,
}

struct MatchmakingState {
    queue: VecDeque<String>,
    players: HashMap<String, Player>,
}

#[derive(Clone)]
struct AppState {
    matchmaking_state: Arc<RwLock<MatchmakingState>>,
}

#[tokio::main]
async fn main() {
    let state = AppState {
        matchmaking_state: Arc::new(RwLock::new(MatchmakingState {
            queue: VecDeque::new(),
            players: HashMap::new(),
        })),
    };

    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/ws", any(ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, state: State<AppState>) -> Response {
    ws.on_upgrade(move |websocket| handle_socket(websocket, state))
}

async fn handle_socket(socket: WebSocket, state: State<AppState>) {
    let player_id = uuid::Uuid::new_v4().to_string();

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let (mut ws_sender, mut ws_receiver) = socket.split();

    let tx_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    match join_queue(&player_id, tx.clone(), &state.matchmaking_state).await {
        Ok(queue_position) => {
            if tx
                .send(Message::Text(Utf8Bytes::from_static("Success")))
                .is_err()
            {
                println!("FUCK");
                return;
            }
        }
        Err(err) => {
            let _ = tx.send(Message::Text(Utf8Bytes::from_static("FUCK ERROR Q")));
            return;
        }
    }

    while let Some(Ok(msg)) = ws_receiver.next().await {
        if let Message::Text(text) = msg {
            match text.as_str() {
                "disconnect" => {
                    let _ = tx.send(Message::Text(Utf8Bytes::from_static("die")));
                }
                _ => {
                    todo!();
                }
            }
        }
    }
}

async fn join_queue(
    player_id: &str,
    tx: mpsc::UnboundedSender<Message>,
    matchmaking_state: &Arc<RwLock<MatchmakingState>>,
) -> Result<(usize), JoinQueueError> {
    let mut state = matchmaking_state
        .write()
        .map_err(|_| JoinQueueError::LockPoisoned)?;

    if state.queue.contains(&player_id.to_string()) {
        return Err(JoinQueueError::AlreadyInQueue);
    }

    state.queue.push_back(player_id.to_string());
    state.players.insert(
        player_id.to_string(),
        Player {
            id: player_id.to_string(),
            tx,
            joined_at: std::time::Instant::now(),
        },
    );

    Ok(state.queue.len())
}
