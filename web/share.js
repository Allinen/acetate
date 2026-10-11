// /s/<token>: a share link. Someone logged in who can see the shared thing is sent on to the
// app's link for it (/open/…); everyone else gets the shared track or album to listen to.

function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

function duration(ms) {
  if (ms == null) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

const root = document.getElementById("share");
const player = document.getElementById("player");
const audio = document.getElementById("audio");
const info = document.getElementById("player-info");
const cover = document.getElementById("player-cover");

async function start(token) {
  // Plain `fetch`: a 401 here just means "not logged in".
  const response = await fetch(`/api/share/${token}`);
  if (!response.ok) {
    root.replaceChildren(h("h2", {}, "This link doesn't work"), h("p", { class: "muted" }, "It may have been revoked or expired."));
    return;
  }
  const share = await response.json();

  const meResponse = await fetch("/api/auth/me");
  const user = meResponse.ok ? await meResponse.json() : null;
  if (user?.username) {
    // Can they see it the normal way? Then it's a link like any other.
    const normal = await fetch(`/api/${share.kind}s/${share.id}`);
    if (normal.ok) {
      location.replace(`/open/${share.kind}/${share.id}`);
      return;
    }
  }

  const base = `/api/share/${token}`;
  const coverSrc = (id) => id && `${base}/covers/${id}`;
  const play = (t) => {
    audio.src = `${base}/tracks/${t.id}/stream`;
    audio.play().catch((err) => info.replaceChildren(h("span", { class: "error" }, `Can't play: ${err.message}`)));
    info.replaceChildren(h("div", {}, h("strong", {}, t.title)), h("div", { class: "muted" }, t.artist_display));
    const art = coverSrc(t.cover_id ?? share.cover_id);
    cover.hidden = !art;
    if (art) cover.src = art;
    player.hidden = false;
  };

  root.replaceChildren(
    h("div", { class: "share-head" },
      share.cover_id ? h("img", { src: coverSrc(share.cover_id), alt: "" }) : null,
      h("div", {},
        h("h2", {}, share.title),
        h("div", {}, share.artist),
        share.kind === "track" ? h("div", { class: "row" }, h("button", { onclick: () => play(share.tracks[0]) }, "▶ Play")) : null)),
    share.kind === "album"
      ? share.tracks.map((t) => h("div", { class: "share-track" },
          h("button", { class: "small", onclick: () => play(t) }, "▶"),
          h("div", { class: "grow" }, h("div", {}, t.title), h("div", { class: "muted" }, t.artist_display)),
          h("span", { class: "muted" }, duration(t.duration_ms))))
      : null);
}

const match = location.pathname.match(/^\/s\/([0-9a-f]+)$/);
if (match) start(match[1]);
else root.replaceChildren(h("p", { class: "muted" }, "This isn't a link Acetate knows."));
