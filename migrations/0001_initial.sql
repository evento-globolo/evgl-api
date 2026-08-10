CREATE TABLE oauth_sessions (
  state_hash text PRIMARY KEY,
  user_id uuid NOT NULL,
  provider text NOT NULL,
  pkce_verifier text,
  uses_pkce boolean NOT NULL DEFAULT false,
  expires_at timestamptz NOT NULL
);
CREATE INDEX oauth_sessions_expiry_idx ON oauth_sessions (expires_at);

CREATE TABLE provider_connections (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  provider text NOT NULL,
  account_key text NOT NULL,
  display_name text NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  token_envelope text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, provider, account_key)
);
CREATE INDEX provider_connections_user_idx
  ON provider_connections (user_id, provider);

CREATE TABLE events (
  id uuid PRIMARY KEY,
  owner_id uuid NOT NULL,
  document jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX events_owner_idx ON events (owner_id, created_at DESC);

CREATE TABLE cross_post_jobs (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  event_id uuid NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  idempotency_key text NOT NULL,
  status text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, idempotency_key)
);
CREATE INDEX cross_post_jobs_event_idx ON cross_post_jobs (event_id, created_at DESC);

CREATE TABLE cross_post_targets (
  id uuid PRIMARY KEY,
  job_id uuid NOT NULL REFERENCES cross_post_jobs(id) ON DELETE CASCADE,
  connection_id uuid NOT NULL REFERENCES provider_connections(id) ON DELETE RESTRICT,
  provider text NOT NULL,
  options jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL,
  attempt integer NOT NULL DEFAULT 0,
  result jsonb,
  error text
);
CREATE INDEX cross_post_targets_job_idx ON cross_post_targets (job_id);
