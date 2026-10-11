-- One table for the words Acetate knows: statuses, credit roles, album kinds, relation
-- kinds, kinds of review flag and kinds of extra, and the words that make a title another
-- version of a song. They were a table each (and extras' kinds
-- free text); now an admin can add, rename and remove them from the app.
--
-- Some words the code is built around are `locked`: they can't be removed, renamed or
-- unlocked ("primary", "featured", "released"...; the checks the server runs itself).
-- Everything else is the library's own.
--
-- Rows that use a word refer to it by (list, word): a column naming the list, constant,
-- next to the one with the word. A rename follows by ON UPDATE CASCADE; a word in use
-- can't be removed.

CREATE TABLE vocabulary (
    list        text NOT NULL CHECK (list IN ('status', 'credit_role', 'album_kind', 'relation_kind', 'review_kind', 'extra_kind', 'version_word')),
    word        text NOT NULL CHECK (word <> '' AND word = btrim(word)),
    -- What people read when the word isn't it (a kind of flag: "Fix later"). Empty: the word.
    label       text NOT NULL DEFAULT '',
    description text NOT NULL DEFAULT '',
    locked      boolean NOT NULL DEFAULT false,
    -- The order to show them in (then by word).
    position    integer NOT NULL DEFAULT 0,
    -- Relation kinds: the word for the other direction (its own, for a symmetric kind).
    inverse     text,
    -- Per list. Relation kinds: {"template": "%A is a remix of %B"}. Kinds of review flag:
    -- {"automatic": bool, "checked": bool}: raised by the server itself, and whether it checks.
    data        jsonb NOT NULL DEFAULT '{}',
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz,
    PRIMARY KEY (list, word),
    CHECK ((list = 'relation_kind') = (inverse IS NOT NULL)),
    CHECK (list <> 'relation_kind' OR (data ->> 'template') LIKE '%\%A%' AND (data ->> 'template') LIKE '%\%B%'),
    CHECK (list <> 'review_kind' OR (data ? 'automatic' AND data ? 'checked')),
    CHECK (list <> 'version_word' OR data ? 'mark'),
    FOREIGN KEY (list, inverse) REFERENCES vocabulary (list, word) ON UPDATE CASCADE DEFERRABLE INITIALLY DEFERRED
);

CREATE UNIQUE INDEX vocabulary_label_key ON vocabulary (list, lower(label)) WHERE label <> '';

INSERT INTO vocabulary (list, word, description, locked)
SELECT 'status', status, description, status IN ('released') FROM track_statuses;
INSERT INTO vocabulary (list, word, locked)
SELECT 'credit_role', role, role IN ('primary', 'featured', 'producer') FROM credit_roles;
INSERT INTO vocabulary (list, word, locked)
SELECT 'album_kind', kind, kind IN ('album', 'single') FROM album_kinds;
INSERT INTO vocabulary (list, word, inverse, data, locked)
SELECT 'relation_kind', kind, inverse_kind, jsonb_build_object('template', template),
       kind IN ('remix_of', 'remix_to', 'instrumental_of', 'instrumental_to', 'acapella_of', 'acapella_to', 'alternate')
FROM relation_kinds;
INSERT INTO vocabulary (list, word, label, position, data, locked, created_by, created_at)
SELECT 'review_kind', kind, name, position, jsonb_build_object('automatic', automatic, 'checked', checked), NOT custom, created_by, created_at
FROM review_kinds;
-- The kinds of extra the clients suggested, and any that people made up since.
INSERT INTO vocabulary (list, word, position, locked)
SELECT 'extra_kind', kind, COALESCE(array_position(ARRAY['vocals', 'instrumental', 'stems', 'sample', 'clip', 'video', 'project', 'other'], kind), 100), kind IN ('video', 'other')
FROM (SELECT unnest(ARRAY['vocals', 'instrumental', 'stems', 'sample', 'clip', 'video', 'project', 'other']) AS kind
      UNION SELECT DISTINCT kind FROM extras) k;

