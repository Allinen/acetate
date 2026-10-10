-- This server's identity: a random id made once, kept with the data (so a restored backup
-- is the same server, and a fresh database is another). Clients keep what they copy of the
-- library under it rather than under the address, so the address can change (a new
-- domain, a move to another machine) without fetching everything again. Returned by
-- login and `GET /api/auth/me`. See `db::state::instance_id`.

INSERT INTO server_state (key, value) VALUES ('instance_id', to_jsonb(gen_random_uuid()::text));
