use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PrefixComponent,
};

use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, time};
use tracing::{error, info};

use crate::verification::generate_verification_code;

// Include the generated protobuf code
pub mod chungustrator {
    tonic::include_proto!("chungustrator");
}

use chungustrator::chungustrator_client::ChungustratorClient;
use tonic::transport::Channel;

pub struct Matchmaker {
    queue: VecDeque<String>,
    players: HashMap<String, Player>,
    receiver: mpsc::UnboundedReceiver<MatchmakingMessage>,
    grpc_client: ChungustratorClient<Channel>,
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
    pub wan_ip: String,
    pub lan_ip: String,
    pub port: u16,
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

impl Matchmaker {
    pub fn new(
        rx: mpsc::UnboundedReceiver<MatchmakingMessage>,
        grpc_client: ChungustratorClient<Channel>,
    ) {
        let mut matchmaker = Matchmaker {
            queue: VecDeque::new(),
            players: HashMap::new(),
            receiver: rx,
            grpc_client,
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
            let mut verification_codes = HashMap::new();
            let mut used_codes = HashSet::new();

            for _ in 0..PLAYERS_PER_MATCH {
                if let Some(player_id) = self.queue.pop_front() {
                    info!("Player {} put in match_players", player_id);
                    if let Some(player) = self.players.get(&player_id) {
                        matched_players.push((player_id.clone(), player.tx.clone()));

                        // Generate unique verification code for this player
                        let mut verification_code = generate_verification_code();
                        while used_codes.contains(&verification_code) {
                            verification_code = generate_verification_code();
                        }
                        used_codes.insert(verification_code.clone());
                        verification_codes.insert(player_id, verification_code);
                    }
                }
            }

            // Create the gRPC request
            let request = tonic::Request::new(chungustrator::MatchRequest { verification_codes });

            match self.grpc_client.create_match(request).await {
                Ok(response) => {
                    let match_data = response.into_inner();
                    info!("Game server container created with id {}", match_data.id);

                    for (player_id, player_tx) in matched_players {
                        info!("Sending server deets to {}", player_id);
                        if let Err(e) =
                            player_tx.send(MatchmakingResponse::MatchCreated(MatchCreatedData {
                                wan_ip: match_data.ip_address.clone(),
                                lan_ip: match_data.lan_address.clone(),
                                port: match_data.port as u16,
                            }))
                        {
                            error!("Error sending MatchCreatedData to player: {}", e);
                        }
                        self.players.remove(&player_id);
                    }
                }
                Err(status) => {
                    error!(
                        "Error creating game server via gRPC, putting players back: {}",
                        status
                    );
                    for (player_id, _) in matched_players {
                        self.queue.push_back(player_id);
                    }
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
