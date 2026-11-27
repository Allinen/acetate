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
through the API too, and in the desktop app: Settings → Users shows everyone and the groups they are in (click a group to put someone in it or take them out, then save) and the groups with the permissions each one allows (hover a permission for what it does). You can only hand out permissions you have yourself, and unless you are an admin, only put people in groups you are in yourself (a group also opens the tracks restricted to it).

### Tokens for bots and scripts

A bot or script authenticates with a session token, sent as `Authorization: Bearer <token>`.
To get one, give the bot its own user (with only the groups it needs), then in Settings → Users
click **Copy token…** under it and enter its password: the app logs in as that user and copies
the token to the clipboard without showing it. The token is a session like any other login: it
lasts as long as it keeps being used (it expires after 30 days unused), and changing the user's
password ends it. Without the app, `POST /api/auth/login`
with `{"username": "...", "password": "..."}` answers with the same token.

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
| `download_original` | stream and download the original file. Without it, `stream` plays a transcoded copy (Opus) instead; see [Quality and downloads](../listening.md#quality-and-downloads) |
| `share` | create share links |

## Restricting tracks

With `manage_access`, a track can be made visible to only some groups; others never see it
in lists, search or the catalog. `acetate ingest --restrict-to group1,group2` creates tracks
restricted from the start.
