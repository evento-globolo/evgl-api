# evgl-api

**Evento Globolo — Rust REST and WebSocket API server**

A global events operating system combining event discovery, publishing, RSVP, ticketing, community, venue, and organizer workflows.

## What is implemented

- JWT-authenticated canonical event CRUD
- OAuth connection start/callback flows for Eventbrite, Meetup, and Meta
- account discovery (Eventbrite organizations and Facebook Pages)
- AES-256-GCM token envelopes bound to user/provider/account AAD
- idempotent cross-post jobs and per-target receipts
- WebSocket job progress
- capability-aware Craigslist manual handoffs
- signed generic webhook destinations
- transactional ticket inventory and signed offline admissions (`TicketingService`, `AdmissionService`)

## GitHub target

`evento-globolo/evgl-api`

## Baseline

- Rust 2024 edition for backend and native components.
- Axum HTTP/WebSocket transport.
- Supabase/PostgreSQL configuration through `DATABASE_URL`, `SUPABASE_URL`, and environment-only secrets.
- OpenTelemetry-compatible tracing hooks.
- Docker, Nix, and GitHub Actions entry points.
- Contracts live in `evgl-interfaces`; shared behavior lives in `evgl-libs`.

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

```bash
cp .env.example .env 2>/dev/null || true
nix develop  # optional
cargo fmt --check 2>/dev/null || true
cargo test 2>/dev/null || true
```

### Ticketing and offline admission

`TicketingService` applies the canonical, pinned `evgl-interfaces` inventory
migration and exposes transaction-safe holds, checkout/payment idempotency,
expiry, cancellation/refund, fair waitlist promotion, and aggregate receipts.
`AdmissionService` applies the dependent admission migration and persists
entitlements, public verification keys, signed scanner receipts, revocation
epochs, and deterministic admission decisions.

```bash
EVGL_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres \
  cargo test --all-targets --all-features
```

## Environment secrets

Secrets live in this repo **encrypted** with [sops](https://github.com/getsops/sops) + [age](https://github.com/FiloSottile/age):
`env/enc/<dev|prod>.env.enc` is committed; `just env-use <name>` decrypts it to
`env/dec/<name>.env` (gitignored, mode 0600) and symlinks `./.env` to it. The
Nix dev shell provides the tooling, `just env-audit` runs keyless in CI, and
containers decrypt at `docker run` — never at build. See [`env/README.md`](env/README.md).
