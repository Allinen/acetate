-- The catalog's artist aliases are the listed ones only (as the artist's page and the
-- server's search have them): a name taken off an artist is hidden, not deleted (old
-- revisions and credits still point at it), and it no longer shows up in clients'
-- search. Clients take the whole catalog once, so the copies they keep drop them too.
SELECT log_change('everything', NULL);
