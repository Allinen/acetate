-- Whether a file is an original file (an "og file", `files.original_filename`, see 0013)
-- can be corrected: ingest only knows it from a tag, and some original files came without
-- one. It's a very specific fix, so it has a permission of its own (`edit_files`; admins
-- have it, as every permission). Files stay otherwise unchangeable: the app may update
-- this one column too, and every change is kept here (`db::og_files`).

INSERT INTO permissions (permission, description) VALUES
    ('edit_files', 'Correct what''s recorded about a file itself (whether it''s an original file, and its name). Only for someone who knows where the files came from');

CREATE TABLE file_original_filename_changes (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    file_id    uuid NOT NULL REFERENCES files (id),
    -- NULL: not an original file.
    old_name   text,
    new_name   text,
    changed_at timestamptz NOT NULL DEFAULT now(),
    changed_by uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user(),
    reason     text NOT NULL DEFAULT ''
);

CREATE INDEX file_original_filename_changes_file_idx ON file_original_filename_changes (file_id, id DESC);

-- Visible with the file; never rewritten.
ALTER TABLE file_original_filename_changes ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON file_original_filename_changes TO acetate_app
    USING (EXISTS (SELECT 1 FROM files f WHERE f.id = file_id));
REVOKE UPDATE, DELETE ON file_original_filename_changes FROM acetate_app;

-- Which files: those the user can see, as the trash's update policy already says.
GRANT UPDATE (original_filename) ON files TO acetate_app;

-- The catalog lists each track's `original_filename` (its file's): a change there changes
-- the tracks playing that file.
CREATE FUNCTION files_log_original_filename() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF NEW.original_filename IS DISTINCT FROM OLD.original_filename THEN
        INSERT INTO library_changes (kind, entity_id)
        SELECT 'track', t.id FROM tracks t WHERE t.file_id = NEW.id;
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER files_log_original_filename AFTER UPDATE OF original_filename ON files
    FOR EACH ROW EXECUTE FUNCTION files_log_original_filename();
