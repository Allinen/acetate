# Sharing

There are two kinds of link.

- **`acetate://` link** (`Ctrl+L`): a link to the track, album or artist you're looking at,
  for someone with an account. It opens straight in their desktop app. The app registers the
  `acetate://` address with the system on Linux and Windows.
- **Share link** (`Ctrl+Shift+L`, or right-click → Share with `Shift` held): a web link to a
  track or album that *anyone* can open, no account needed. Creating one needs the `share`
  permission.

Right-click a track or album for the Share menu.

A share link plays in the browser. By default it plays a lower-quality copy (Opus, 128 kbps),
so the link can't be used to collect original files. If you have the `download_original`
permission, the web UI's Share form has **give the original file**: that link plays the
original. If you later lose that permission, your links go back to playing copies.
The web UI's Shares page says which kind each link is.
