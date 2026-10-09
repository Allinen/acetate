# The Android app

A listening app for your server: browse the whole library, play it with the screen off,
keep music on the phone for when there's no connection. Library work (merging, review,
importing) stays on the desktop; for an account that may change the library, the phone
fixes what you notice while listening (see Editing).

It needs Android 10 or newer and a server that speaks catalog format 9 (the version
that adds each file's hash to the catalog: update the server first).

## Installing

There is no store listing. Download the `.apk` from the
[releases page](https://github.com/Allinen/acetate/releases) and open it on the phone (Android
asks once to allow installs from your browser or file manager), or build it yourself
([below](#building-it)). Updates are installed the same way, over the old one; they keep
your login and downloads as long as they're signed with the same key.

Log in with the server's address, your user and password. Plain `http://` works (a server on
your home network usually is), and so does `https://` with a certificate from a public
authority or one you installed on the phone yourself (Settings, Security, install a
certificate).

## What it does

- **Albums, Artists, Tracks, Playlists** along the bottom, and **Search**: one line in the
  same [query language](searching.md) as the desktop app (`artist:ann year:2020`), with the
  keys and the values in your library offered as chips above the keyboard. Words alone are a
  fuzzy search over everything.
- **Playing:** gapless, with the notification, the lock screen and headset buttons. The queue
  (what you added by hand, then the list you play from) comes back as it was when you reopen
  the app. Shuffle and repeat-one are on Now playing. Tap an artist's name anywhere to go to
  their page; tap a track's title on Now playing for the track's own page.
- **Stars and ratings** (half stars) on tracks, albums and artists, kept on the server.
  Liked songs is the starred tracks.
- **Playlists:** add tracks from the long-press menu, reorder with Move up / Move down, filter
  inside a playlist (play and shuffle then use only what's left).
- **Lyrics** full screen from Now playing, following the song; tap a line to go there.
- **Extras** (stems, vocals, videos) from albums, artists and tracks.
- **Sharing** through Android's share sheet: a link for someone with an account (it opens in
  their app), or a link anyone can listen with that expires when you say. `acetate://` links
  open albums and artists in the app.

## Editing

For accounts that may change the library there's an **Edit** switch at the top of the library
screens. While it's on, a track's long-press menu, its page and Now playing offer **Edit
track**; while it's off nothing can change by accident. It needs the server (the changes are
checked by it), so it does nothing offline.

The editor shows the track's fields as the changes will leave them (what a field was is under
it), with the review flags that are about it. Tap a field to change it: its line is put in
the box at the bottom, ready to finish, with the choices (artists, roles, statuses…) as chips
over the keyboard. The box speaks the same language as the desktop's editor
(`title:…`, `artist:Ann role:featured`, `date:2020`, `status:demo`, `genre:house`, `album:…`),
so a typed line and a tapped field are the same thing. Each change is staged as a chip
(tap one to take it back); under them is what it does, in the history's words, and what the
server's checks say, including which flags it fixes. **Save** writes it as one revision,
with an optional reason, and offers **Undo** for a few seconds (it puts the earlier revision
back as a new one; needs the right to restore revisions).

**Albums and artists** have forms of their own (Edit album, Edit artist on their pages): title,
kind, date and the credited artists (role, order, a new artist is made when you save) for an
album; name, what tells it apart and the other names for an artist. Each saves as one
revision, and if someone saved the same thing meanwhile it says so and offers their version.
An other name that's still credited somewhere stays, and says where. **Edit all tracks** on an
album opens the track editor for every track of it at once, with a preview for each.
**Reorder** (albums, and your own playlists) is its own screen: drag the rows by their
handles, then Save; nothing is written before. A playlist can also be renamed and deleted
(to the trash) in edit mode.

**Covers:** an album's **Covers** screen lists them with the main one first: make one main,
move it earlier or later, put it on all the album's tracks, move it to the trash (the server
keeps it there), or add an image from the phone's storage (the system picker, so the app
needs no storage permission). A track's covers are in its editor, where an image added is
a change like the others. **Notes** on tracks, albums and artists show at the bottom of their
pages; in edit mode they can be written, changed (with an optional reason) and taken off
(kept; "Bring back" undoes it).

