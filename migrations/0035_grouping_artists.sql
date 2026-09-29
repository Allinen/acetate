-- Who's in a grouping (a collective's members), said explicitly: someone credited on one
-- of its songs isn't necessarily part of it (a feature, a producer). Added and taken out
-- by hand, each change in `grouping_log` (`artist_id` there says who).

CREATE TABLE grouping_artists (
    grouping_id uuid NOT NULL REFERENCES groupings (id),
    artist_id   uuid NOT NULL REFERENCES artists (id),
    added_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (grouping_id, artist_id)
);
CREATE INDEX grouping_artists_artist_idx ON grouping_artists (artist_id);

-- Artists are visible to everyone logged in (see 0027), and so are groupings: so is who's
-- in them (a share's guest sees none).
ALTER TABLE grouping_artists ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON grouping_artists TO acetate_app USING ((SELECT current_app_share()) IS NULL);

ALTER TABLE grouping_log ADD COLUMN artist_id uuid;

-- The catalog lists every grouping with its members: a change is a grouping change.
CREATE FUNCTION grouping_artists_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('grouping', COALESCE(NEW.grouping_id, OLD.grouping_id));
    RETURN NULL;
END
$$;
CREATE TRIGGER grouping_artists_log_change AFTER INSERT OR UPDATE OR DELETE ON grouping_artists
    FOR EACH ROW EXECUTE FUNCTION grouping_artists_log_change();
