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
use tokio::{
    sync::mpsc,
    time::{Duration, sleep},
};
use tracing::{debug, info};

#[derive(Debug)]
enum QueueError {
    AlreadyInQueue,
    LockPoisoned,
    NotInQueue,
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
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_thread_ids(true)
        .init();

    let state = AppState {
        matchmaking_state: Arc::new(RwLock::new(MatchmakingState {
            queue: VecDeque::new(),
            players: HashMap::new(),
        })),
    };

    start_matchmaking(state.matchmaking_state.clone());

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
            info!("Joined queue at {}", queue_position);
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
                "hi" => {
                    let _ = tx.send(Message::Text(Utf8Bytes::from_static("die")));
                }
                "pos" => {
                    let pos = get_position(&player_id, &state.matchmaking_state)
                        .await
                        .unwrap();
                    let _ = tx.send(Message::text(pos.to_string()));
                }
                _ => {
                    todo!();
                }
            }
        }
    }

    if let Err(e) = leave_queue(&player_id, &state.matchmaking_state).await {
        eprintln!("Failed to remove player {} from queue: {:?}", player_id, e);
    }
}

fn start_matchmaking(state: Arc<RwLock<MatchmakingState>>) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(2)).await;

            match process_matchmaking(&state).await {
                Ok(()) => {
                    info!("Successful process matchmaking");
                }
                Err(err) => {
                    debug!("Error {:?}", err);
                }
            };
        }
    });
}

async fn process_matchmaking(
    matchmaking_state: &Arc<RwLock<MatchmakingState>>,
) -> Result<(), QueueError> {
    let mut state = matchmaking_state
        .write()
        .map_err(|_| QueueError::LockPoisoned)?;

    if state.queue.len() >= 1 {
        if let Some(player_id) = state.queue.pop_front() {
            info!("Player {} found a match", player_id);
            if let Some(player) = state.players.get(&player_id) {
                let _ = player.tx.send(Message::text("Player found message"));
                // Remove from players
                let _ = state.players.remove(&player_id);
            };
        };
    } else {
        info!("No players in queue :(");
    };

    Ok(())
}

async fn join_queue(
    player_id: &str,
    tx: mpsc::UnboundedSender<Message>,
    matchmaking_state: &Arc<RwLock<MatchmakingState>>,
) -> Result<(usize), QueueError> {
    let mut state = matchmaking_state
        .write()
        .map_err(|_| QueueError::LockPoisoned)?;

    if state.queue.contains(&player_id.to_string()) {
        return Err(QueueError::AlreadyInQueue);
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

    info!("Inserted {} into players", player_id.to_string());

    Ok(state.queue.len())
}

async fn leave_queue(
    player_id: &str,
    state: &Arc<RwLock<MatchmakingState>>,
) -> Result<(), QueueError> {
    let mut state = state.write().map_err(|_| QueueError::LockPoisoned)?;

    if let Some(index) = state.queue.iter().position(|id| id == player_id) {
        state.queue.remove(index);
    } else {
        return Err(QueueError::NotInQueue);
    }

    state.players.remove(player_id);

    info!("Player {} leaving queue", player_id);
    Ok(())
}

async fn get_position(
    player_id: &str,
    state: &Arc<RwLock<MatchmakingState>>,
) -> Result<usize, QueueError> {
    let state = state.write().map_err(|_| QueueError::LockPoisoned)?;

    let index = state
        .queue
        .iter()
        .position(|id| id == player_id)
        .ok_or(QueueError::NotInQueue)?;

    Ok(index)
}
