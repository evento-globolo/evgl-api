# evgl-api

Rust API and WebSocket job service for Evento Globolo.

## What is implemented

- JWT-authenticated canonical event create, bounded list, and read operations
- OAuth connection start/callback flows for Eventbrite, Meetup, and Meta
- account discovery (Eventbrite organizations and Facebook Pages)
- AES-256-GCM token envelopes bound to user/provider/account AAD
- idempotent cross-post jobs and per-target receipts
- owner-scoped event updates and job-specific WebSocket progress
- capability-aware Craigslist manual handoffs
- signed generic webhook destinations

## Run

```sh
docker compose up -d postgres
export DATABASE_URL=postgres://evgl:evgl@localhost:5432/evgl
export JWT_SECRET='replace-me'
export TOKEN_VAULT_KEY="$(openssl rand -base64 32)"
sqlx migrate run
cargo run
```

OAuth client secrets and the token-vault key are environment-only.
Browser access is restricted to the exact `APP_WEB_URL` origin; provider target payloads never carry credentials.