Not on the phone yet: lyrics, and the desktop's credits-in-titles tool. Merging,
review and the rest of library work stay on the desktop.

## Sound and data

In **Settings**:

- **Quality on Wi-Fi / on mobile data.** The original, or a copy the server makes (320, 192,
  128, 96 kbps). The choice applies to what plays next, and changes by itself as the
  connection does. The default is the original on Wi-Fi and 128 kbps on mobile data.
- **Output level:** lowers the app's own volume below the phone's (for headphones that are
  loud even at the lowest step).
- **Lyrics timing:** moves the lyrics earlier or later. A Bluetooth headset delays the sound
  by an amount the phone only half knows, so there is one setting for when one is connected
  and one for everything else. If the lyrics come late, raise the number until a line lands
  as it is sung (a few hundred milliseconds is typical).

## Keeping music on the phone

**Download** on an album, a playlist, a track or Liked songs keeps those files on the phone;
they play from there, with or without the server, and their covers are kept too. Liked songs
follows your stars: a track you star later is fetched.

- Downloads fetch the original if your account may have it, else the server's usual copy, or
  the quality you choose in Settings, Downloads. Files are named by their SHA-256 and checked
  as they arrive.
- By default they only run on Wi-Fi; Settings can allow mobile data.
- **Stop** pauses them (what's here stays; **Resume** goes on). Nothing is deleted unless you
  remove it: tapping "Downloaded ✓" asks first, and Settings, Downloads, **Manage…** lists
  what's kept with its size, with a Remove for each.
- A track whose file isn't downloaded still needs the server to play.

The desktop app has the same Download chip and plays downloaded files (and a mirror's) from
disk.

## Building it

You need the Android SDK (platform 36, build tools, command-line tools) and NDK 28.2, Rust
with the `aarch64-linux-android` target, and `cargo-ndk`:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
export ANDROID_HOME=~/Android         # wherever the SDK is
cd clients/android
./gradlew assembleDebug                # clients/android/app/build/outputs/apk/debug/
```

Gradle builds the Rust core in release mode (a debug build of it is far too slow to judge
anything by) and generates the Kotlin that calls it. Add `-Pacetate.abis=arm64-v8a,x86_64`
for the emulator.

`./gradlew installDebug` puts the debug build on a connected phone **next to** a release
build: it has its own id (`io.github.allinen.acetate.debug`), login and downloads, so you can
try it against a development server without touching your real setup. `adb reverse
tcp:3000 tcp:3000` lets the phone reach a server on `127.0.0.1:3000` of the computer over USB.

## Signing a release

Android only installs an update if it's signed with the same key as the app it replaces, so
**the key is the app's identity: make one, back it up, and never lose or publish it.**

```sh
keytool -genkeypair -v -keystore ~/acetate-release.jks -alias acetate \
    -keyalg RSA -keysize 4096 -validity 36500
```

Put where it is in `clients/android/keystore.properties` (git ignores it, and `*.jks`):

```properties
storeFile=/home/you/acetate-release.jks
storePassword=…
keyAlias=acetate
keyPassword=…
```

(or set `ACETATE_KEYSTORE`, `ACETATE_KEYSTORE_PASSWORD`, `ACETATE_KEY_ALIAS` and
`ACETATE_KEY_PASSWORD` in the environment, for a build machine.) Then:

```sh
./gradlew assembleRelease -Pacetate.versionCode=2 -Pacetate.versionName=0.2.0
# clients/android/app/build/outputs/apk/release/app-release.apk
```

`versionCode` must go up with every release (Android refuses to install a lower one over a
higher); `versionName` is what people see. (A phone that has a build signed with the debug key can't be updated by one signed with your key: uninstall once, which loses its login and downloads.) Without a key the release is signed with the debug
key: it installs, but don't publish it, because the real key can't replace it later without
uninstalling.

The release is shrunk with R8. Before a release, try what shrinking does with
`./gradlew installStage`: the same build, signed with the debug key and installable next to
the others (`io.github.allinen.acetate.stage`). Log in, play something, open an album: what
breaks if a keep rule is missing is the Rust calls into Kotlin and the player, which
`app/proguard-rules.pro` covers.

To check a signed APK: `apksigner verify --print-certs app-release.apk` (in the SDK's
build-tools).
