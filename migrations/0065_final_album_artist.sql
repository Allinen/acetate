-- Where an album or artist went: itself, or (merged) the one it was merged into, following
-- merges of merges. Like `final_track`: extras stay linked to what they were linked to, and
-- are shown on the one it became (so undoing a merge needs nothing).
-- SECURITY DEFINER: merged rows are hidden from the app; this only gives an id, and the
-- row it names is then read as the user may see it.
CREATE FUNCTION final_album(p_album uuid) RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    WITH RECURSIVE chain (id, next, depth) AS (
        SELECT id, merged_into, 0 FROM albums WHERE id = p_album
        UNION ALL
        SELECT a.id, a.merged_into, c.depth + 1 FROM chain c JOIN albums a ON a.id = c.next WHERE c.depth < 50
    )
    SELECT id FROM chain WHERE next IS NULL LIMIT 1
$$;
GRANT EXECUTE ON FUNCTION final_album(uuid) TO acetate_app;

CREATE FUNCTION final_artist(p_artist uuid) RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    WITH RECURSIVE chain (id, next, depth) AS (
        SELECT id, merged_into, 0 FROM artists WHERE id = p_artist
        UNION ALL
        SELECT a.id, a.merged_into, c.depth + 1 FROM chain c JOIN artists a ON a.id = c.next WHERE c.depth < 50
    )
    SELECT id FROM chain WHERE next IS NULL LIMIT 1
$$;
GRANT EXECUTE ON FUNCTION final_artist(uuid) TO acetate_app;
