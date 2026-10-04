# Users and permissions

Create the first admin from the command line (which acts as the system, with no login):

```sh
acetate user create alice --group admin     # asks for the password (or --password-stdin)
acetate user list
acetate user set-groups alice admin user
acetate user set-password alice             # logs them out everywhere
```

Users belong to **groups**; groups have **permissions**. Built-in groups: `admin` (everything),
`user` (a regular listener) and `guest` (stream only). Admins can manage users and groups
through the API too.

## Permissions

| Permission | Allows |
|---|---|
| `admin` | everything, including server settings, purging and storage |
| `browse` | see the library: list, search and view |
| `stream` | stream audio |
| `upload` | upload and import new files |
| `edit_metadata` | edit tracks, albums, artists, covers (creates revisions) |
| `edit_relations` | related tracks and credits |
| `edit_sources` | external sources of tracks, albums and artists |
| `edit_files` | correct what's recorded about a file (original file, name) |
| `replace_file` | replace a track's file with a better one |
| `view_history` | see revision history |
| `restore_revision` | restore an older revision |
| `manage_access` | restrict tracks, and change which groups can see them |
| `manage_users` | create users, manage groups and their permissions |
| `download_original` | download the untouched original (not yet enforced separately from streaming) |
| `share` | create share links |

## Restricting tracks

With `manage_access`, a track can be made visible to only some groups; others never see it
in lists, search or the catalog. `acetate ingest --restrict-to group1,group2` creates tracks
restricted from the start.
