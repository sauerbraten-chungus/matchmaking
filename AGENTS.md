# CLAUDE.md — matchmaking

## Overview

WebSocket-based matchmaking queue. Batches players and requests game server provisioning from chungustrator via gRPC. Generates 6-digit verification codes for player authentication.

- **Language**: Rust (Axum + Tonic)
- **Port**: 5000
- **Status**: Work in progress

## Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/` | GET | Health check |
| `/ws?player_id=xyz` | WebSocket | Matchmaking queue |

## WebSocket Protocol

**Client → Server:**
- `{"type": "get_queue_position"}` — request current position
- `{"type": "leave_queue"}` — leave queue

**Server → Client:**
- `JoinQueue` — joined successfully, returns queue position
- `MatchFound` — match is being created
- `MatchCreated` — match ready, returns `{wan_ip, lan_ip, port, verification_code}`
- `QueuePosition` — current position response
- `LeaveQueue` — left queue
- `JoinError` — error (e.g., already in queue)

## Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `CHUNGUSTRATOR_URL` | gRPC address of orchestrator | `http://127.0.0.1:7000` |

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | HTTP/WebSocket server, socket lifecycle |
| `src/matchmaker.rs` | Matchmaker actor: queue, batching, gRPC calls |
| `src/verification.rs` | 6-digit verification code generation |
| `proto/chungustrator.proto` | gRPC service definition for CreateMatch |
| `build.rs` | Proto compilation |
| `Dockerfile` | Multi-stage build (rust:1.88 + protoc → debian:bookworm-slim) |

## Development

```bash
cargo build
cargo run             # binds 0.0.0.0:5000
cargo fmt && cargo clippy
```

## Architecture Notes

- Matchmaker runs as a Tokio actor with mpsc channels
- Processes queue every 5 seconds
- `PLAYERS_PER_MATCH = 1` (TODO: increase for real batching)
- On match: generates unique verification codes per player, calls `chungustrator.CreateMatch` gRPC
- On gRPC failure: re-queues players for retry
- In-memory only — restarting loses queue state
- No tests, no graceful shutdown
