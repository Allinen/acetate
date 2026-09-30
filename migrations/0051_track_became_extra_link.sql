-- A track made into an extra is seen by nobody (0042), so an old link to it was a plain
-- 404. This gives the extra it became, so the link can lead there: only its id, and the
-- caller reads the extra as the user may see it (`db::extras::became`).
CREATE FUNCTION track_became_extra(p_track uuid) RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT became_extra_id FROM tracks WHERE id = p_track AND current_app_share() IS NULL
$$;
GRANT EXECUTE ON FUNCTION track_became_extra(uuid) TO acetate_app;
