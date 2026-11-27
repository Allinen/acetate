-- Proposals: changes to a track suggested by a tool (a script, a language model) instead of
-- made, for a person to adopt or reject on the Review page. A flag of kind `proposal`; what
-- it proposes and why is in `related` (see `db::review::Proposal`). Unlike other kinds, a
-- track can have several open at once: one per `related.key`, which the proposer picks so
-- that proposing the same thing again doesn't add it twice.

INSERT INTO vocabulary (list, word, label, description, locked, position, data)
SELECT 'review_kind', 'proposal', 'Proposal', 'Changes suggested by a tool, to adopt or reject', true,
       COALESCE(max(position), 0) + 1, jsonb_build_object('automatic', false, 'checked', true)
FROM vocabulary WHERE list = 'review_kind';

CREATE UNIQUE INDEX review_flags_open_proposal_key ON review_flags (track_id, (related ->> 'key'))
    WHERE kind = 'proposal' AND resolved_at IS NULL;
