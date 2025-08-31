use std::{
    collections::{HashMap, VecDeque},
    path::PrefixComponent,
};

use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, time};
use tracing::{error, info};

pub struct Matchmaker {
    queue: VecDeque<String>,
    players: HashMap<String, Player>,
    receiver: mpsc::UnboundedReceiver<MatchmakingMessage>,
    http_client: reqwest::Client,
}

struct Player {
    id: String,
    tx: mpsc::UnboundedSender<MatchmakingResponse>,
}

pub enum MatchmakingMessage {
    JoinQueue(JoinRequest),
    LeaveQueue(LeaveRequest),
    GetQueuePosition(PositionRequest),
}

pub enum MatchmakingResponse {
    MatchFound,
    MatchCreated(MatchCreatedData),
    JoinSuccess(usize),
    JoinError(String),
    LeaveSuccess,
    PositionSuccess(usize),
}

#[derive(Serialize)]
pub struct MatchCreatedData {
    pub ip: String,
    pub port: String,
}

#[derive(Debug)]
enum MatchmakingError {
    ChannelError,
    AlreadyInQueue,
    NotFound,
}

pub struct JoinRequest {
    pub id: String,
    pub tx: mpsc::UnboundedSender<MatchmakingResponse>,
}

pub struct PositionRequest {
    pub id: String,
}

pub struct LeaveRequest {
    pub id: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OrchestratorContainerResponse {
    Success(ContainerSuccessData),
    Error(ContainerErrorData),
}

#[derive(Deserialize)]
struct ContainerSuccessData {
    id: String,
}

#[derive(Deserialize)]
struct ContainerErrorData {
    error: String,
}

impl Matchmaker {
    pub fn new(rx: mpsc::UnboundedReceiver<MatchmakingMessage>) {
        let mut matchmaker = Matchmaker {
            queue: VecDeque::new(),
            players: HashMap::new(),
            receiver: rx,
            http_client: reqwest::Client::new(),
        };

        tokio::spawn(async move { matchmaker.run().await });
    }

    async fn run(mut self) {
        let mut interval = time::interval(time::Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.process_matchmaking().await;
                }
                Some(msg) = self.receiver.recv() => {
                    if let Err(e) = self.receive(msg) {
                        error!("Error receving message {:?}", e);
                    }
                }
            }
        }
    }

    fn receive(&mut self, msg: MatchmakingMessage) -> Result<(), MatchmakingError> {
        match msg {
            MatchmakingMessage::JoinQueue(join_req) => self.join_queue(join_req),
            MatchmakingMessage::LeaveQueue(leave_queue_req) => self.leave_queue(leave_queue_req),
            MatchmakingMessage::GetQueuePosition(position_req) => {
                self.get_queue_position(position_req)
            }
        }
    }

    async fn process_matchmaking(&mut self) {
        // TODO: batch players into Match, broadcast game server IP

        const PLAYERS_PER_MATCH: usize = 1;

        if self.queue.len() >= PLAYERS_PER_MATCH {
            let mut matched_players = Vec::new();

            for _ in 0..PLAYERS_PER_MATCH {
                if let Some(player_id) = self.queue.pop_front() {
                    info!("Player {} put in match_players", player_id);
                    if let Some(player) = self.players.get(&player_id) {
                        matched_players.push((player_id, player.tx.clone()));
                    }
                }
            }

            let orchestrator_url = std::env::var("CHUNGUSTRATOR_URL")
                .unwrap_or_else(|_| "http://localhost:7000/create".to_string());

            let http_response = self.http_client.post(orchestrator_url).send().await;
            match http_response {
                Ok(response) => match response.json::<OrchestratorContainerResponse>().await {
                    Ok(OrchestratorContainerResponse::Success(data)) => {
                        info!("Game server container created with id {}", data.id);

                        for (player_id, player_tx) in matched_players {
                            info!("Sending server deets to {}", player_id);
                            player_tx.send(MatchmakingResponse::MatchCreated(MatchCreatedData {
                                ip: "kappa chungite".to_string(),
                                port: "penis".to_string(),
                            }));
                            self.players.remove(&player_id);
                        }
                    }
                    Ok(OrchestratorContainerResponse::Error(data)) => {
                        error!(
                            "Error creating game server, putting players back: {}",
                            data.error
                        );
                        for (player_id, _) in matched_players {
                            self.queue.push_back(player_id);
                        }
                    }
                    Err(e) => {
                        error!("Error converting response: {}", e);
                    }
                },
                Err(e) => {
                    error!("Error getting http response from orchestrator: {}", e);
                }
            }
        } else {
            info!("No players in queue D:");
        }
    }

    fn join_queue(&mut self, join_req: JoinRequest) -> Result<(), MatchmakingError> {
        let player_id = join_req.id.to_string();
        let player_tx = join_req.tx.clone();

        if self.queue.contains(&player_id) {
            if let Err(_) = player_tx.send(MatchmakingResponse::JoinError(
                "Already in queue".to_string(),
            )) {
                return Err(MatchmakingError::ChannelError);
            }
            return Err(MatchmakingError::AlreadyInQueue);
        }

        info!("Inserting {} into queue and players", &player_id,);
        self.queue.push_back(player_id.clone());
        self.players.insert(
            player_id.clone(),
            Player {
                id: player_id.clone(),
                tx: player_tx.clone(),
            },
        );

        let queue_pos = self.queue.len();
        if let Err(_) = player_tx.send(MatchmakingResponse::JoinSuccess(queue_pos)) {
            return Err(MatchmakingError::ChannelError);
        }

        Ok(())
    }

    fn leave_queue(&mut self, leave_req: LeaveRequest) -> Result<(), MatchmakingError> {
        let player_id = leave_req.id;

        if let Some(index) = self.queue.iter().position(|val| val == &player_id) {
            if let Some(player) = self.players.get(&player_id) {
                if let Err(_) = player.tx.send(MatchmakingResponse::LeaveSuccess) {
                    return Err(MatchmakingError::ChannelError);
                }
            } else {
                return Err(MatchmakingError::NotFound);
            }
            info!("Removing player {} from queue", &player_id);
            self.queue.remove(index);
            self.players.remove(&player_id);
        } else {
            error!("Cannot remove player {}, not found in queue", &player_id);
            return Err(MatchmakingError::NotFound);
        }

        Ok(())
    }

    fn get_queue_position(&self, position_req: PositionRequest) -> Result<(), MatchmakingError> {
        let player_id = position_req.id;

        if let Some(index) = self.queue.iter().position(|val| val == &player_id) {
            if let Some(player) = self.players.get(&player_id) {
                if let Err(_) = player.tx.send(MatchmakingResponse::PositionSuccess(index)) {
                    return Err(MatchmakingError::ChannelError);
                }
            } else {
                return Err(MatchmakingError::NotFound);
            }
        } else {
            return Err(MatchmakingError::NotFound);
        }

        Ok(())
    }
}
