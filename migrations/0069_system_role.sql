-- The server logs in as `acetate_app` itself (it used to log in as the owner and `SET ROLE`,
-- which SQL injected into a query could undo with `RESET ROLE`, and the owner is a
-- superuser in the Docker setup). A few places have to see or change what row-level security
-- hides (a cascade onto a hidden track, "is this file stored already?"): they switch to
-- `acetate_system` for a moment (`db::as_system`) instead of to the owner. It bypasses
-- row-level security and can read and write rows, and nothing more: no changing tables,
-- no roles, not a superuser.
--
-- `acetate_app` becomes a role that can log in; the server gives it a password of its own
-- when it starts (nothing to configure), and the owner's pool, which the CLI and the
-- migrations use, is the owner as before.

-- (Roles belong to the whole Postgres, and the tests migrate databases at the same time.)
SELECT pg_advisory_xact_lock(7305);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'acetate_system') THEN
        CREATE ROLE acetate_system NOLOGIN BYPASSRLS;
    END IF;
END
$$;

GRANT USAGE ON SCHEMA public TO acetate_system;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO acetate_system;
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO acetate_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO acetate_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO acetate_system;

-- The app can become it (SET ROLE) but doesn't have its rights otherwise.
GRANT acetate_system TO acetate_app WITH INHERIT FALSE, SET TRUE;

ALTER ROLE acetate_app LOGIN;
