-- Where a login came from, shown in the user's list of sessions.
-- (The client address as the server saw it; behind a reverse proxy, see ACETATE_TRUST_PROXY.)
ALTER TABLE sessions ADD COLUMN ip text;
