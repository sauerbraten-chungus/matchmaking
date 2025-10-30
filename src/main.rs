use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::{any, get},
};
use futures_util::{SinkExt, StreamExt};
use matchmaker::MatchmakingResponse;
use matchmaker::chungustrator::chungustrator_client::ChungustratorClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::error;

mod matchmaker;

#[derive(Debug)]
enum QueueError {
    AlreadyInQueue,
    LockPoisoned,
    NotInQueue,
}

#[derive(Serialize)]
struct WebsocketResponse {
    event_type: EventType,
    data: serde_json::Value,
}

#[derive(Serialize)]
enum EventType {
    JoinQueue,
    MatchFound,
    MatchCreated,
    JoinError,
    QueuePosition,
    LeaveQueue,
}

#[derive(Serialize)]
struct JoinQueueData {
    queue_pos: usize,
}

#[derive(Serialize)]
struct MatchFoundData {
    message: String,
}

#[derive(Serialize)]
struct MatchCreatedData {
    ip: String,
    port: String,
}

#[derive(Serialize)]
struct JoinErrorData {
    message: String,
}

#[derive(Serialize)]
struct QueuePositionData {
    queue_pos: usize,
}

#[derive(Serialize)]
struct LeaveQueueData {
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "get_queue_position")]
    GetQueuePosition,
    #[serde(rename = "leave_queue")]
    LeaveQueue,
}

#[derive(Clone)]
struct AppState {
    tx: mpsc::UnboundedSender<matchmaker::MatchmakingMessage>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_thread_ids(true)
        .init();

    // Get orchestrator gRPC address from environment or use default
    let orchestrator_addr =
        std::env::var("CHUNGUSTRATOR_URL").unwrap_or_else(|_| "http://localhost:7000".to_string());

    // Create gRPC client
    let grpc_client = ChungustratorClient::connect(orchestrator_addr)
        .await
        .expect("Failed to connect to orchestrator gRPC service");

    let (tx, rx) = mpsc::unbounded_channel();
    matchmaker::Matchmaker::new(rx, grpc_client);

    let state = Arc::new(AppState { tx });

    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/ws", any(ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, state: State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |websocket| handle_socket(websocket, state.tx.clone()))
}

async fn handle_socket(
    socket: WebSocket,
    matchmaker_tx: mpsc::UnboundedSender<matchmaker::MatchmakingMessage>,
) {
    let player_id = uuid::Uuid::new_v4().to_string();

    let (player_tx, mut player_rx) = mpsc::unbounded_channel::<matchmaker::MatchmakingResponse>();

    let (mut ws_sender, mut ws_receiver) = socket.split();

    let tx_task = tokio::spawn(async move {
        while let Some(response) = player_rx.recv().await {
            let ws_response = match response {
                MatchmakingResponse::JoinSuccess(queue_pos) => WebsocketResponse {
                    event_type: EventType::JoinQueue,
                    data: serde_json::to_value(JoinQueueData { queue_pos }).unwrap(),
                },
                MatchmakingResponse::MatchFound => WebsocketResponse {
                    event_type: EventType::MatchFound,
                    data: serde_json::to_value(MatchFoundData {
                        message: "Match Found xD".to_string(),
                    })
                    .unwrap(),
                },
                MatchmakingResponse::MatchCreated(match_data) => WebsocketResponse {
                    event_type: EventType::MatchCreated,
                    data: serde_json::to_value(match_data).unwrap(),
                },
                MatchmakingResponse::JoinError(err_str) => WebsocketResponse {
                    event_type: EventType::JoinError,
                    data: serde_json::to_value(JoinErrorData { message: err_str }).unwrap(),
                },
                MatchmakingResponse::PositionSuccess(queue_pos) => WebsocketResponse {
                    event_type: EventType::QueuePosition,
                    data: serde_json::to_value(QueuePositionData { queue_pos }).unwrap(),
                },
                MatchmakingResponse::LeaveSuccess => WebsocketResponse {
                    event_type: EventType::LeaveQueue,
                    data: serde_json::to_value(LeaveQueueData {
                        message: "Left q xD".to_string(),
                    })
                    .unwrap(),
                },
            };

            let json_msg = serde_json::to_string(&ws_response).unwrap();
            ws_sender.send(Message::text(json_msg)).await.unwrap();
        }
    });

    let join_message = matchmaker::MatchmakingMessage::JoinQueue(matchmaker::JoinRequest {
        id: player_id.clone(),
        tx: player_tx,
    });

    if let Err(e) = matchmaker_tx.send(join_message) {
        error!("Error sending message to matchmaker: {:?}", e);
    }

    while let Some(Ok(msg)) = ws_receiver.next().await {
        if let Message::Text(text) = msg {
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::GetQueuePosition) => {
                    let mm_message = matchmaker::MatchmakingMessage::GetQueuePosition(
                        matchmaker::PositionRequest {
                            id: player_id.clone(),
                        },
                    );
                    matchmaker_tx.send(mm_message).unwrap();
                }
                Ok(ClientMessage::LeaveQueue) => {
                    let mm_message =
                        matchmaker::MatchmakingMessage::LeaveQueue(matchmaker::LeaveRequest {
                            id: player_id.clone(),
                        });
                    matchmaker_tx.send(mm_message).unwrap();
                }
                Err(e) => {
                    error!("Unknown client message: {:?}", e)
                }
            };
        }
    }

    let force_leave_message =
        matchmaker::MatchmakingMessage::LeaveQueue(matchmaker::LeaveRequest {
            id: player_id.clone(),
        });

    if let Err(_) = matchmaker_tx.send(force_leave_message) {
        error!("Client and Player {} closed connection", player_id);
    }
}