-- Words in a title that make it another version of the song ("Song" and "Song (Instrumental)"
-- aren't duplicates): each word (several in a row are matched as a phrase) has a mark, so
-- spellings of the same thing agree ("remastered" and "remaster"). `substring`: found
-- anywhere in the title, for languages written without spaces between words.
INSERT INTO vocabulary (list, word, data) VALUES
    ('version_word', 'instrumental', jsonb_build_object('mark', 'instrumental')),
    ('version_word', 'inst', jsonb_build_object('mark', 'instrumental')),
    ('version_word', 'off vocal', jsonb_build_object('mark', 'instrumental')),
    ('version_word', 'karaoke', jsonb_build_object('mark', 'instrumental')),
    ('version_word', 'a cappella', jsonb_build_object('mark', 'a cappella')),
    ('version_word', 'acapella', jsonb_build_object('mark', 'a cappella')),
    ('version_word', 'acappella', jsonb_build_object('mark', 'a cappella')),
    ('version_word', 'sped', jsonb_build_object('mark', 'sped up')),
    ('version_word', 'speed up', jsonb_build_object('mark', 'sped up')),
    ('version_word', 'spedup', jsonb_build_object('mark', 'sped up')),
    ('version_word', 'nightcore', jsonb_build_object('mark', 'sped up')),
    ('version_word', 'slowed', jsonb_build_object('mark', 'slowed')),
    ('version_word', 'reverb', jsonb_build_object('mark', 'reverb')),
    ('version_word', 'remaster', jsonb_build_object('mark', 'remaster')),
    ('version_word', 'remastered', jsonb_build_object('mark', 'remaster')),
    ('version_word', 'remix', jsonb_build_object('mark', 'remix')),
    ('version_word', 'mix', jsonb_build_object('mark', 'mix')),
    ('version_word', 'edit', jsonb_build_object('mark', 'edit')),
    ('version_word', 'extended', jsonb_build_object('mark', 'extended')),
    ('version_word', 'live', jsonb_build_object('mark', 'live')),
    ('version_word', 'acoustic', jsonb_build_object('mark', 'acoustic')),
    ('version_word', 'unplugged', jsonb_build_object('mark', 'acoustic')),
    ('version_word', 'demo', jsonb_build_object('mark', 'demo')),
    ('version_word', '8d', jsonb_build_object('mark', '8d')),
    ('version_word', '伴奏', jsonb_build_object('mark', 'instrumental', 'substring', true)),
    ('version_word', '纯音乐', jsonb_build_object('mark', 'instrumental', 'substring', true)),
    ('version_word', 'インスト', jsonb_build_object('mark', 'instrumental', 'substring', true)),
    ('version_word', '加速', jsonb_build_object('mark', 'sped up', 'substring', true)),
    ('version_word', '降速', jsonb_build_object('mark', 'slowed', 'substring', true)),
    ('version_word', '慢速', jsonb_build_object('mark', 'slowed', 'substring', true)),
    ('version_word', '现场', jsonb_build_object('mark', 'live', 'substring', true)),
    ('version_word', 'ライブ', jsonb_build_object('mark', 'live', 'substring', true)),
    ('version_word', '重制', jsonb_build_object('mark', 'remaster', 'substring', true)),
    ('version_word', '混音', jsonb_build_object('mark', 'remix', 'substring', true));

-- An inverse's inverse is the word itself.
CREATE FUNCTION check_relation_kind_pair() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- (Renamed or removed since the change this is for, in the same transaction.)
    IF NOT EXISTS (SELECT 1 FROM vocabulary v WHERE v.list = NEW.list AND v.word = NEW.word AND v.inverse IS NOT DISTINCT FROM NEW.inverse) THEN
        RETURN NULL;
    END IF;
    IF NEW.list = 'relation_kind' AND NOT EXISTS (
        SELECT 1 FROM vocabulary v WHERE v.list = 'relation_kind' AND v.word = NEW.inverse AND v.inverse = NEW.word
    ) THEN
        RAISE EXCEPTION 'the relation kind "%" and its inverse "%" must be each other''s inverse', NEW.word, NEW.inverse
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NULL;
END
$$;
CREATE CONSTRAINT TRIGGER vocabulary_relation_pair AFTER INSERT OR UPDATE ON vocabulary
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_relation_kind_pair();

-- Locked words stay.
CREATE FUNCTION vocabulary_locked() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.locked THEN
            RAISE EXCEPTION 'the % "%" is one the app is built around: it can''t be removed', replace(OLD.list, '_', ' '), OLD.word
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN OLD;
    END IF;
    IF OLD.locked AND (NEW.locked IS DISTINCT FROM OLD.locked OR NEW.word <> OLD.word OR NEW.list <> OLD.list OR NEW.inverse IS DISTINCT FROM OLD.inverse) THEN
        RAISE EXCEPTION 'the % "%" is one the app is built around: it can''t be renamed', replace(OLD.list, '_', ' '), OLD.word
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER vocabulary_locked BEFORE UPDATE OR DELETE ON vocabulary FOR EACH ROW EXECUTE FUNCTION vocabulary_locked();

-- Rows that use a word.
ALTER TABLE tracks ADD COLUMN status_list text NOT NULL DEFAULT 'status' CHECK (status_list = 'status');
ALTER TABLE tracks DROP CONSTRAINT tracks_status_fkey;
ALTER TABLE tracks ADD FOREIGN KEY (status_list, status) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE track_credits ADD COLUMN role_list text NOT NULL DEFAULT 'credit_role' CHECK (role_list = 'credit_role');
ALTER TABLE track_credits DROP CONSTRAINT track_credits_role_fkey;
ALTER TABLE track_credits ADD FOREIGN KEY (role_list, role) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE album_credits ADD COLUMN role_list text NOT NULL DEFAULT 'credit_role' CHECK (role_list = 'credit_role');
ALTER TABLE album_credits DROP CONSTRAINT album_credits_role_fkey;
ALTER TABLE album_credits ADD FOREIGN KEY (role_list, role) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE albums ADD COLUMN kind_list text NOT NULL DEFAULT 'album_kind' CHECK (kind_list = 'album_kind');
ALTER TABLE albums DROP CONSTRAINT albums_kind_fkey;
ALTER TABLE albums ADD FOREIGN KEY (kind_list, kind) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE track_relations ADD COLUMN kind_list text NOT NULL DEFAULT 'relation_kind' CHECK (kind_list = 'relation_kind');
ALTER TABLE track_relations DROP CONSTRAINT track_relations_kind_fkey;
ALTER TABLE track_relations ADD FOREIGN KEY (kind_list, kind) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE review_flags ADD COLUMN kind_list text NOT NULL DEFAULT 'review_kind' CHECK (kind_list = 'review_kind');
ALTER TABLE review_flags DROP CONSTRAINT review_flags_kind_fkey;
ALTER TABLE review_flags ADD FOREIGN KEY (kind_list, kind) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

ALTER TABLE extras ADD COLUMN kind_list text NOT NULL DEFAULT 'extra_kind' CHECK (kind_list = 'extra_kind');
ALTER TABLE extras ADD FOREIGN KEY (kind_list, kind) REFERENCES vocabulary (list, word) ON UPDATE CASCADE;

-- (A rename touches the rows that use the word, and so the flags' unique index and the
-- relations' pairs: the automatic kinds can't be renamed, being locked.)

CREATE OR REPLACE FUNCTION check_relation_pair(p_from uuid, p_kind text, p_to uuid) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    v_inverse    text;
    v_has_row    boolean;
    v_has_inverse boolean;
BEGIN
    SELECT inverse INTO v_inverse FROM vocabulary WHERE list = 'relation_kind' AND word = p_kind;

    v_has_row := EXISTS (
        SELECT 1 FROM track_relations
        WHERE from_track_id = p_from AND kind = p_kind AND to_track_id = p_to);
    v_has_inverse := EXISTS (
        SELECT 1 FROM track_relations
        WHERE from_track_id = p_to AND kind = v_inverse AND to_track_id = p_from);

    IF v_has_row <> v_has_inverse THEN
        RAISE EXCEPTION 'track relation (%, %, %) is missing its inverse (%, %, %)',
            p_kind, p_from, p_to, v_inverse, p_to, p_from
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
END;
$$;

CREATE OR REPLACE FUNCTION check_track(p_track uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    t record;
    main_album uuid;
    lacks text[] := '{}';
    missing text[];
    before text[];
BEGIN
    SELECT id, title, release_date, merged_into INTO t FROM tracks WHERE id = p_track;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    IF t.merged_into IS NULL THEN
        -- The main album, as the catalog chooses it: a real album before a single, then
        -- the oldest.
        SELECT at.album_id INTO main_album
        FROM album_tracks at
        JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
        WHERE at.track_id = p_track
        ORDER BY a.kind = 'single', a.id
        LIMIT 1;
        IF btrim(t.title) = '' THEN
            lacks := lacks || 'missing_title'::text;
        END IF;
        IF t.title ~* '(^|[[:space:](\[{-])(feat\.?|ft\.?|featuring|w/(?!o)|prod\.?|produced by)([[:space:]]|$)' THEN
            lacks := lacks || 'credits_in_title'::text;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM track_credits WHERE track_id = p_track) THEN
            lacks := lacks || 'missing_artists'::text;
        END IF;
        IF main_album IS NULL THEN
            lacks := lacks || 'missing_album'::text;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM covers
            WHERE trashed_at IS NULL AND (track_id = p_track OR (main_album IS NOT NULL AND album_id = main_album))
        ) THEN
            lacks := lacks || 'missing_cover'::text;
        END IF;
        IF COALESCE(btrim(t.release_date), '') = '' THEN
            lacks := lacks || 'missing_date'::text;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM sources s
            WHERE s.track_id = p_track AND s.trashed_at IS NULL
              AND NOT (s.platform = 'custom' AND (
                  SELECT COALESCE(btrim(ss.data ->> 'description'), '') = '' AND COALESCE(btrim(ss.data ->> 'url'), '') = ''
                  FROM source_snapshots ss WHERE ss.source_id = s.id
                  ORDER BY ss.captured_at DESC, ss.id DESC LIMIT 1
              ))
        ) THEN
            lacks := lacks || 'weak_source'::text;
        END IF;
    END IF;
    -- Only what's checked for.
    missing := ARRAY(SELECT k.word FROM vocabulary k
                     WHERE k.list = 'review_kind' AND (k.data ->> 'automatic')::boolean AND (k.data ->> 'checked')::boolean AND k.word = ANY (lacks));
    before := COALESCE((SELECT m.missing FROM track_missing m WHERE m.track_id = p_track), '{}');

    -- Newly found: flagged.
    INSERT INTO review_flags (kind, track_id)
    SELECT k, p_track FROM unnest(missing) k
    WHERE k <> ALL (before)
    ON CONFLICT (track_id, kind) WHERE resolved_at IS NULL AND kind IN (
        'missing_title', 'missing_artists', 'missing_album', 'missing_cover', 'missing_date', 'credits_in_title',
        'weak_source'
    ) DO NOTHING;
    -- Gone (or not checked anymore): its open flag is settled.
    UPDATE review_flags f
    SET resolved_at = now(), resolved_by = current_app_user(), resolution = 'fixed'
    FROM vocabulary k
    WHERE f.track_id = p_track AND f.resolved_at IS NULL
      AND k.list = 'review_kind' AND k.word = f.kind AND (k.data ->> 'automatic')::boolean AND f.kind <> ALL (missing);

    INSERT INTO track_missing (track_id, missing) VALUES (p_track, missing)
    ON CONFLICT (track_id) DO UPDATE SET missing = EXCLUDED.missing
    WHERE track_missing.missing IS DISTINCT FROM EXCLUDED.missing;
