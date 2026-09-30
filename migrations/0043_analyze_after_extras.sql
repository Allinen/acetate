-- Statistics for the columns 0042 added. `tracks.became_extra_id` is in the tracks'
-- visibility rule (`IS NULL`), and `albums.all_tracks_extras` in the albums'; a column
-- that was just added has none, and nothing gathers them until many rows change. Without
-- them the planner guesses a null test keeps about 0.5% of rows: it took nearly every
-- track for hidden, and planned for a handful, so loading the catalog of a large library
-- took minutes. Gathered here (a few seconds).
--
-- A migration that adds a column a policy (or a hot query) filters on should do the same.

ANALYZE tracks;
ANALYZE albums;
