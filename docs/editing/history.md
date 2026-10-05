# History and undo

Almost everything has a history. A track's, album's or artist's page shows every revision,
newest first, under **History**: who, when, how (an edit, an import, a merge…), why, and
what changed in words: `title: "A" → "B"`. On album and artist pages, `history` on the
commandline opens it.

- **Restore** on a revision writes a *new* revision with that one's data. The ones in between
  stay in the history. It asks first, and needs `restore_revision`.
- Seeing history needs `view_history`.
- Track and album **merges** can be undone in edit mode, under **Merged into this track**
  or **Merged into this album** on the kept one's page. An album merge shows what goes back
  and what stays before anything happens.
- Saves give an optional **reason** (`why:` on a commandline); it appears in the history.
  It goes with a change to the revision's own data (a title, artists, names…): a reason on
  its own is nothing to save.


What *doesn't* have a history? 
- Some less important things like which version is the main one

Nobody can wipe metadata for good: either don't give the permission, or revert. Take backups
all the same.
