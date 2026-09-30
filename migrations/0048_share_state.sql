-- Whether a share link still works, and if not, why: for the list of shares
-- (`db::shares::list`), so a UI can say which links stopped working. A link can stop
-- without being revoked or expiring: its creator lost the permission to share, or can't
-- see what it shares anymore (a share looks through its creator's eyes: migration 0008).
--
-- 'revoked', 'expired', 'creator_cannot_share', 'target_hidden' (nothing it shares is
-- visible to its creator, or it's gone: merged away, become an extra) or 'active'.
--
-- SECURITY DEFINER: it looks at the creator's groups and at tracks the one listing may
-- not see, and answers only with the state. The track check is the tracks policy's
-- (0042) with the creator as the viewer, plus having a file (a share plays only those).
CREATE FUNCTION share_state(p_share_id uuid) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    WITH s AS (SELECT * FROM shares WHERE id = p_share_id),
    perms AS (
        SELECT gp.permission FROM s
        JOIN user_groups ug ON ug.user_id = s.created_by
        JOIN group_permissions gp ON gp.group_id = ug.group_id
    )
    SELECT CASE
        WHEN s.revoked_at IS NOT NULL THEN 'revoked'
        WHEN s.expires_at IS NOT NULL AND s.expires_at <= now() THEN 'expired'
        WHEN NOT EXISTS (SELECT 1 FROM perms WHERE permission IN ('share', 'admin')) THEN 'creator_cannot_share'
        WHEN NOT EXISTS (
            SELECT 1 FROM tracks t
            WHERE t.id IN (SELECT s.track_id UNION ALL SELECT track_id FROM album_tracks WHERE album_id = s.album_id)
              AND t.file_id IS NOT NULL
              AND t.became_extra_id IS NULL
              AND (NOT t.is_restricted
                   OR EXISTS (SELECT 1 FROM perms WHERE permission = 'admin')
                   OR EXISTS (SELECT 1 FROM track_access ta
                              JOIN user_groups ug ON ug.group_id = ta.group_id
                              WHERE ta.track_id = t.id AND ug.user_id = s.created_by))
        ) THEN 'target_hidden'
        ELSE 'active'
    END
    FROM s
$$;
GRANT EXECUTE ON FUNCTION share_state(uuid) TO acetate_app;
