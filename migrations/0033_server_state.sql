-- Things the server keeps between runs, each under a name, as JSON (see `db::state`).
-- The first: how much was uploaded today (`api::upload_limits`), so a restart doesn't
-- start the day's count again. Nothing here is about what a user may see, so no
-- row-level security (the table is in `TABLES_WITHOUT_RLS`). Entries are updated, never
-- removed.

CREATE TABLE server_state (
    key        text PRIMARY KEY,
    value      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

REVOKE DELETE ON server_state FROM acetate_app;