END
$$;

-- A custom kind of flag named `p_name`: its key is the name in lowercase words joined by
-- "_" ("Missing SoundCloud metadata" → missing_soundcloud_metadata). Refused if a kind has
-- that name or key already. Returns the key. (Anyone who edits metadata can make one in the
-- track editor; the other words are the admin's, see `vocabulary` below.)
CREATE OR REPLACE FUNCTION make_review_kind(p_name text) RETURNS text
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
    IF EXISTS (SELECT 1 FROM vocabulary WHERE list = 'review_kind' AND (word = v_kind OR lower(label) = lower(v_name))) THEN
        RAISE EXCEPTION 'there''s a kind of flag named "%" already', v_name USING ERRCODE = 'unique_violation';
    END IF;
    INSERT INTO vocabulary (list, word, label, position, data, created_by, created_at)
    VALUES ('review_kind', v_kind, v_name, (SELECT COALESCE(max(position), 0) + 1 FROM vocabulary WHERE list = 'review_kind'),
            jsonb_build_object('automatic', false, 'checked', true), current_app_user(), now());
    RETURN v_kind;
END
$$;

DROP TABLE review_kinds, relation_kinds, album_kinds, credit_roles, track_statuses;

-- The app reads the words, and an admin (checked in the API) changes the ones that aren't
-- locked: adding, describing and ordering them, and removing those not in use. Renaming
-- goes through `rename_vocabulary`, which moves the words' users with them.
GRANT SELECT ON vocabulary TO acetate_app;
GRANT INSERT (list, word, label, description, position, inverse, data, created_by, created_at) ON vocabulary TO acetate_app;
GRANT UPDATE (label, description, position, data) ON vocabulary TO acetate_app;
GRANT DELETE ON vocabulary TO acetate_app;

-- How many rows use a word (to say why it can't be removed).
CREATE FUNCTION vocabulary_uses(p_list text, p_word text) RETURNS bigint
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT CASE p_list
        WHEN 'status' THEN (SELECT count(*) FROM tracks WHERE status = p_word)
        WHEN 'credit_role' THEN (SELECT count(*) FROM track_credits WHERE role = p_word) + (SELECT count(*) FROM album_credits WHERE role = p_word)
        WHEN 'album_kind' THEN (SELECT count(*) FROM albums WHERE kind = p_word)
        WHEN 'relation_kind' THEN (SELECT count(*) FROM track_relations WHERE kind = p_word)
        WHEN 'review_kind' THEN (SELECT count(*) FROM review_flags WHERE kind = p_word)
        WHEN 'extra_kind' THEN (SELECT count(*) FROM extras WHERE kind = p_word)
        ELSE 0
    END
$$;
GRANT EXECUTE ON FUNCTION vocabulary_uses(text, text) TO acetate_app;
REVOKE EXECUTE ON FUNCTION vocabulary_uses(text, text) FROM PUBLIC;

-- Renames a word that isn't locked, everything that uses it following.
CREATE FUNCTION rename_vocabulary(p_list text, p_word text, p_new text) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF btrim(p_new) = '' OR p_new <> btrim(p_new) THEN
        RAISE EXCEPTION 'a word is not empty and has no spaces around it' USING ERRCODE = 'check_violation';
    END IF;
    IF EXISTS (SELECT 1 FROM vocabulary WHERE list = p_list AND word = p_new) THEN
        RAISE EXCEPTION 'there''s a % "%" already', replace(p_list, '_', ' '), p_new USING ERRCODE = 'unique_violation';
    END IF;
    UPDATE vocabulary SET word = p_new WHERE list = p_list AND word = p_word;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'no % "%"', replace(p_list, '_', ' '), p_word USING ERRCODE = 'no_data_found';
    END IF;
END
$$;
GRANT EXECUTE ON FUNCTION rename_vocabulary(text, text, text) TO acetate_app;
REVOKE EXECUTE ON FUNCTION rename_vocabulary(text, text, text) FROM PUBLIC;
