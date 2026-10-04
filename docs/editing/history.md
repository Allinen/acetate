# History and undo

Almost everything has a history. A track's page shows every revision, newest first: who,
when, how (an edit, an import, a merge…), why, and what changed in words:
`title: "A" → "B"`.

- **Restore** on a revision writes a *new* revision with that one's data. The ones in between
  stay in the history. It asks first, and needs `restore_revision`.
- Seeing history needs `view_history`.
- Track and album **merges** can be undone from the kept track's page or album's page.
- Saves give an optional **reason**; it appears in the history.


What *doesn't* have a history? 
- Some less important things like which version is the main one

Nobody can wipe metadata for good: either don't give the permission, or revert. Take backups
all the same.
