-- Kinds of review flag made by people, from the app (anyone who edits metadata): like
-- "Fix later", raised and settled by hand (in the track editor: `flag:…`, `-flag:…`), e.g.
-- "Missing SoundCloud metadata" for tracks whose SoundCloud source has only its id yet.
-- Listed with the others, after them. The built-in ones are still the owner's (the CLI).

ALTER TABLE review_kinds
    ADD COLUMN custom     boolean NOT NULL DEFAULT false,
    ADD COLUMN created_by uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN created_at timestamptz;

-- No two kinds people would read the same.
CREATE UNIQUE INDEX review_kinds_name_key ON review_kinds (lower(name));

-- A custom kind named `p_name`: its key is the name in lowercase words joined by "_"
-- ("Missing SoundCloud metadata" → missing_soundcloud_metadata). Refused if a kind has
-- that name or key already. Returns the key. The app can't write `review_kinds` itself;
-- this is the one way in for it.
CREATE FUNCTION make_review_kind(p_name text) RETURNS text
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    v_name text := btrim(regexp_replace(p_name, '\s+', ' ', 'g'));
    v_kind text := btrim(regexp_replace(lower(btrim(p_name)), '[^a-z0-9]+', '_', 'g'), '_');
BEGIN
    IF v_name = '' OR v_kind = '' THEN
        RAISE EXCEPTION 'a kind of flag needs a name with letters or digits' USING ERRCODE = 'check_violation';
    END IF;
    IF length(v_name) > 80 THEN
        RAISE EXCEPTION 'a kind of flag''s name is at most 80 characters' USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (SELECT 1 FROM review_kinds WHERE kind = v_kind OR lower(name) = lower(v_name)) THEN
        RAISE EXCEPTION 'there''s a kind of flag named "%" already', v_name USING ERRCODE = 'unique_violation';
    END IF;
    INSERT INTO review_kinds (kind, name, automatic, checked, position, custom, created_by, created_at)
    VALUES (v_kind, v_name, false, true, (SELECT COALESCE(max(position), 0) + 1 FROM review_kinds), true,
            current_app_user(), now());
    RETURN v_kind;
END
$$;
REVOKE ALL ON FUNCTION make_review_kind(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION make_review_kind(text) TO acetate_app;
