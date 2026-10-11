-- An empty grouping can be deleted (made by mistake): `db::groupings::delete` checks it
-- has nothing in it. Its log is kept, with a `deleted` entry holding its name and kind;
-- the log rows no longer point at a grouping, so nobody sees them but the database's owner.
-- The catalog is told as for any change (the trigger from 0034 already covers DELETE).

GRANT DELETE ON groupings TO acetate_app;
ALTER TABLE grouping_log DROP CONSTRAINT grouping_log_grouping_id_fkey;
