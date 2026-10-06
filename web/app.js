// Acetate: a deliberately simple web UI for trying things out. No framework, no build step.
"use strict";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// h("div", {class: "x", onclick: fn}, "text", otherElement, [more, children])
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

async function api(path, options = {}) {
  const response = await fetch(path, options);
  const text = await response.text();
  let body;
  try { body = text ? JSON.parse(text) : null; } catch { body = text; }
  // Session gone (expired, logged out elsewhere, password changed): back to the login form.
  if (response.status === 401 && path !== "/api/auth/login") showLogin();
  if (!response.ok) {
    // `message` is written for people.
    const message = body?.message || `${response.status} ${response.statusText}`;
    throw new Error(message);
  }
  return body;
}

function postJson(path, data) {
  return api(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(data),
  });
}

function fmtDuration(ms) {
  if (ms === null || ms === undefined) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes) {
  if (bytes > 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024).toFixed(0)} KB`;
}

function fmtTime(iso) {
  return iso ? new Date(iso).toLocaleString() : "";
}

function kv(pairs) {
  return h("div", { class: "kv" },
    pairs.filter(([, v]) => v !== null && v !== undefined && v !== "")
      .map(([k, v]) => [h("div", {}, k), h("div", {}, v)]));
}

function errorBox(err) {
  return h("p", { class: "error" }, String(err.message || err));
}

// Source snapshot data as "key: value" lines.
function snapshotData(data) {
  return kv(Object.entries(data).map(([k, v]) => [k, typeof v === "string" && v.startsWith("http")
    ? h("a", { href: v, target: "_blank", rel: "noreferrer" }, v)
    : String(v)]));
}

// ---------------------------------------------------------------------------
// Source shapes and the source form (used for origins and adding sources)
// ---------------------------------------------------------------------------

let shapesPromise = null;
function getShapes() {
  shapesPromise ??= api("/api/source-shapes");
  return shapesPromise;
}

// A widget to fill in one source snapshot for `kind` ("track" / "album" / "artist").
// The platform must be chosen before the form can be submitted.
// Returns { element, read() } where read() returns the JSON to send.
// `initial` ({id, platform, data}): pre-filled from an existing source, to correct it
// (the result is a new snapshot of that source; the old one stays).
async function sourceWidget(kind, initial = null) {
  const shapes = (await getShapes()).filter((s) => s.kind === kind);
  const platform = h("select", { required: true },
    h("option", { value: "" }, "— choose —"),
    shapes.map((s) => h("option", { value: s.platform }, s.platform)));
  const fieldsBox = h("div", { class: "stack" });
  const capturedAt = h("input", { type: "datetime-local", step: "1" });
  let inputs = {};

  function render() {
    fieldsBox.replaceChildren();
    inputs = {};
    const shape = shapes.find((s) => s.platform === platform.value);
    if (!shape) return;
    for (const field of shape.fields) {
      // A list ("strs"): one value per line.
      const input = field.type === "strs"
        ? h("textarea", { rows: 2, placeholder: `${field.required ? "required" : "optional"}, one per line` })
        : h("input", {
          type: field.type === "int" ? "number" : "text",
          placeholder: field.required ? "required" : "optional",
          required: field.required,
        });
      inputs[field.name] = { input, field };
      const label = field.name + (field.name === shape.id_field ? " (id)" : "");
      fieldsBox.append(h("label", {}, label, input));
    }
    fieldsBox.append(h("label", {}, "captured at", capturedAt));
  }
  platform.addEventListener("change", render);
  if (initial) {
    platform.value = initial.platform;
    platform.disabled = true; // a correction stays on the same platform
  }
  render();
  if (initial) {
    for (const [name, value] of Object.entries(initial.data ?? {})) {
      if (!inputs[name]) continue;
      inputs[name].input.value = inputs[name].field.type === "strs" && Array.isArray(value) ? value.join("\n")
        : Array.isArray(value) ? value[0] ?? "" : value;
    }
  }

  const element = h("div", { class: "stack" }, h("label", {}, "platform", platform), fieldsBox);

  function read() {
    if (!platform.value) return null;
    const data = {};
    for (const [name, { input, field }] of Object.entries(inputs)) {
      if (input.value === "") continue;
      data[name] = field.type === "int" ? Number(input.value)
        : field.type === "strs" ? input.value.split("\n").map((v) => v.trim()).filter((v) => v)
        : input.value;
    }
    const source = { platform: platform.value, data };
    if (initial) source.source_id = initial.id;
    if (capturedAt.value) source.captured_at = new Date(capturedAt.value).toISOString();
    return source;
  }

  return { element, read };
}

// ---------------------------------------------------------------------------
// Track list
// ---------------------------------------------------------------------------

const listBody = document.querySelector("#track-list tbody");
const loadMore = document.getElementById("load-more");
const search = document.getElementById("search");
const PAGE = 100;
let lastId = null;
let selectedId = null;

async function loadTracks(reset) {
  if (reset) {
    listBody.replaceChildren();
    lastId = null;
  }
  // With a search: fuzzy results grouped into artists, albums and tracks (no paging).
  if (search.value.trim()) {
    loadMore.hidden = true;
    try {
      const r = await api(`/api/search?q=${encodeURIComponent(search.value.trim())}&limit=30`);
      const group = (label, rows) => rows.length ? [
        h("tr", {}, h("td", { colspan: 6, class: "muted" }, label)), rows] : [];
      // Built in a throwaway <tbody> (which flattens the groups), then moved over.
      listBody.replaceChildren(...Array.from(h("tbody", {},
        group("Artists", r.artists.map((a) => h("tr", { onclick: () => { location.hash = `#/artist/${a.id}`; } },
          h("td", { colspan: 6 }, a.name, a.disambiguation ? h("span", { class: "muted" }, ` (${a.disambiguation})`) : "",
            a.matched_alias ? h("span", { class: "muted" }, ` · alias "${a.matched_alias}"`) : "")))),
        group("Albums", r.albums.map((a) => h("tr", { onclick: () => { location.hash = `#/album/${a.id}`; } },
          h("td", { colspan: 6 }, a.title, h("span", { class: "muted" }, ` (${a.kind})`))))),
        group("Tracks", r.tracks.map((t) => h("tr", {
          "data-id": t.id, class: t.id === selectedId ? "selected" : null,
          onclick: () => { location.hash = `#/track/${t.id}`; },
          ondblclick: () => playTrackById(t.id),
        }, h("td", {}, t.title), h("td", { colspan: 5 }, t.artist_display))))).children));
      if (!listBody.childElementCount) {
        listBody.append(h("tr", {}, h("td", { colspan: 6, class: "muted" }, "No matches.")));
      }
    } catch (err) {
      listBody.replaceChildren(h("tr", {}, h("td", { colspan: 6 }, errorBox(err))));
    }
    return;
  }

  const params = new URLSearchParams({ limit: PAGE });
  if (lastId) params.set("before", lastId);

  try {
    const tracks = await api(`/api/tracks?${params}`);
    for (const t of tracks) {
      listBody.append(h("tr", {
        "data-id": t.id,
        class: t.id === selectedId ? "selected" : null,
        onclick: () => { location.hash = `#/track/${t.id}`; },
        ondblclick: () => playTrackById(t.id),
      },
        h("td", {}, t.title),
        h("td", {}, t.artist_display),
        h("td", {}, t.status),
        h("td", { class: "mono" }, t.release_date || ""),
        h("td", {}, t.codec || ""),
        h("td", {}, fmtDuration(t.duration_ms))));
    }
    if (tracks.length) lastId = tracks[tracks.length - 1].id;
    loadMore.hidden = tracks.length < PAGE;
    if (reset && !tracks.length) {
      listBody.append(h("tr", {}, h("td", { colspan: 6, class: "muted" }, "No tracks.")));
    }
  } catch (err) {
    listBody.append(h("tr", {}, h("td", { colspan: 6 }, errorBox(err))));
  }
}

let searchTimer;
search.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => loadTracks(true), 250);
});
loadMore.addEventListener("click", () => loadTracks(false));

function markSelected() {
  for (const row of listBody.querySelectorAll("tr")) {
    row.classList.toggle("selected", row.dataset.id === selectedId);
  }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

const audio = document.getElementById("audio");
const playerInfo = document.getElementById("player-info");
const playerCover = document.getElementById("player-cover");

// `trackId`: this is a listen of that track (its current file), so it counts as a play once
// it's been heard long enough. Previews (a previous file, the merge view) don't pass it.
function play(fileId, title, artist, coverId, note, trackId) {
  playSrc(`/api/files/${fileId}/stream`, title, artist, coverId && `/api/covers/${coverId}`, note);
  playing = trackId ? { trackId, counted: false } : null;
}

// What's playing, for counting plays: counted after half the track or 4 minutes, like
// Navidrome and Last.fm.
let playing = null;
audio.addEventListener("timeupdate", () => {
  if (!playing || playing.counted || !audio.duration) return;
  if (audio.currentTime >= Math.min(audio.duration / 2, 240)) {
    playing.counted = true;
    postJson(`/api/tracks/${playing.trackId}/played`, {}).catch(() => {});
  }
});

// Plays any stream URL (normal files, or through a share link).
function playSrc(src, title, artist, coverSrc, note) {
  playing = null;
  queue = null; // playing something else ends a queue (`playQueue` sets it again)
  audio.src = src;
  audio.play().catch((err) => {
    playerInfo.replaceChildren(errorBox(`Can't play: ${err.message}`));
  });
  playerInfo.replaceChildren(
    h("div", {}, h("strong", {}, title)),
    h("div", { class: "muted" }, artist, note ? ` · ${note}` : ""));
  playerCover.hidden = !coverSrc;
  if (coverSrc) playerCover.src = coverSrc;
}

// The user's own star, rating (0-10 = half stars) and plays of a track, album or artist.
function annotationBox(kind, id, a) {
  const status = h("span", {});
  const starLabel = (starred) => starred ? "★ starred" : "☆ star";
  const star = h("button", { type: "button", class: "small" }, starLabel(a.starred));
  star.addEventListener("click", async () => {
    try {
      a = await api(`/api/${kind}s/${id}/star`, { method: a.starred ? "DELETE" : "POST" });
      star.textContent = starLabel(a.starred);
    } catch (err) { status.replaceChildren(errorBox(err)); }
  });
  const stars = (n) => n ? "★".repeat(Math.floor(n / 2)) + (n % 2 ? "½" : "") : "not rated";
  const rating = h("select", {}, [...Array(11).keys()].map((n) => h("option", { value: n, selected: n === a.rating }, stars(n))));
  rating.addEventListener("change", async () => {
    try {
      a = await putJson(`/api/${kind}s/${id}/rating`, { rating: Number(rating.value) });
    } catch (err) { status.replaceChildren(errorBox(err)); }
  });
  return h("div", { class: "row" }, star, rating,
    h("span", { class: "muted" }, a.play_count ? `played ${a.play_count}×, last ${a.last_played_at ? fmtTime(a.last_played_at) : "?"}` : "never played"),
    status);
}

async function playTrackById(id) {
  const t = await api(`/api/tracks/${id}`);
  play(t.file.id, t.title, t.artist_display, t.covers[0]?.id, null, t.id);
}

// ---------------------------------------------------------------------------
// Detail views
// ---------------------------------------------------------------------------

const detail = document.getElementById("detail-pane");

// Like detail.replaceChildren, but accepts nested arrays and skips null/false,
// the same way h() does for its children.
function setDetail(...children) {
  detail.replaceChildren(h("div", {}, ...children));
}

function originView(origin) {
  if (!origin) return h("span", { class: "muted" }, "unknown");
  return h("div", {},
    h("span", { class: "tag" }, origin.platform),
    origin.external_id ? h("span", { class: "mono" }, origin.external_id, " ") : null,
    h("span", { class: "muted" }, "snapshot of ", fmtTime(origin.captured_at)),
    snapshotData(origin.data));
}

// `kind`/`ownerId`/`reload`: with `edit_sources`, each source gets "Correct…" (a new
// snapshot, pre-filled from the latest) and "Trash".
async function sourcesView(sources, kind, ownerId, reload) {
  if (!sources.length) return h("p", { class: "muted" }, "None.");
  const editable = can("edit_sources");
  return Promise.all(sources.map(async (s) => {
    const status = h("span", {});
    let correct = null;
    if (editable) {
      const widget = await sourceWidget(kind, { id: s.id, platform: s.platform, data: s.snapshots[0]?.data });
      const form = h("form", { class: "stack" }, widget.element,
        h("div", { class: "row" }, h("button", { type: "submit" }, "Save as new snapshot"), status));
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        save(status, () => postJson(`/api/${kind}s/${ownerId}/sources`, widget.read()), reload);
      });
      correct = h("details", {}, h("summary", {}, "Correct…"), form);
    }
    return h("div", { class: "box" },
      h("div", { class: "row" },
        h("span", { class: "tag" }, s.platform),
        s.external_id ? h("span", { class: "mono" }, s.external_id) : null,
        h("span", { class: "muted mono" }, `source ${s.id}`),
        editable ? h("button", {
          type: "button", class: "small",
          onclick: () => {
            if (!confirm("Move this source to the trash? It can be taken back out for 7 days.")) return;
            save(status, () => api(`/api/sources/${s.id}`, { method: "DELETE" }), reload);
          },
        }, "Trash") : null,
        editable ? null : status),
      s.snapshots.map((snap, i) => h("details", { open: i === 0 },
        h("summary", {}, fmtTime(snap.captured_at), i === 0 ? " (latest)" : ""),
        snapshotData(snap.data))),
      correct);
  }));
}

// "Add source" form for a track / album / artist.
async function addSourceForm(kind, ownerId, onDone) {
  const widget = await sourceWidget(kind);
  const status = h("span", {});
  const form = h("form", { class: "stack" }, widget.element,
    h("div", { class: "row" }, h("button", { type: "submit" }, "Add source"), status));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      const result = await postJson(`/api/${kind}s/${ownerId}/sources`, widget.read());
      status.replaceChildren(h("span", { class: "ok" },
        result.created ? "Added." : "Identical data already stored."));
      onDone();
    } catch (err) {
      status.replaceChildren(errorBox(err));
    }
  });
  return h("details", {}, h("summary", {}, "Add source…"), form);
}

async function replaceFileForm(trackId, onDone) {
  const file = h("input", { type: "file", accept: "audio/*", required: true });
  const reason = h("input", { type: "text", placeholder: "e.g. lossless upgrade" });
  const origin = await sourceWidget("track");
  const status = h("span", {});
  const form = h("form", { class: "stack" },
    h("label", {}, "file", file),
    h("label", {}, "reason", reason),
    h("div", { class: "muted" }, "Where the new file came from:"),
    origin.element,
    h("div", { class: "row" }, h("button", { type: "submit" }, "Replace file"), status));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = new FormData();
    body.append("reason", reason.value);
    body.append("origin", JSON.stringify(origin.read()));
    body.append("file", file.files[0]);
    status.replaceChildren(h("span", { class: "muted" }, "Uploading…"));
    try {
      await api(`/api/tracks/${trackId}/file`, { method: "POST", body });
      onDone();
    } catch (err) {
      status.replaceChildren(errorBox(err));
    }
  });
  return h("details", {}, h("summary", {}, "Replace file (e.g. upgrade quality)…"), form);
}

// ---------------------------------------------------------------------------
// Editing helpers: vocabularies, names, pickers, credits, history
// ---------------------------------------------------------------------------

let vocabPromise = null;
function getVocab() {
  vocabPromise ??= api("/api/vocab");
  return vocabPromise;
}

function options(values, selected) {
  return values.map((v) => h("option", { value: v, selected: v === selected }, v));
}

// Names of things revisions refer to by id, so diffs and editors can show names.
const names = { artist: new Map(), album: new Map(), track: new Map(), alias: new Map() };
// Artist details (name, listed aliases), fetched once.
const artistCache = new Map();
async function artistInfo(id, reload = false) {
  if (reload || !artistCache.has(id)) {
    const a = await api(`/api/artists/${id}`).catch(() => null);
    artistCache.set(id, a ? { name: a.name, aliases: a.aliases, revision_id: a.revision_id, disambiguation: a.disambiguation }
      : { name: id.slice(0, 8), aliases: [] });
    const info = artistCache.get(id);
    names.artist.set(id, info.name);
    for (const al of info.aliases) names.alias.set(al.id, al.name);
  }
  return artistCache.get(id);
}
async function artistName(id) {
  if (!names.artist.has(id)) await artistInfo(id);
  return names.artist.get(id);
}

// A search box listing matches; clicking one calls onPick(hit).
function picker(placeholder, search, label, onPick) {
  const input = h("input", { type: "search", placeholder });
  const results = h("div", { class: "picker-results" });
  let timer;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const q = input.value.trim();
      if (!q) { results.replaceChildren(); return; }
      const hits = await search(q).catch(() => []);
      results.replaceChildren(h("div", {},
        hits.length ? hits.map((hit) => h("button", {
          type: "button", class: "small",
          onclick: () => { onPick(hit); input.value = ""; results.replaceChildren(); },
        }, label(hit))) : h("span", { class: "muted" }, "no matches")));
    }, 200);
  });
  return h("div", { class: "picker" }, input, results);
}
const q = encodeURIComponent;
const findArtists = (text) => api(`/api/artists?q=${q(text)}&limit=8`);
const findAlbums = (text) => api(`/api/albums?q=${q(text)}&limit=8`);
const findTracks = (text) => api(`/api/search?q=${q(text)}&limit=8`).then((r) => r.tracks);
// A search leaving out one item: merging or relating something to itself is never meant.
const without = (search, id) => (text) => search(text).then((hits) => hits.filter((x) => x.id !== id));

// Editable list of credits. Returns { element, read() }.
async function creditsEditor(initial) {
  const vocab = await getVocab();
  const rows = [];
  const list = h("div", { class: "stack" });

  // Which name the credit shows: the artist's, or one of its aliases (or a new alias).
  function aliasSelect(artistId, info, selected) {
    const select = h("select", {},
      h("option", { value: "" }, `as "${info.name}"`),
      info.aliases.map((al) => h("option", { value: al.id, selected: al.id === selected }, `as "${al.name}"`)),
      h("option", { value: "new" }, "new alias…"));
    select.addEventListener("change", async () => {
      if (select.value !== "new") return;
      const aliasName = prompt(`New alias of ${info.name} (exact spelling, case matters):`);
      select.value = "";
      if (!aliasName) return;
      try {
        const current = await artistInfo(artistId, true);
        await putJson(`/api/artists/${artistId}`, {
          base_revision_id: current.revision_id, name: current.name, disambiguation: current.disambiguation,
          aliases: [...current.aliases, { name: aliasName }], reason: "alias added while editing credits",
        });
        const updated = await artistInfo(artistId, true);
        const created = updated.aliases.find((al) => al.name === aliasName);
        const replacement = aliasSelect(artistId, updated, created?.id);
        const row = rows.find((r) => r.alias === select);
        if (row) row.alias = replacement;
        select.replaceWith(replacement);
      } catch (err) {
        alert(err.message);
      }
    });
    return select;
  }
  function addRow(artistId, info, role, aliasId) {
    rows.push({
      artistId, name: info.name,
      role: h("select", {}, options(vocab.credit_roles, role)),
      alias: aliasSelect(artistId, info, aliasId),
    });
  }
  function move(i, by) {
    const j = i + by;
    if (j < 0 || j >= rows.length) return;
    [rows[i], rows[j]] = [rows[j], rows[i]];
    render();
  }
  function render() {
    list.replaceChildren(h("div", { class: "stack" }, rows.map((r, i) => h("div", { class: "row" },
      h("a", { href: `#/artist/${r.artistId}` }, r.name), r.role, r.alias,
      h("button", { type: "button", class: "small", onclick: () => move(i, -1) }, "▲"),
      h("button", { type: "button", class: "small", onclick: () => move(i, 1) }, "▼"),
      h("button", { type: "button", class: "small", onclick: () => { rows.splice(i, 1); render(); } }, "✕")))));
  }

  for (const c of initial) addRow(c.artist_id, await artistInfo(c.artist_id), c.role, c.alias_id);
  render();

  const add = picker("add artist…", findArtists,
    (a) => [a.name, a.disambiguation ? ` (${a.disambiguation})` : "", a.matched_alias ? ` · alias "${a.matched_alias}"` : ""],
    async (a) => {
      const info = await artistInfo(a.id);
      // Picked via an alias: credit it under that alias.
      const alias = a.matched_alias ? info.aliases.find((al) => al.name === a.matched_alias) : null;
      addRow(a.id, info, "primary", alias?.id);
      render();
    });
  const create = h("button", {
    type: "button", class: "small",
    onclick: async () => {
      const name = prompt("New artist's name:");
      if (!name) return;
      const disambiguation = prompt("Disambiguation (to tell apart artists with the same name), or leave empty:") ?? "";
      try {
        const { id } = await postJson("/api/artists", { name, disambiguation });
        addRow(id, await artistInfo(id), "primary", null);
        render();
      } catch (err) {
        alert(err.message);
      }
    },
  }, "new artist…");

  return {
    element: h("div", { class: "stack" }, list, h("div", { class: "row" }, add, create)),
    read: () => rows.map((r) => {
      const credit = { artist_id: r.artistId, role: r.role.value };
      if (r.alias.value && r.alias.value !== "new") credit.alias_id = r.alias.value;
      return credit;
    }),
  };
}

// Sends an edit; on success calls onSaved, on failure shows the error (with a reload
// button when someone else saved first).
async function save(status, send, onSaved) {
  status.replaceChildren(h("span", { class: "muted" }, "Saving…"));
  try {
    const written = await send();
    status.replaceChildren(h("span", { class: "ok" }, written?.changed === false ? "Nothing changed." : "Saved."));
    onSaved(written);
  } catch (err) {
    const stale = err.message.startsWith("changed by someone else");
    status.replaceChildren(errorBox(err), stale ? h("button", { type: "button", onclick: onSaved }, "Reload") : null);
  }
}

function putJson(path, data) {
  return api(path, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(data) });
}

// "%A is a remix of %B" with the two tracks filled in (the other one as a link).
function relationSentence(template, thisTitle, other) {
  return template.split(/(%A|%B)/).map((part) =>
    part === "%A" ? h("strong", {}, thisTitle)
      : part === "%B" ? h("a", { href: `#/track/${other.track_id}` }, other.title)
        : part);
}

// Shows a value from revision data, with names instead of ids where known.
function formatValue(key, value) {
  if (value === undefined || value === null) return h("span", { class: "muted" }, "—");
  if (key === "credits") {
    return value.map((c) => h("div", {}, names.artist.get(c.artist_id) ?? c.artist_id.slice(0, 8),
      ` (${c.role})`, c.alias_id ? ` as "${names.alias.get(c.alias_id) ?? c.alias_id.slice(0, 8)}"` : ""));
  }
  if (key === "albums") {
    return value.map((a) => h("div", {}, names.album.get(a.album_id) ?? a.album_id.slice(0, 8),
      a.disc_number ? ` disc ${a.disc_number}` : "", a.track_number ? ` #${a.track_number}` : ""));
  }
  if (key === "relations") {
    return value.map((r) => h("div", {}, r.kind, " ", names.track.get(r.track_id) ?? r.track_id.slice(0, 8)));
  }
  if (Array.isArray(value)) return value.join(", ");
  return String(value);
}

// The fields that differ between two revisions' data, as a table (old -> new).
function diffView(before, after) {
  const keys = [...new Set([...Object.keys(before ?? {}), ...Object.keys(after ?? {})])];
  const changed = keys.filter((k) => JSON.stringify(before?.[k]) !== JSON.stringify(after?.[k]));
  if (!before) return h("pre", {}, JSON.stringify(after, null, 2));
  if (!changed.length) return h("p", { class: "muted" }, "Same data as the revision before.");
  return h("table", { class: "diff" },
    h("tr", {}, h("th", {}, "field"), h("th", {}, "before"), h("th", {}, "after")),
    changed.map((k) => h("tr", {},
      h("td", { class: "mono" }, k),
      h("td", { class: "old" }, formatValue(k, before[k])),
      h("td", { class: "new" }, formatValue(k, after[k])))));
}

// History: revisions newest first, each diffed against the previous one in its own line
// (a variant, the track's external line, or the artist/album). `lineOf(r)` names the
// line, `restorePath(r)` is the restore endpoint.
async function historyView(revisions, lineOf, restorePath, onRestored) {
  // Make sure credited artists have names before rendering.
  const artistIds = new Set();
  for (const r of revisions) for (const c of r.data.credits ?? []) artistIds.add(c.artist_id);
  await Promise.all([...artistIds].map(artistName));

  const latestOfLine = new Map();
  return revisions.map((r, i) => {
    const line = lineOf(r);
    const previous = revisions.slice(i + 1).find((o) => lineOf(o) === line);
    const isLatest = !latestOfLine.has(line);
    latestOfLine.set(line, true);
    const status = h("span", {});
    return h("details", {},
      h("summary", {},
        fmtTime(r.created_at), " · ", r.author ?? h("span", { class: "muted" }, "system"), " · ",
        h("span", { class: "tag" }, line), " ", r.source,
        r.reason ? ` · ${r.reason}` : "", r.is_canon ? " · canon" : "", isLatest ? " · current" : ""),
      diffView(previous?.data, r.data),
      can("restore_revision") && !isLatest ? h("div", { class: "row" },
        h("button", {
          type: "button", class: "small",
          onclick: () => save(status, () => postJson(restorePath(r), {}), onRestored),
        }, "Restore this version"), status) : null);
  });
}

async function showTrack(id) {
  selectedId = id;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let t;
  try {
    t = await api(`/api/tracks/${id}`);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  // A merged track redirects to the one it was merged into.
  if (t.id !== id) {
    location.replace(`#/track/${t.id}`);
    return;
  }
  const reload = () => showTrack(id);
  const cover = t.covers[0];
  const f = t.file;
  for (const c of t.credits) {
    names.artist.set(c.artist_id, c.artist_name);
    if (c.alias_id) names.alias.set(c.alias_id, c.alias);
  }
  // Aliases the history credits, listed or not.
  for (const [id, alias] of Object.entries(t.alias_names || {})) names.alias.set(id, alias);
  for (const a of t.albums) names.album.set(a.album_id, a.title);
  for (const r of t.relations) names.track.set(r.track_id, r.title);
  const variantLabel = (variantId) => t.variants.find((v) => v.id === variantId)?.label ?? "variant";

  setDetail(
    h("div", { class: "detail-head" },
      cover ? h("img", { src: `/api/covers/${cover.id}`, alt: "" }) : null,
      h("div", {},
        h("h2", {}, t.title),
        h("div", {}, t.artist_display),
        annotationBox("track", t.id, t.annotation),
        addToPlaylistBox(t.id),
        h("div", { class: "row" },
          h("span", { class: "tag" }, t.status),
          t.is_restricted ? h("span", { class: "tag" }, "restricted") : null,
          t.genres.map((g) => h("span", { class: "tag" }, g))),
        h("div", { class: "row" },
          can("stream") ? h("button", { onclick: () => play(f.id, t.title, t.artist_display, cover?.id, null, t.id) }, "▶ Play") : null,
          can("download_original")
            ? h("a", { href: `/api/tracks/${id}/download`, title: "The original file, with the library's current tags written in" }, "Download")
            : null),
        can("share") ? shareForm("track", id) : null)),

    can("manage_access") ? [h("h3", {}, "Access"), await trackAccessBox(id, reload)] : null,

    t.covers.length || can("edit_metadata") ? [h("h3", {}, "Covers"), coverGallery("track", id, t.covers, reload)] : null,

    h("h3", {}, "Metadata"),
    kv([
      ["release date", t.release_date && h("span", { class: "mono" }, t.release_date)],
      ["release year", t.release_year],
      ["track id", h("span", { class: "mono" }, t.id)],
      ["created", fmtTime(t.created_at)],
    ]),

    h("h3", {}, "Credits"),
    t.credits.length ? t.credits.map((c) => h("div", {},
      h("a", { href: `#/artist/${c.artist_id}` }, c.artist_name), " ",
      h("span", { class: "tag" }, c.role),
      c.alias ? h("span", { class: "muted" }, ` credited as "${c.alias}"`) : null))
      : h("p", { class: "muted" }, "None."),

    h("h3", {}, "Relations"),
    t.relations.length
      ? t.relations.map((r) => h("div", {}, relationSentence(r.template, t.title, r)))
      : h("p", { class: "muted" }, "None."),

    can("edit_metadata") ? [h("h3", {}, "Edit"), await trackEditor(t, reload)] : null,
    t.merges?.length ? [h("h3", {}, "Merged into this track"), t.merges.map((m) => {
      const status = h("span", {});
      return h("div", { class: "box" },
        h("div", { class: "row" },
          `"${m.from_title}"`, h("span", { class: "muted" }, ` merged ${fmtTime(m.merged_at)}${m.merged_by ? ` by ${m.merged_by}` : ""}`),
          m.undone_at ? h("span", { class: "tag" }, `undone ${fmtTime(m.undone_at)}`)
            : !m.undo_blockers.length && can("edit_metadata") && can("replace_file") ? h("button", {
              type: "button", class: "small",
              onclick: () => {
                if (!confirm(`Undo this merge? "${m.from_title}" becomes its own track again, and this track goes back to how it was before.`)) return;
                save(status, () => postJson(`/api/track-merges/${m.id}/undo`, {}), () => { loadTracks(true); reload(); });
              },
            }, "Undo") : null,
          status),
        !m.undone_at && m.undo_blockers.length
          ? h("div", { class: "muted" }, "Can't be undone: ", m.undo_blockers.join("; "), ".") : null);
    })] : null,
    can("edit_metadata") && can("replace_file") ? h("details", {},
      h("summary", {}, "Merge a duplicate into this track…"),
      h("div", { class: "row" }, picker("the duplicate…", without(findTracks, t.id),
        (x) => [x.title, h("span", { class: "muted" }, " · ", x.artist_display)],
        (x) => { location.hash = `#/merge/${t.id}/${x.id}`; }))) : null,

    h("h3", {}, "Albums"),
    t.albums.length ? t.albums.map((a) => h("div", {},
      h("a", { href: `#/album/${a.album_id}` }, a.title), " ",
      h("span", { class: "tag" }, a.kind),
      h("span", { class: "muted" },
        a.track_number ? ` track ${a.track_number}` : "",
        a.disc_number ? `, disc ${a.disc_number}` : "")))
      : h("p", { class: "muted" }, "None."),

    h("h3", {}, "File"),
    h("div", { class: "box" },
      kv([
        ["original filename", f.original_filename ?? "— (none in the tags)"],
        ["uploaded as", f.uploaded_filename],
        ["format", [f.container, f.codec].filter(Boolean).join(" / ")],
        ["quality", [
          f.bitrate && `${f.bitrate} kbps`,
          f.sample_rate && `${f.sample_rate} Hz`,
          f.bit_depth && `${f.bit_depth} bit`,
          f.channels && `${f.channels} ch`,
        ].filter(Boolean).join(", ")],
        ["length", fmtDuration(f.duration_ms)],
        ["size", fmtSize(f.size_bytes)],
        ["sha256", h("span", { class: "mono" }, f.sha256)],
        ["uploaded", fmtTime(f.uploaded_at)],
        ["origin", originView(f.origin)],
        t.file_shared_with.length ? ["also played by", t.file_shared_with.map((o) =>
          h("div", {}, h("a", { href: `#/track/${o.id}` }, o.title), h("span", { class: "muted" }, ` ${o.artist_display}`)))] : null,
      ].filter(Boolean))),
    can("replace_file") ? await replaceFileForm(id, reload) : null,

    // Only there with the `view_history` permission.
    t.file_history?.length ? [
      h("h3", {}, "Previous files"),
      t.file_history.map((c) => h("div", { class: "box" },
        h("div", { class: "row" },
          h("strong", {}, c.old_original_filename ?? c.old_uploaded_filename),
          h("span", { class: "muted" }, [c.old_codec, c.old_bitrate && `${c.old_bitrate} kbps`, fmtSize(c.old_size_bytes)].filter(Boolean).join(" · ")),
          c.old_purged_at
            ? h("span", { class: "tag" }, `purged ${fmtTime(c.old_purged_at)}${c.old_purged_by ? ` by ${c.old_purged_by}` : ""}`)
            : c.old_trashed_at ? h("a", { href: "#/trash", class: "tag" }, "in the trash") : null,
          can("stream") && !c.old_purged_at ? h("button", {
            class: "small",
            onclick: () => play(c.old_file_id, t.title, t.artist_display, cover?.id, "previous file"),
          }, "▶") : null,
          can("replace_file") && !c.old_purged_at && c.old_file_id !== f.id ? h("button", {
            class: "small",
            onclick: async () => {
              if (!confirm("Play this file again? The current file becomes a previous file.")) return;
              try { await postJson(`/api/tracks/${id}/use-file`, { previous_file: c.old_file_id }); reload(); } catch (err) { alert(err.message); }
            },
          }, "Use again") : null,
          can("admin") && !c.old_trashed_at && c.old_file_id !== f.id ? h("button", {
            class: "small",
            onclick: async () => {
              if (!confirm(`Move "${c.old_original_filename ?? c.old_uploaded_filename}" to the trash? After 7 days an admin can purge it (its audio is then deleted for good).`)) return;
              try { await postJson("/api/files/trash", { file_ids: [c.old_file_id] }); reload(); } catch (err) { alert(err.message); }
            },
          }, "Trash") : null),
        kv([
          ["replaced", fmtTime(c.replaced_at)],
          ["reason", c.reason],
          ["origin", originView(c.old_origin)],
        ]))),
    ] : null,

    h("h3", {}, "Sources"),
    await sourcesView(t.sources, "track", id, reload),
    can("edit_sources") ? await addSourceForm("track", id, reload) : null,

    t.lyrics.length || can("edit_metadata") ? [h("h3", {}, "Lyrics"), await lyricsView(t, reload)] : null,

    t.revisions ? [
      h("h3", {}, "History"),
      await historyView(t.revisions,
        (r) => r.kind === "external" ? "status, albums, relations" : variantLabel(r.variant_id),
        (r) => `/api/revisions/${r.id}/restore`, reload),
    ] : null,

    h("h3", {}, "Embedded tags (verbatim)"),
    (f.embedded_tags.tags || []).map((tag) => h("details", {},
      h("summary", {}, tag.format, ` (${tag.items.length} items)`),
      h("table", {}, tag.items.map((item) => h("tr", {},
        h("td", { class: "mono" }, item.key, item.description ? `:${item.description}` : ""),
        h("td", { class: "mono" },
          typeof item.value === "string" ? item.value : JSON.stringify(item.value))))))),
  );
}

// The editing forms of a track: its variants' metadata, and its status/albums/relations.
async function trackEditor(t, reload) {
  const vocab = await getVocab();
  const primary = t.variants.find((v) => v.id === t.primary_variant_id) ?? t.variants[0];
  let current = primary;
  const variantBox = h("div", {});

  // ---- Variants and their metadata (internal revisions) ----
  async function renderVariant() {
    const d = current.data;
    const title = h("input", { value: d.title ?? "", required: true });
    const date = h("input", { value: d.release_date ?? "", placeholder: "verbatim, e.g. 2019-05-12T11:11:11Z" });
    const genres = h("textarea", { rows: 2, placeholder: "one per line" }, (d.genres ?? []).join("\n"));
    const comment = h("textarea", { rows: 2 }, d.comment ?? "");
    const credits = await creditsEditor(d.credits ?? []);
    const reason = h("input", { placeholder: "why (optional)" });
    const canon = h("input", { type: "checkbox" });
    const status = h("span", {});
    const form = h("form", { class: "stack" },
      h("label", {}, "title", title),
      h("label", {}, "release date", date),
      h("label", {}, "genres", genres),
      h("label", {}, "comment", comment),
      h("div", {}, "credits (the artist text is derived from these)"), credits.element,
      h("label", {}, "reason", reason),
      h("label", { class: "row" }, canon, "canon: this existed in the real world (e.g. an old official title)"),
      h("div", { class: "row" }, h("button", { type: "submit" }, `Save "${current.label}"`), status));
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const data = { title: title.value, credits: credits.read() };
      if (date.value) data.release_date = date.value;
      const genreList = genres.value.split("\n").filter((g) => g.trim());
      if (genreList.length) data.genres = genreList;
      if (comment.value) data.comment = comment.value;
      save(status, () => putJson(`/api/tracks/${t.id}/variants/${current.id}`, {
        base_revision_id: current.revision_id, data, reason: reason.value, is_canon: canon.checked,
      }), reload);
    });

    const variantStatus = h("span", {});
    const tabs = h("div", { class: "row" },
      t.variants.map((v) => h("button", {
        type: "button", class: v.id === current.id ? "small selected" : "small",
        onclick: () => { current = v; renderVariant(); },
      }, v.label, v.id === t.primary_variant_id ? " ★" : "")),
      h("button", {
        type: "button", class: "small",
        onclick: () => {
          const label = prompt(`New variant, starting as a copy of "${current.label}". Label:`);
          if (label) save(variantStatus, () => postJson(`/api/tracks/${t.id}/variants`, { label, copy_from: current.id }), reload);
        },
      }, "+ variant"),
      current.id !== t.primary_variant_id ? h("button", {
        type: "button", class: "small",
        onclick: () => save(variantStatus, () => putJson(`/api/tracks/${t.id}/primary-variant`, { variant_id: current.id }), reload),
      }, "make shown (★)") : null,
      h("button", {
        type: "button", class: "small",
        onclick: () => {
          const label = prompt("Rename variant:", current.label);
          if (label) save(variantStatus, () => api(`/api/tracks/${t.id}/variants/${current.id}`, {
            method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ label }),
          }), reload);
        },
      }, "rename"),
      variantStatus);
    variantBox.replaceChildren(h("div", { class: "box stack" },
      h("strong", {}, "Metadata"), h("div", { class: "muted" }, "Each variant has its own; ★ is the one shown."), tabs, form));
  }
  await renderVariant();

  // ---- Status, albums, relations (external revisions) ----
  const status = h("select", {}, options(vocab.statuses, t.status));
  const albumRows = t.albums.map((a) => ({
    album_id: a.album_id, title: a.title,
    disc: h("input", { type: "number", min: 1, value: a.disc_number ?? "", placeholder: "disc" }),
    track: h("input", { type: "number", min: 1, value: a.track_number ?? "", placeholder: "#" }),
  }));
  const albumList = h("div", {});
  function renderAlbums() {
    albumList.replaceChildren(h("div", { class: "stack" }, albumRows.map((a, i) => h("div", { class: "row" },
      h("a", { href: `#/album/${a.album_id}` }, a.title), a.disc, a.track,
      h("button", { type: "button", class: "small", onclick: () => { albumRows.splice(i, 1); renderAlbums(); } }, "✕")))));
  }
  renderAlbums();
  const addAlbum = picker("add to album…", findAlbums, (a) => [a.title, ` (${a.kind})`], (a) => {
    albumRows.push({
      album_id: a.id, title: a.title,
      disc: h("input", { type: "number", min: 1, placeholder: "disc" }),
      track: h("input", { type: "number", min: 1, placeholder: "#" }),
    });
    renderAlbums();
  });
  const newAlbum = h("button", {
    type: "button", class: "small",
    onclick: async () => {
      const title = prompt("New album's title:");
      if (!title) return;
      try {
        const { id } = await postJson("/api/albums", { title, credits: t.credits.filter((c) => c.role === "primary").map((c) => ({ artist_id: c.artist_id, role: "primary" })) });
        albumRows.push({ album_id: id, title, disc: h("input", { type: "number", min: 1, placeholder: "disc" }), track: h("input", { type: "number", min: 1, placeholder: "#" }) });
        renderAlbums();
      } catch (err) {
        alert(err.message);
      }
    },
  }, "new album…");

  const relations = t.relations.map((r) => ({ ...r }));
  const relationList = h("div", {});
  const canRelate = can("edit_relations");
  function renderRelations() {
    relationList.replaceChildren(h("div", { class: "stack" }, relations.map((r, i) => h("div", { class: "row" },
      h("span", {}, relationSentence(r.template, t.title, r)),
      canRelate ? h("button", { type: "button", class: "small", onclick: () => { relations.splice(i, 1); renderRelations(); } }, "✕") : null))));
  }
  renderRelations();
  const kind = h("select", {}, vocab.relation_kinds.map((k) =>
    h("option", { value: k.kind }, k.template.replace("%A", "this").replace("%B", "…"))));
  const addRelation = picker("related track…", without(findTracks, t.id), (x) => [x.title, h("span", { class: "muted" }, " · ", x.artist_display)], (x) => {
    const k = vocab.relation_kinds.find((k) => k.kind === kind.value);
    relations.push({ kind: k.kind, track_id: x.id, title: x.title, template: k.template });
    names.track.set(x.id, x.title);
    renderRelations();
  });

  const reason = h("input", { placeholder: "why (optional)" });
  const saveStatus = h("span", {});
  const factsForm = h("form", { class: "stack" },
    h("label", {}, "status", status),
    h("div", {}, "albums"), albumList, h("div", { class: "row" }, addAlbum, newAlbum),
    h("div", {}, "relations"), relationList,
    canRelate ? h("div", { class: "row" }, kind, addRelation) : h("div", { class: "muted" }, "Changing relations needs the edit_relations permission."),
    h("label", {}, "reason", reason),
    h("div", { class: "row" }, h("button", { type: "submit" }, "Save status, albums, relations"), saveStatus));
  factsForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const number = (input) => input.value ? Number(input.value) : undefined;
    save(saveStatus, () => putJson(`/api/tracks/${t.id}/facts`, {
      base_revision_id: t.external_revision_id,
      status: status.value,
      albums: albumRows.map((a) => ({ album_id: a.album_id, disc_number: number(a.disc), track_number: number(a.track) })),
      relations: relations.map((r) => ({ kind: r.kind, track_id: r.track_id })),
      reason: reason.value,
    }), reload);
  });

  return [variantBox, h("div", { class: "box stack" },
    h("strong", {}, "Status, albums, relations"), h("div", { class: "muted" }, "The same for every variant."), factsForm)];
}

async function showArtist(id) {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let a;
  try {
    a = await api(`/api/artists/${id}`);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  names.artist.set(a.id, a.name);
  artistCache.delete(a.id);
  const reload = () => showArtist(id);

  let editor = null;
  let merger = null;
  if (can("edit_metadata") && !a.merged_into) {
    const name = h("input", { value: a.name, required: true });
    const disambiguation = h("input", { value: a.disambiguation, placeholder: "to tell apart artists with the same name" });
    // Alias rows keep their id, so renaming one updates every credit using it.
    const aliasRows = a.aliases.map((al) => ({ id: al.id, input: h("input", { value: al.name }), uses: null }));
    const aliasList = h("div", {});
    // An alias still credited somewhere can't be removed: ✕ lists where, to change first.
    async function removeAlias(r) {
      if (r.id) {
        const uses = await api(`/api/artists/${id}/aliases/${r.id}/uses`);
        if (uses.tracks.length || uses.albums.length || uses.not_visible) {
          r.uses = uses;
          renderAliases();
          return;
        }
      }
      aliasRows.splice(aliasRows.indexOf(r), 1);
      renderAliases();
    }
    function aliasUses(u) {
      return h("div", { class: "muted" },
        "Still credited on these; change their credits first:",
        h("ul", {},
          u.tracks.map((t) => h("li", {}, "track ", h("a", { href: `#/track/${t.track_id}` }, t.title),
            t.variant ? ` (variant "${t.variant}")` : null)),
          u.albums.map((al) => h("li", {}, "album ", h("a", { href: `#/album/${al.album_id}` }, al.title))),
          u.not_visible ? h("li", {}, "tracks or albums you can't see") : null));
    }
    function renderAliases() {
      aliasList.replaceChildren(h("div", { class: "stack" }, aliasRows.map((r) => h("div", {},
        h("div", { class: "row" },
          r.input,
          h("button", { type: "button", class: "small", onclick: () => removeAlias(r).catch((err) => alert(err.message)) }, "✕")),
        r.uses ? aliasUses(r.uses) : null))));
    }
    renderAliases();
    const addAlias = h("button", {
      type: "button", class: "small",
      onclick: () => { aliasRows.push({ id: null, input: h("input", { placeholder: "exact spelling" }), uses: null }); renderAliases(); },
    }, "+ alias");
    const reason = h("input", { placeholder: "why (optional)" });
    const status = h("span", {});
    editor = h("form", { class: "stack box" },
      h("label", {}, "name", name),
      h("label", {}, "disambiguation", disambiguation),
      h("div", {}, "aliases (case variants count as separate aliases)"), aliasList, h("div", { class: "row" }, addAlias),
      h("label", {}, "reason", reason),
      h("div", { class: "row" }, h("button", { type: "submit" }, "Save artist"), status));
    editor.addEventListener("submit", (event) => {
      event.preventDefault();
      const aliases = aliasRows.filter((r) => r.input.value.trim())
        .map((r) => (r.id ? { id: r.id, name: r.input.value } : { name: r.input.value }));
      save(status, () => putJson(`/api/artists/${id}`, {
        base_revision_id: a.revision_id, name: name.value, disambiguation: disambiguation.value,
        aliases, reason: reason.value,
      }), reload);
    });

    // Merge this artist into another.
    let target = null;
    const targetLabel = h("span", { class: "muted" }, "no artist chosen");
    const keepName = h("input", { type: "checkbox", checked: true });
    const flagTracks = h("input", { type: "checkbox", checked: true });
    const mergeStatus = h("span", {});
    const choose = picker("merge into…", without(findArtists, a.id),
      (x) => [x.name, x.disambiguation ? ` (${x.disambiguation})` : ""],
      (x) => {
        target = x;
        targetLabel.replaceChildren(h("a", { href: `#/artist/${x.id}` }, x.name), x.disambiguation ? ` (${x.disambiguation})` : "");
      });
    merger = h("div", { class: "stack box" },
      h("div", { class: "muted" }, `For when "${a.name}" is really not a different artist. Its tracks, albums, aliases and sources move to the chosen one; this artist then points to it.`),
      h("div", { class: "row" }, choose, targetLabel),
      h("label", { class: "row" }, keepName, `keep "${a.name}" as an alias, so its tracks keep showing that name`),
      h("label", { class: "row" }, flagTracks, "flag the affected tracks for review (e.g. to re-add a featured artist)"),
      h("div", { class: "row" }, h("button", {
        type: "button",
        onclick: () => {
          if (!target) { mergeStatus.replaceChildren(errorBox("Choose an artist first.")); return; }
          if (!confirm(`Merge "${a.name}" into "${target.name}"? This rewrites the credits of its tracks (as new revisions).`)) return;
          save(mergeStatus, () => postJson(`/api/artists/${id}/merge`, {
            into: target.id, keep_name_as_alias: keepName.checked, flag_for_review: flagTracks.checked,
          }),
            () => { location.hash = `#/artist/${target.id}`; });
        },
      }, "Merge"), mergeStatus));
  }

  const trackRows = h("div", {});
  const more = h("button", { type: "button", class: "small" }, "More");
  function addTracks(tracks) {
    trackRows.append(...tracks.map((tr) => h("div", { class: "row" },
      h("a", { href: `#/track/${tr.id}` }, tr.title), h("span", { class: "muted" }, tr.artist_display))));
    more.hidden = tracks.length < 100;
  }
  addTracks(a.tracks);
  more.addEventListener("click", async () => {
    const last = trackRows.lastChild?.querySelector("a")?.getAttribute("href")?.split("/").pop();
    addTracks(await api(`/api/artists/${id}/tracks?before=${last}`));
  });

  setDetail(
    h("h2", {}, a.name),
    a.disambiguation ? h("div", { class: "muted" }, a.disambiguation) : null,
    a.merged_into ? null : annotationBox("artist", a.id, a.annotation),
    a.merged_into ? h("p", { class: "box" }, "This artist was merged into ",
      h("a", { href: `#/artist/${a.merged_into}` }, "another artist"), ".") : null,
    kv([
      ["aliases", a.aliases.map((al) => al.name).join(", ")],
      ["artist id", h("span", { class: "mono" }, a.id)],
    ]),
    editor ? [h("h3", {}, "Edit"), editor, h("h3", {}, "Merge"), merger] : null,
    h("h3", {}, "Tracks"),
    a.tracks.length ? [trackRows, more] : h("p", { class: "muted" }, "None."),
    h("h3", {}, "Sources"),
    await sourcesView(a.sources, "artist", id, reload),
    can("edit_sources") ? await addSourceForm("artist", id, reload) : null,
    a.revisions ? [h("h3", {}, "History"),
      await historyView(a.revisions, () => "artist", (r) => `/api/artist-revisions/${r.id}/restore`, reload)] : null);
}

async function showAlbum(id) {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let a;
  try {
    a = await api(`/api/albums/${id}`);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  // A merged album redirects to the one it was merged into.
  if (a.id !== id) {
    location.replace(`#/album/${a.id}`);
    return;
  }
  names.album.set(a.id, a.title);
  for (const c of a.credits) names.artist.set(c.artist_id, c.artist_name);
  const reload = () => showAlbum(id);

  let editor = null;
  let merger = null;
  if (can("edit_metadata")) {
    const vocab = await getVocab();
    const title = h("input", { value: a.title, required: true });
    const kind = h("select", {}, options(vocab.album_kinds, a.kind));
    const date = h("input", { value: a.release_date ?? "", placeholder: "verbatim" });
    const credits = await creditsEditor(a.credits);
    const reason = h("input", { placeholder: "why (optional)" });
    const status = h("span", {});
    editor = h("form", { class: "stack box" },
      h("label", {}, "title", title),
      h("label", {}, "kind", kind),
      h("label", {}, "release date", date),
      h("div", {}, "credits"), credits.element,
      h("label", {}, "reason", reason),
      h("div", { class: "row" }, h("button", { type: "submit" }, "Save album"), status));
    editor.addEventListener("submit", (event) => {
      event.preventDefault();
      save(status, () => putJson(`/api/albums/${id}`, {
        base_revision_id: a.revision_id, title: title.value, kind: kind.value,
        release_date: date.value || null, credits: credits.read(), reason: reason.value,
      }), reload);
    });

    // Merge this album into another.
    let target = null;
    const targetLabel = h("span", { class: "muted" }, "no album chosen");
    const mergeStatus = h("span", {});
    const choose = picker("merge into…", without(findAlbums, a.id), (x) => [x.title, ` (${x.kind})`], (x) => {
      target = x;
      targetLabel.replaceChildren(h("a", { href: `#/album/${x.id}` }, x.title), ` (${x.kind})`);
    });
    merger = h("div", { class: "stack box" },
      h("div", { class: "muted" }, `For when "${a.title}" is really the same album as another (e.g. split because the album artists or date were written differently). ` +
        "Its tracks (keeping their disc/track numbers), covers, sources and share links move to the chosen one, which keeps its own title, date and credits; this album then points to it. It can be undone from the other album's page."),
      h("div", { class: "row" }, choose, targetLabel),
      h("div", { class: "row" }, h("button", {
        type: "button",
        onclick: () => {
          if (!target) { mergeStatus.replaceChildren(errorBox("Choose an album first.")); return; }
          if (!confirm(`Merge "${a.title}" into "${target.title}"? Its ${a.tracks.length} tracks move there (as new revisions).`)) return;
          save(mergeStatus, () => postJson(`/api/albums/${id}/merge`, { into: target.id }),
            () => { location.hash = `#/album/${target.id}`; });
        },
      }, "Merge"), mergeStatus));
  }

  setDetail(
    h("h2", {}, a.title),
    // The album's cover: its own main one, else the first track's (`display_cover_id`).
    a.display_cover_id ? h("figure", { class: "album-cover" },
      h("img", { src: `/api/covers/${a.display_cover_id}`, alt: "" }),
      a.covers.length ? null : h("figcaption", { class: "muted" },
        "from the tracklist: ", a.tracks.find((t) => t.cover_id === a.display_cover_id)?.title ?? "a track")) : null,
    h("div", {}, a.credits.map((c) => (c.alias ?? c.artist_name) + (c.role === "primary" ? "" : ` (${c.role})`)).join(", ")),
    h("div", { class: "row" }, h("span", { class: "tag" }, a.kind)),
    annotationBox("album", a.id, a.annotation),
    can("share") ? shareForm("album", id) : null,
    can("manage_access") ? await albumAccessBox(id, reload) : null,
    a.covers.length || can("edit_metadata") ? [
      h("h3", {}, "Covers"),
      coverGallery("album", id, a.covers, reload),
      can("edit_metadata") && a.tracks.length ? copyCoverPicker(id, a.tracks, reload) : null,
    ] : null,
    kv([
      ["release date", a.release_date && h("span", { class: "mono" }, a.release_date)],
      ["album id", h("span", { class: "mono" }, a.id)],
    ]),
    h("h3", {}, "Tracks"),
    a.tracks.length ? a.tracks.map((tr) => h("div", { class: "row" },
      h("span", { class: "muted mono" }, [tr.disc_number && `${tr.disc_number}-`, tr.track_number ?? "·"].filter(Boolean).join("")),
      h("a", { href: `#/track/${tr.id}` }, tr.title),
      h("span", { class: "muted" }, tr.artist_display, " · ", fmtDuration(tr.duration_ms))))
      : h("p", { class: "muted" }, "None."),
    editor ? [h("h3", {}, "Edit"), editor, h("h3", {}, "Merge"), merger] : null,
    a.merges.length ? [h("h3", {}, "Merged into this album"), a.merges.map((m) => albumMergeRow(m))] : null,
    h("h3", {}, "Sources"),
    await sourcesView(a.sources, "album", id, reload),
    can("edit_sources") ? await addSourceForm("album", id, reload) : null,
    a.revisions ? [h("h3", {}, "History"),
      await historyView(a.revisions, () => "album", (r) => `/api/album-revisions/${r.id}/restore`, reload)] : null);
}

// One merge into an album. Undoing it first shows exactly what would go back and what
// stays (the server's preview), then does just that.
function albumMergeRow(m) {
  const from = m.from_title ? h("a", { href: `#/album/${m.from_album_id}` }, `"${m.from_title}"`) : "an album you can't see";
  const status = h("span", {});
  const preview = h("div", {});
  const list = (title, items) => items.length ? h("div", {}, h("strong", {}, title), h("ul", {}, items.map((x) => h("li", {}, x)))) : null;
  async function showPreview() {
    let r;
    try {
      r = await api(`/api/album-merges/${m.id}/undo`);
    } catch (err) {
      preview.replaceChildren(errorBox(err));
      return;
    }
    const hidden = r.hidden_tracks_back + r.hidden_tracks_staying
      ? `and tracks you can't see: ${r.hidden_tracks_back} go back, ${r.hidden_tracks_staying} stay` : null;
    preview.replaceChildren(h("div", { class: "stack box" },
      r.blockers.length ? errorBox(`Can't be undone: ${r.blockers.join("; ")}`) : null,
      list("Tracks going back", r.tracks.filter((t) => t.goes_back).map((t) =>
        [h("a", { href: `#/track/${t.id}` }, t.title), h("span", { class: "muted" }, `: ${t.detail}`)])),
      list("Tracks staying", r.tracks.filter((t) => !t.goes_back).map((t) =>
        [h("a", { href: `#/track/${t.id}` }, t.title), h("span", { class: "muted" }, `: ${t.detail}`)])),
      hidden ? h("div", { class: "muted" }, hidden) : null,
      list("Also goes back", r.goes_back),
      list("Stays as it is", r.stays),
      r.blockers.length ? null : h("div", { class: "row" }, h("button", {
        type: "button",
        onclick: () => {
          if (!confirm("Undo this merge, exactly as listed?")) return;
          save(status, () => postJson(`/api/album-merges/${m.id}/undo`, {}), () => { location.hash = `#/album/${r.from_album_id}`; });
        },
      }, "Undo merge"), status)));
  }
  return h("div", { class: "stack" },
    h("div", { class: "row" }, from,
      h("span", { class: "muted" }, ` merged ${fmtTime(m.merged_at)}${m.merged_by ? ` by ${m.merged_by}` : ""}${m.undone_at ? `, undone ${fmtTime(m.undone_at)}` : ""}`),
      !m.undone_at && can("edit_metadata") ? h("button", { type: "button", class: "small", onclick: showPreview }, "Undo…") : null),
    preview);
}

// #/review: things flagged for a person to look at. "Fix later" flags (a quick import's)
// and what tracks are missing (flags the server raises itself: "no cover"...) are shown
// on their own, so they don't bury the rest.
let reviewFilter = "exclude=fix_later&automatic=false";
async function showReview() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let flags, problems, vocab;
  try {
    [flags, problems, vocab] = await Promise.all([
      can("edit_metadata") ? api(`/api/review?${reviewFilter}`) : [],
      can("upload") ? api("/api/ingest-problems") : [],
      getVocab(),
    ]);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  setDetail(
    h("h2", {}, "Review"),
    can("upload") ? [h("h3", {}, `Files that didn't import (${problems.length})`), problemList(problems)] : null,
    can("edit_metadata") ? h("div", { class: "row" }, h("h3", {}, "Flags"),
      h("select", { onchange: (e) => { reviewFilter = e.target.value; showReview(); } },
        [["exclude=fix_later&automatic=false", "to check"], ["kind=fix_later", "fix later"], ["automatic=true", "missing something"], ["", "all"]].map(([value, label]) =>
          h("option", { value, selected: value === reviewFilter }, label)))) : null,
    !can("edit_metadata") ? null : flags.length ? flags.map((f) => {
      const resolution = h("input", { placeholder: "what you did (optional)" });
      const status = h("span", {});
      const kind = (vocab.review_kinds ?? []).find((k) => k.kind === f.kind);
      return h("div", { class: "box stack" },
        h("div", { class: "row" },
          h("span", { class: "tag" }, kind?.name ?? f.kind),
          h("a", { href: `#/track/${f.track_id}` }, f.track_title),
          h("span", { class: "muted" }, f.track_artist, " · ", fmtTime(f.created_at))),
        f.kind === "possible_duplicate"
          ? h("div", {}, f.related.reason ? `The ${f.related.reason}: ` : "Similar titles by the same artist: ",
              f.related_tracks.map((x) => [h("a", { href: `#/track/${x.id}` }, x.title), " ",
                can("edit_metadata") && can("replace_file")
                  ? h("a", { href: `#/merge/${f.track_id}/${x.id}`, class: "tag" }, "compare & merge")
                  : null, " "]))
          : f.kind === "fix_later"
          ? h("div", {}, "Imported to fix later: ", h("em", {}, f.related.note ?? ""))
          : kind?.automatic
          ? h("div", {}, `${kind.name}. Settled by itself once it's fixed; resolving it says it's fine as it is (asked again only if it's fixed and goes missing later).`)
          : f.kind === "merged_artist"
          ? h("div", {}, `Was credited to "${f.related.from_name}", which was merged into `,
              h("a", { href: `#/artist/${f.related.into}` }, f.related_artists.find((x) => x.id === f.related.into)?.name ?? "another artist"),
              ". Check the credits are still right (e.g. a featured artist that was part of the old name).")
          : f.kind === "ambiguous_artist"
          ? h("div", {}, `"${f.related.name}" matches several artists; the oldest was credited: `,
              f.related_artists.map((x) => [h("a", { href: `#/artist/${x.id}` }, x.name,
                x.disambiguation ? ` (${x.disambiguation})` : "", x.id === f.related.chosen ? " ✓" : ""), " "]))
          // A kind this page doesn't know yet (a newer server): its name.
          : h("div", {}, kind?.name ?? f.kind),
        h("div", { class: "row" }, resolution,
          h("button", {
            type: "button", class: "small",
            onclick: () => save(status, () => postJson(`/api/review/${f.id}/resolve`, { resolution: resolution.value }), showReview),
          }, "Resolve"), status));
    }) : h("p", { class: "muted" }, "Nothing to review."));
}

// Files that were refused (missing tags...) or failed, with their audio kept. They resolve
// by themselves when the same file imports fine later.
function problemList(problems) {
  if (!problems.length) return h("p", { class: "muted" }, "None.");
  return problems.map((p) => {
    const tags = p.embedded_tags?.tags ?? [];
    return h("div", { class: "box stack" },
      h("div", { class: "row" },
        h("span", { class: "tag" }, p.kind),
        h("strong", {}, p.uploaded_filename),
        h("span", { class: "muted" }, fmtSize(p.size_bytes), " · ", fmtTime(p.last_attempt_at),
          p.attempts > 1 ? ` · tried ${p.attempts} times` : "", p.created_by ? ` · ${p.created_by}` : "")),
      p.path ? h("div", { class: "muted mono" }, p.path) : null,
      h("div", {}, p.reason),
      h("audio", { controls: true, preload: "none", src: `/api/ingest-problems/${p.id}/audio` }),
      tags.length ? h("details", {}, h("summary", {}, "tags in the file"),
        h("pre", { class: "mono" }, tags.map((t) => [`[${t.format}]`,
          ...t.items.map((i) => `  ${i.key}${i.description ? ` (${i.description})` : ""}: ${typeof i.value === "string" ? i.value : JSON.stringify(i.value)}`)].join("\n")).join("\n"))) : null,
      importForm(p),
      can("replace_file") ? addToTrackForm(p) : null);
  });
}

// Import the kept file again as it is (the file isn't changed). What its tags lack (a
// title, an artist) is given in the desktop app's editor, which imports it with them.
function importForm(p) {
  const status = h("span", {});
  return h("div", { class: "row" },
    h("button", {
      type: "button", class: "small",
      onclick: () => save(status, () => postJson(`/api/ingest-problems/${p.id}/import`, {}),
        (answer) => { location.hash = `#/track/${answer.track_id}`; }),
    }, "Import as it is"),
    h("span", { class: "muted" }, "(what its tags lack: in the desktop app)"), status);
}

// For a file that belongs to a track that's already there: make it that track's file (the
// old one becomes a previous file), or keep it as a previous file.
function addToTrackForm(p) {
  let target = null;
  const chosen = h("span", { class: "muted" }, "no track chosen");
  const addAs = h("select", {},
    h("option", { value: "previous_file" }, "as a previous file"),
    h("option", { value: "current_file" }, "as its file (the current one becomes previous)"));
  const reason = h("input", { placeholder: "why (optional)" });
  const status = h("span", {});
  const pick = picker("add to track…", findTracks, (x) => [x.title, h("span", { class: "muted" }, " · ", x.artist_display)], (x) => {
    target = x;
    chosen.replaceChildren(h("a", { href: `#/track/${x.id}` }, x.title), ` · ${x.artist_display}`);
  });
  return h("details", {}, h("summary", {}, "add to an existing track"),
    h("div", { class: "stack" }, pick, h("div", { class: "row" }, chosen, addAs, reason,
      h("button", {
        type: "button", class: "small",
        onclick: () => {
          if (!target) { status.textContent = "choose a track first"; return; }
          save(status, () => postJson(`/api/ingest-problems/${p.id}/add-to-track`,
            { track_id: target.id, add_as: addAs.value, reason: reason.value }),
            () => { location.hash = `#/track/${target.id}`; });
        },
      }, "Add"), status)));
}

// ---------------------------------------------------------------------------
// Upload panel
// ---------------------------------------------------------------------------

const uploadPanel = document.getElementById("upload-panel");
document.getElementById("upload-toggle").addEventListener("click", async () => {
  uploadPanel.hidden = !uploadPanel.hidden;
  if (!uploadPanel.hidden && !uploadPanel.childElementCount) await buildUploadPanel();
});

async function buildUploadPanel() {
  const files = h("input", { type: "file", accept: "audio/*", multiple: true, required: true });
  const status = h("select", {}, options((await getVocab()).statuses, "released"));
  // Created restricted right away, so an unreleased track is never briefly visible to all.
  const access = can("manage_access") ? await accessPicker({ restricted: false, groups: [] }) : null;
  const origin = await sourceWidget("track");
  const sourcesBox = h("div", {});
  const extraSources = [];
  const results = h("div", {});

  const addSourceButton = h("button", {
    type: "button",
    onclick: async () => {
      const widget = await sourceWidget("track");
      const box = h("div", { class: "box" }, widget.element,
        h("button", {
          type: "button", class: "small",
          onclick: () => { box.remove(); extraSources.splice(extraSources.indexOf(widget), 1); },
        }, "Remove"));
      extraSources.push(widget);
      sourcesBox.append(box);
    },
  }, "+ Add another source");

  const form = h("form", { class: "stack" },
    h("label", {}, "files", files),
    h("label", {}, "status", status),
    access ? access.element : null,
    h("div", { class: "muted" }, "Where the file(s) came from (applies to each file):"),
    origin.element,
    h("div", { class: "muted" }, "Other places the song can be found (single file only):"),
    sourcesBox,
    h("div", { class: "row" }, addSourceButton),
    h("div", { class: "row" }, h("button", { type: "submit" }, "Upload")),
    results);

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    results.replaceChildren();
    const chosen = [...files.files];
    const sources = extraSources.map((w) => w.read());
    if (sources.length && chosen.length > 1) {
      results.append(errorBox("Other sources describe one song; upload a single file with them."));
      return;
    }
    const o = origin.read();

    // One request per file, so progress shows file by file.
    for (const file of chosen) {
      const line = h("div", {}, file.name, ": ", h("span", { class: "muted" }, "uploading…"));
      results.append(line);
      const body = new FormData();
      body.append("status", status.value);
      if (access?.read().restricted) body.append("restrict_to", JSON.stringify(access.read().groups));
      body.append("origin", JSON.stringify(o));
      if (sources.length) body.append("sources", JSON.stringify(sources));
      body.append("file", file);
      try {
        const [r] = await api("/api/upload", { method: "POST", body });
        line.lastChild.replaceWith(r.track_id
          ? h("a", { href: `#/track/${r.track_id}`, class: r.status === "created" ? "ok" : "" }, r.status)
          : r.status === "duplicate"
            ? h("span", { class: "muted" }, "duplicate: already in the library")
            : h("span", { class: "error" }, `${r.status}: ${r.error}`));
      } catch (err) {
        line.lastChild.replaceWith(errorBox(err));
      }
    }
    loadTracks(true);
  });

  uploadPanel.append(h("strong", {}, "Upload"), form);
}

// ---------------------------------------------------------------------------
// Routing: #/track/<id>, #/artist/<id>, #/album/<id>
// ---------------------------------------------------------------------------

function route() {
  const [, kind, id, other] = location.hash.split("/");
  if (kind === "track" && id) showTrack(id);
  else if (kind === "merge" && id && other) showMerge(id, other);
  else if (kind === "artist" && id) showArtist(id);
  else if (kind === "album" && id) showAlbum(id);
  else if (kind === "shares") showShares();
  else if (kind === "playlists") showPlaylists();
  else if (kind === "playlist" && id) showPlaylist(id);
  else if (kind === "account") showAccount();
  else if (kind === "review") showReview();
  else if (kind === "groups") showGroups();
  else if (kind === "trash") showTrash();
  else setDetail(h("p", { class: "muted" }, "Select a track. Double-click to play."));
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

// Who we are: {id, username, permissions}, or just {permissions} for a guest (not logged in).
let me = null;

// Whether the user has a permission. Only decides what's shown; the server checks for real.
function can(permission) {
  return !!me && (me.permissions.includes("admin") || me.permissions.includes(permission));
}

function showLogin() {
  audio.pause();
  document.body.classList.add("logged-out");
  document.getElementById("login").hidden = false;
}

document.getElementById("login-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const error = document.getElementById("login-error");
  error.textContent = "";
  try {
    // Sets the session cookie, which the browser then sends with every request.
    await postJson("/api/auth/login", {
      username: form.username.value,
      password: form.password.value,
    });
    location.reload(); // start fresh as the new user
  } catch (err) {
    error.textContent = err.message;
  }
});

const accountButton = document.getElementById("account");
accountButton.addEventListener("click", async () => {
  await api("/api/auth/logout", { method: "POST" }).catch(() => {});
  location.reload(); // back to the login form
});

async function start() {
  try {
    me = await api("/api/auth/me");
  } catch {
    return; // stale session: `api` has shown the login form
  }
  // Guests (not logged in) can't do anything but log in.
  if (!me.username) {
    showLogin();
    return;
  }
  document.getElementById("whoami").textContent = me.username;
  accountButton.textContent = "Log out";
  document.getElementById("upload-toggle").hidden = !can("upload");
  document.getElementById("shares-link").hidden = !can("share");
  document.getElementById("playlists-link").hidden = !can("browse");
  document.getElementById("review-link").hidden = !can("edit_metadata") && !can("upload");
  document.getElementById("groups-link").hidden = !can("manage_users");
  document.getElementById("trash-link").hidden = !(can("edit_metadata") || can("edit_sources"));
  window.addEventListener("hashchange", route);
  loadTracks(true);
  route();
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

// A plain queue for playing a playlist: when a track ends, the next one plays. Playing
// anything else ends it.
let queue = null; // { items: [{fileId, title, artist, coverId, trackId}], index }
function playQueue(items, index) {
  const it = items[index];
  if (!it) return;
  play(it.fileId, it.title, it.artist, it.coverId, `${index + 1} of ${items.length}`, it.trackId);
  queue = { items, index };
}
audio.addEventListener("ended", () => {
  if (queue && queue.index + 1 < queue.items.length) playQueue(queue.items, queue.index + 1);
});

// #/playlists: yours and public ones, a form for a new one, and yours in the trash.
async function showPlaylists() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  try {
    const [lists, trashed] = await Promise.all([api("/api/playlists"), api("/api/playlists?trash=true")]);
    const name = h("input", { placeholder: "name", required: true });
    const pub = h("input", { type: "checkbox" });
    const status = h("span", {});
    const form = h("form", { class: "row box" }, name, h("label", {}, pub, " public"),
      h("button", { type: "submit" }, "New playlist"), status);
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      save(status, () => postJson("/api/playlists", { name: name.value, public: pub.checked }),
        (r) => { location.hash = `#/playlist/${r.id}`; });
    });
    const row = (p) => h("div", { class: "box row" },
      h("a", { href: `#/playlist/${p.id}` }, p.name),
      p.public ? h("span", { class: "tag" }, "public") : h("span", { class: "tag" }, "private"),
      h("span", { class: "muted" }, `${p.entries} tracks · ${fmtDuration(p.duration_ms)}`),
      p.mine ? null : h("span", { class: "muted" }, `by ${p.owner}`));
    setDetail(
      h("h2", {}, "Playlists"),
      form,
      lists.length ? lists.map(row) : h("p", { class: "muted" }, "No playlists yet."),
      trashed.length ? h("details", {}, h("summary", {}, `In the trash (${trashed.length})`),
        trashed.map((p) => h("div", { class: "box row" }, p.name,
          h("span", { class: "muted" }, `trashed ${fmtTime(p.trashed_at)}`),
          h("button", { class: "small", onclick: async () => {
            await postJson(`/api/playlists/${p.id}/restore`, {});
            showPlaylists();
          } }, "Restore")))) : null);
  } catch (err) {
    setDetail(errorBox(err));
  }
}

// #/playlist/<id>: its tracks (play all or from one), and for the owner: rename, public,
// reorder, remove, trash, and the log with undo.
async function showPlaylist(id) {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let p;
  try {
    p = await api(`/api/playlists/${id}`);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  const reload = () => showPlaylist(id);
  const status = h("span", {});
  const items = p.entries.map((e) => ({
    fileId: e.file_id, title: e.title, artist: e.artist_display, coverId: e.cover_id, trackId: e.track_id,
  }));
  const ids = p.entries.map((e) => e.id);
  const move = (i, by) => {
    const order = [...ids];
    const [moved] = order.splice(i, 1);
    order.splice(i + by, 0, moved);
    save(status, () => putJson(`/api/playlists/${id}/order`, { entries: order }), reload);
  };
  const remove = (entry) => save(status, () => api(`/api/playlists/${id}/entries`, {
    method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify({ entries: [entry] }),
  }), reload);
  const entryRow = (e, i) => h("tr", {},
    h("td", { class: "muted" }, String(i + 1)),
    h("td", {}, h("button", { class: "small", title: "Play from here", onclick: () => playQueue(items, i) }, "▶")),
    h("td", {}, h("a", { href: `#/track/${e.track_id}` }, e.title)),
    h("td", { class: "muted" }, e.artist_display),
    h("td", { class: "muted" }, fmtDuration(e.duration_ms)),
    p.mine ? h("td", { class: "row" },
      h("button", { class: "small", disabled: i === 0, onclick: () => move(i, -1) }, "↑"),
      h("button", { class: "small", disabled: i === ids.length - 1, onclick: () => move(i, 1) }, "↓"),
      h("button", { class: "small", onclick: () => remove(e.id) }, "✕")) : null);

  let owner = null;
  if (p.mine) {
    const name = h("input", { value: p.name, required: true });
    const description = h("input", { value: p.description, placeholder: "description" });
    const pub = h("input", { type: "checkbox", checked: p.public });
    const form = h("form", { class: "row" }, name, description, h("label", {}, pub, " public"),
      h("button", { type: "submit", class: "small" }, "Save"),
      h("button", { type: "button", class: "small", onclick: () => {
        if (confirm(`Put "${p.name}" in the trash? It can be restored from the Playlists page.`)) {
          save(status, () => api(`/api/playlists/${id}`, { method: "DELETE" }), () => { location.hash = "#/playlists"; });
        }
      } }, "Trash"));
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      save(status, () => api(`/api/playlists/${id}`, {
        method: "PATCH", headers: { "content-type": "application/json" },
        body: JSON.stringify({ name: name.value, description: description.value, public: pub.checked }),
      }), reload);
    });
    const log = await api(`/api/playlists/${id}/log`).catch(() => []);
    const describe = (l) => {
      const n = l.detail.entries?.length ?? l.detail.entries;
      return {
        add: `added ${n} track${n === 1 ? "" : "s"}`,
        remove: `removed ${n} track${n === 1 ? "" : "s"}`,
        reorder: "changed the order",
        undo: `undid a change: ${l.detail.summary ?? ""}`,
        import: `imported (${l.detail.from ?? ""}, ${l.detail.entries ?? 0} tracks)`,
      }[l.action] ?? l.action;
    };
    owner = h("div", {},
      form,
      h("details", {}, h("summary", {}, `Changes (${log.length})`),
        log.map((l) => h("div", { class: "row" },
          h("span", { class: "muted" }, fmtTime(l.at)),
          h("span", {}, describe(l), l.by ? ` · ${l.by}` : ""),
          l.undone_at ? h("span", { class: "muted" }, "undone")
            : ["add", "remove", "reorder"].includes(l.action)
              ? h("button", { class: "small", onclick: () => save(status,
                  () => postJson(`/api/playlists/${id}/log/${l.id}/undo`, {}), reload) }, "Undo")
              : null))));
  }

  setDetail(
    h("h2", {}, p.name),
    h("div", { class: "muted" }, p.mine ? "yours" : `by ${p.owner}`, " · ", p.public ? "public" : "private",
      ` · ${p.entries.length} tracks · ${fmtDuration(p.duration_ms)}`),
    p.description ? h("p", {}, p.description) : null,
    h("div", { class: "row" },
      h("button", { disabled: !items.length, onclick: () => playQueue(items, 0) }, "▶ Play all"), status),
    owner,
    p.entries.length ? h("table", {}, h("tbody", {}, p.entries.map(entryRow)))
      : h("p", { class: "muted" }, "Empty. Add tracks from a track's page."));
}

// On a track's page: add it to one of your playlists (again, if it's in it already).
function addToPlaylistBox(trackId) {
  const box = h("div", { class: "row" });
  api("/api/playlists").then((lists) => {
    const mine = lists.filter((p) => p.mine);
    if (!mine.length) return;
    const select = h("select", {}, mine.map((p) => h("option", { value: p.id }, p.name)));
    const status = h("span", {});
    box.replaceChildren(select, h("button", { class: "small", onclick: async () => {
      try {
        const current = await api(`/api/playlists/${select.value}`);
        const already = current.entries.some((e) => e.track_id === trackId);
        await postJson(`/api/playlists/${select.value}/entries`, { tracks: [trackId] });
        status.replaceChildren(h("span", { class: "ok" }, already ? "added (it was in it already)" : "added"));
      } catch (err) { status.replaceChildren(errorBox(err)); }
    } }, "Add to playlist"), status);
  }).catch(() => {});
  return box;
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Lyrics and covers
// ---------------------------------------------------------------------------

// A track's lyrics entries: each with an editor and its history. A removed entry is kept
// (listed apart): restoring a revision from before brings it back.
async function lyricsView(t, reload) {
  const removeButton = (l) => {
    const status = h("span", {});
    return [h("button", {
      type: "button", class: "small",
      onclick: () => {
        if (!confirm("Take these lyrics off the track? They're kept, to bring back.")) return;
        save(status, () => api(`/api/lyrics/${l.id}?base_revision_id=${l.revision_id}`, { method: "DELETE" }), reload);
      },
    }, "Remove"), status];
  };
  const removed = await Promise.all((t.removed_lyrics || []).map(async (l) => h("div", { class: "box" },
    h("pre", {}, l.body),
    l.revisions ? await historyView(l.revisions, () => "lyrics", (r) => `/api/lyrics-revisions/${r.id}/restore`, reload) : null)));
  const entries = await Promise.all(t.lyrics.map(async (l) => h("div", { class: "box" },
    h("div", { class: "row" },
      h("span", { class: "tag" }, l.is_official ? "official" : "unofficial"),
      h("span", { class: "tag" }, l.is_synced ? "synced" : "plain"),
      l.language ? h("span", { class: "tag" }, l.language) : null,
      h("span", { class: "muted" }, l.source)),
    h("pre", {}, l.body),
    can("edit_metadata") ? h("details", {}, h("summary", {}, "Edit…"), lyricsForm(t.id, l, reload)) : null,
    can("edit_metadata") ? h("div", { class: "row" }, removeButton(l)) : null,
    l.revisions ? h("details", {}, h("summary", {}, `History (${l.revisions.length})`),
      await historyView(l.revisions, () => "lyrics", (r) => `/api/lyrics-revisions/${r.id}/restore`, reload)) : null)));
  return [
    entries.length ? entries : h("p", { class: "muted" }, "None."),
    can("edit_metadata") ? h("details", {}, h("summary", {}, "Add lyrics…"), lyricsForm(t.id, null, reload)) : null,
    removed.length ? h("details", {}, h("summary", {}, `Removed (${removed.length}): restore a revision from before to bring one back`), removed) : null,
  ];
}

// Form for a new lyrics entry (`l` = null) or a new revision of one.
function lyricsForm(trackId, l, reload) {
  const body = h("textarea", { rows: 10, required: true });
  body.value = l?.body ?? "";
  const official = h("input", { type: "checkbox", checked: l?.is_official ?? false });
  const language = h("input", { value: l?.language ?? "", placeholder: "e.g. en (optional)" });
  const reason = h("input", { placeholder: "why (optional)" });
  const status = h("span", {});
  const form = h("form", { class: "stack" },
    h("label", {}, "lyrics", body),
    h("div", { class: "muted" }, "Synced (LRC, lines like [01:23.45]) is detected from the text."),
    h("label", { class: "row" }, official, "official (from the artist/label)"),
    h("label", {}, "language", language),
    h("label", {}, "reason", reason),
    h("div", { class: "row" }, h("button", { type: "submit" }, l ? "Save lyrics" : "Add lyrics"), status));
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const data = { body: body.value, is_official: official.checked, language: language.value || null, reason: reason.value };
    save(status, () => l
      ? putJson(`/api/lyrics/${l.id}`, { base_revision_id: l.revision_id, ...data })
      : postJson(`/api/tracks/${trackId}/lyrics`, data), reload);
  });
  return form;
}

// Covers of a track or album (`kind` = "track" / "album"): the first is the main one.
// With `edit_metadata`: reorder, make main, trash, upload.
function coverGallery(kind, ownerId, covers, reload) {
  const editable = can("edit_metadata");
  const status = h("span", {});
  const ids = covers.map((c) => c.id);
  const setOrder = (order) => save(status, () => putJson(`/api/${kind}s/${ownerId}/covers/order`, { cover_ids: order }), reload);
  const swap = (i, j) => { const order = [...ids]; [order[i], order[j]] = [order[j], order[i]]; setOrder(order); };
  const upload = h("input", { type: "file", accept: "image/jpeg,image/png,image/webp,image/gif" });
  upload.addEventListener("change", () => {
    const body = new FormData();
    body.append("image", upload.files[0]);
    save(status, () => api(`/api/${kind}s/${ownerId}/covers`, { method: "POST", body }), reload);
  });
  const button = (label, onclick) => h("button", { type: "button", class: "small", onclick }, label);
  return h("div", {},
    covers.length ? h("div", { class: "covers" }, covers.map((c, i) => h("figure", {},
      h("img", { src: `/api/covers/${c.id}`, alt: "" }),
      h("figcaption", { class: "muted" },
        i === 0 ? "main · " : "", c.width ? `${c.width}×${c.height} · ` : "", c.source),
      editable ? h("div", { class: "row" },
        i > 0 ? button("←", () => swap(i, i - 1)) : null,
        i < covers.length - 1 ? button("→", () => swap(i, i + 1)) : null,
        i > 0 ? button("make main", () => setOrder([c.id, ...ids.filter((x) => x !== c.id)])) : null,
        button("Trash", () => {
          if (!confirm("Move this cover to the trash? It can be taken back out for 7 days.")) return;
          save(status, () => api(`/api/covers/${c.id}`, { method: "DELETE" }), reload);
        })) : null))) : h("p", { class: "muted" }, "None."),
    editable ? h("div", { class: "row" }, h("label", { class: "row" }, "upload an image:", upload), status) : null);
}

// Album page: pick one of its tracks' covers to use for the album too.
function copyCoverPicker(albumId, tracks, reload) {
  const box = h("div", { class: "covers" }, h("span", { class: "muted" }, "Loading…"));
  const status = h("span", {});
  const details = h("details", {}, h("summary", {}, "Copy a cover from one of its tracks…"), box, status);
  details.addEventListener("toggle", async () => {
    if (!details.open || box.dataset.loaded) return;
    box.dataset.loaded = "1";
    const found = (await Promise.all(tracks.map((tr) => api(`/api/tracks/${tr.id}`))))
      .flatMap((t) => t.covers.map((c) => ({ c, title: t.title })));
    box.replaceChildren(...(found.length ? found.map(({ c, title }) => h("figure", {},
      h("img", {
        src: `/api/covers/${c.id}`, alt: "", title: "Use for the album", style: "cursor: pointer",
        onclick: () => save(status, () => postJson(`/api/albums/${albumId}/covers/copy`, { cover_id: c.id }), reload),
      }),
      h("figcaption", { class: "muted" }, title))) : [h("span", { class: "muted" }, "Its tracks have no covers.")]));
  });
  return details;
}

// ---------------------------------------------------------------------------
// Access: restricted tracks and groups
// ---------------------------------------------------------------------------

// The groups, loaded once (the Groups page reloads them after changes).
let groupsPromise = null;
function getGroups(reload = false) {
  if (reload || !groupsPromise) groupsPromise = api("/api/groups");
  return groupsPromise;
}

// "Restricted" checkbox + which groups may see it. `initial`: {restricted, groups: [ids]}.
// read() -> {restricted, groups}; hidesMe() -> true if saving it would hide it from me.
async function accessPicker(initial) {
  const groups = await getGroups();
  const restricted = h("input", { type: "checkbox", checked: initial.restricted });
  const boxes = groups.map((g) => ({
    group: g,
    input: h("input", { type: "checkbox", checked: initial.groups.includes(g.id) }),
  }));
  const groupRow = h("div", { class: "row" },
    h("span", { class: "muted" }, "visible to:"),
    boxes.map((b) => h("label", { class: "row" }, b.input, b.group.name)));
  const sync = () => { groupRow.hidden = !restricted.checked; };
  restricted.addEventListener("change", sync);
  sync();
  const read = () => ({
    restricted: restricted.checked,
    groups: restricted.checked ? boxes.filter((b) => b.input.checked).map((b) => b.group.id) : [],
  });
  return {
    element: h("div", {},
      h("label", { class: "row" }, restricted, "restricted: only admins and the groups below can see it"),
      groupRow),
    read,
    hidesMe: () => {
      const value = read();
      if (!value.restricted || can("admin")) return false;
      return !boxes.some((b) => b.input.checked && b.group.members.includes(me.username));
    },
  };
}

// Access box of a track page (manage_access).
async function trackAccessBox(id, reload) {
  const access = await api(`/api/tracks/${id}/access`);
  const picker = await accessPicker({ restricted: access.restricted, groups: access.groups.map((g) => g.id) });
  const reason = h("input", { placeholder: "why (optional)" });
  const status = h("span", {});
  const form = h("form", { class: "stack box" }, picker.element,
    h("label", {}, "reason", reason),
    h("div", { class: "row" }, h("button", { type: "submit" }, "Save access"), status),
    access.log.length ? h("details", {}, h("summary", {}, `Changes (${access.log.length})`),
      access.log.map((l) => h("div", { class: "muted" },
        fmtTime(l.changed_at), " · ", l.changed_by ?? "system", " · ",
        l.restricted ? `restricted to ${l.groups.map((g) => g.name).join(", ") || "admins only"}` : "unrestricted",
        l.reason ? ` · ${l.reason}` : ""))) : null);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (picker.hidesMe() && !confirm("You're in none of these groups: you won't see this track anymore. Save anyway?")) return;
    save(status, () => putJson(`/api/tracks/${id}/access`, { ...picker.read(), reason: reason.value }),
      (r) => { if (r.visible) reload(); else { location.hash = ""; loadTracks(true); } });
  });
  return form;
}

// Album page: set access on all its tracks (manage_access).
async function albumAccessBox(id, reload) {
  const picker = await accessPicker({ restricted: false, groups: [] });
  const status = h("span", {});
  const form = h("form", { class: "stack box" },
    h("div", { class: "muted" }, "Sets who can see each track of this album (that you can see)."),
    picker.element,
    h("div", { class: "row" }, h("button", { type: "submit" }, "Apply to all tracks"), status));
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (picker.hidesMe() && !confirm("You're in none of these groups: you won't see these tracks anymore. Apply anyway?")) return;
    save(status, () => putJson(`/api/albums/${id}/access`, picker.read()), () => { loadTracks(true); reload(); });
  });
  return h("details", {}, h("summary", {}, "Restrict all tracks…"), form);
}

// #/merge/<kept>/<duplicate>: both tracks side by side. Left is kept, right is merged
// into it. Metadata, file and covers: pick a side per row (left by default). Albums,
// relations, sources, lyrics and variants: combined.
async function showMerge(leftId, rightId) {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let left, right;
  try {
    [left, right] = await Promise.all([api(`/api/tracks/${leftId}`), api(`/api/tracks/${rightId}`)]);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  if (left.id === right.id) {
    setDetail(errorBox("These are the same track (one was merged into the other already)."));
    return;
  }
  const primary = (t) => t.variants.find((v) => v.id === t.primary_variant_id);
  const [lv, rv] = [primary(left), primary(right)];
  for (const t of [left, right]) for (const c of t.credits) names.artist.set(c.artist_id, c.artist_name);

  const picks = {};
  const choice = (field, leftView, rightView, same) => {
    const name = `pick-${field}`;
    const radio = (side) => h("input", {
      type: "radio", name, checked: side === "left",
      onchange: () => { picks[field] = side; },
    });
    picks[field] = "left";
    return h("tr", {},
      h("td", { class: "mono" }, field),
      h("td", {}, h("label", { class: "row" }, radio("left"), leftView)),
      h("td", {}, h("label", { class: "row" }, radio("right"), rightView)),
      h("td", { class: "muted" }, same ? "same" : ""));
  };
  const text = (v) => v === null || v === undefined || v === "" || (Array.isArray(v) && !v.length)
    ? h("span", { class: "muted" }, "—") : Array.isArray(v) ? v.join(", ") : String(v);
  const creditsText = (credits) => (credits ?? []).map((c) =>
    `${names.artist.get(c.artist_id) ?? c.artist_id.slice(0, 8)} (${c.role})`).join(", ") || "—";
  const fileView = (t) => {
    const f = t.file;
    return h("div", {},
      h("div", {}, h("strong", {}, f.original_filename ?? f.uploaded_filename)),
      h("div", { class: "muted" }, [f.container, f.codec].filter(Boolean).join(" / "),
        f.bitrate ? ` · ${f.bitrate} kbps` : "", f.sample_rate ? ` · ${f.sample_rate} Hz` : "",
        f.bit_depth ? ` · ${f.bit_depth} bit` : "", f.channels ? ` · ${f.channels} ch` : ""),
      h("div", { class: "muted" }, fmtDuration(f.duration_ms), " · ", fmtSize(f.size_bytes)),
      h("button", { type: "button", class: "small", onclick: () => play(f.id, t.title, t.artist_display, t.covers[0]?.id) }, "▶"));
  };
  const coversView = (t) => t.covers.length
    ? h("div", { class: "covers" }, t.covers.map((c) => h("figure", {}, h("img", { src: `/api/covers/${c.id}`, alt: "" }))))
    : h("span", { class: "muted" }, "none");
  const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
  const ld = lv.data, rd = rv.data;

  const table = h("table", { class: "trash" },
    h("tr", {}, h("th", {}, ""),
      h("th", {}, "left: kept ", h("a", { href: `#/track/${left.id}` }, "(open)")),
      h("th", {}, "right: merged into it ", h("a", { href: `#/track/${right.id}` }, "(open)")), h("th", {}, "")),
    choice("title", text(ld.title), text(rd.title), same(ld.title, rd.title)),
    choice("credits", creditsText(ld.credits), creditsText(rd.credits), same(ld.credits, rd.credits)),
    choice("release_date", text(ld.release_date), text(rd.release_date), same(ld.release_date, rd.release_date)),
    choice("genres", text(ld.genres), text(rd.genres), same(ld.genres, rd.genres)),
    choice("comment", text(ld.comment), text(rd.comment), same(ld.comment, rd.comment)),
    choice("status", text(left.status), text(right.status), left.status === right.status),
    choice("file", fileView(left), fileView(right), left.file.sha256 === right.file.sha256),
    choice("covers", coversView(left), coversView(right), false));

  const union = (a, b, key) => [...new Map([...a, ...b].map((x) => [key(x), x])).values()];
  const albums = union(left.albums, right.albums, (a) => a.album_id);
  const relations = union(left.relations, right.relations, (r) => `${r.kind}/${r.track_id}`)
    .filter((r) => r.track_id !== left.id && r.track_id !== right.id);
  const status = h("span", {});
  const reason = h("input", { placeholder: "why (optional)" });
  const shareStatus = h("span", {});

  setDetail(
    h("h2", {}, "Merge tracks"),
    h("p", { class: "muted" },
      "The right track is merged into the left one and then redirects to it. Pick a side per row. " +
      "The file not picked becomes a previous file (then it can be trashed and purged). All covers are " +
      "kept: the picked side's come first (the first is the main cover), then the other side's."),
    left.is_restricted !== right.is_restricted ? h("p", { class: "box error" },
      `Their access differs: left is ${left.is_restricted ? "restricted" : "visible to everyone"}, right is ` +
      `${right.is_restricted ? "restricted" : "visible to everyone"}. The merged track keeps left's.`) : null,
    table,
    h("h3", {}, "Combined automatically"),
    kv([
      ["albums", albums.map((a) => a.title).join(", ") || "—"],
      ["relations", relations.map((r) => `${r.kind} ${r.title}`).join(", ") || "—"],
      ["sources", [...left.sources, ...right.sources].map((s) => s.platform + (s.external_id ? ` ${s.external_id}` : "")).join(", ") || "—"],
      ["lyrics", `${left.lyrics.length} + ${right.lyrics.length} (identical ones counted once)`],
      ["right's variants", right.variants.map((v) => v.label).join(", ") + " (become variants of the left track)"],
    ]),
    h("div", { class: "row" },
      h("button", { type: "button", onclick: () => { location.hash = `#/merge/${right.id}/${left.id}`; } }, "⇄ swap sides"),
      h("label", { class: "row" }, "reason", reason)),
    h("div", { class: "row" },
      h("button", {
        type: "button",
        onclick: () => {
          if (!confirm(`Merge "${right.title}" into "${left.title}"? The right track will only redirect to the left one afterwards.`)) return;
          save(status, () => postJson(`/api/tracks/${left.id}/merge`, {
            from: right.id,
            base: { into_revision_id: lv.revision_id, from_revision_id: rv.revision_id },
            pick: picks, reason: reason.value,
          }), () => { loadTracks(true); location.hash = `#/track/${left.id}`; });
        },
      }, "Merge"), status),
    can("replace_file") && left.file.sha256 !== right.file.sha256 ? [
      h("h3", {}, "Or: keep both tracks, share one file"),
      h("p", { class: "muted" },
        "Both stay separate tracks with their own metadata; only the audio is stored once. The other " +
        "file becomes a previous file of its track (it can be played again from there, or trashed). " +
        "The use case for this is having two separate entries for the same song. Otherwise always merge!"),
      h("div", { class: "row" },
        [[left, right], [right, left]].map(([keeps, gives]) => h("button", {
          type: "button",
          onclick: () => {
            if (!confirm(`"${gives.title}" plays the file of "${keeps.title}" from now on?`)) return;
            save(shareStatus, () => postJson(`/api/tracks/${gives.id}/use-file`, { file_of: keeps.id, reason: reason.value }),
              () => { location.hash = `#/track/${gives.id}`; });
          },
        }, `Both use ${keeps === left ? "left" : "right"}'s file`)),
        shareStatus),
    ] : null);
}

// #/trash: covers, sources and old audio files in the trash; restoring, and purging
// (admins, after the grace period). Plus replaced audio files, to trash in bulk.
async function showTrash() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let items;
  try {
    items = await api("/api/trash");
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  const now = new Date();
  const rows = items.map((it) => ({ it, check: h("input", { type: "checkbox" }) }));
  const selected = () => rows.filter((r) => r.check.checked).map((r) => r.it);
  const status = h("span", {});
  const ownerLink = (it) => it.owner_id && it.owner
    ? h("a", { href: `#/${it.owner_kind}/${it.owner_id}` }, it.owner)
    : h("span", { class: "muted" }, it.owner ?? "—");

  const restore = () => {
    const chosen = selected();
    if (!chosen.length) return;
    save(status, () => postJson("/api/trash/restore", { items: chosen.map((it) => ({ kind: it.kind, id: it.id })) }), showTrash);
  };
  const purge = () => {
    const chosen = selected();
    if (!chosen.length) return;
    const early = chosen.filter((it) => new Date(it.purgeable_at) > now);
    if (early.length) {
      status.replaceChildren(errorBox(`${early.length} of these can't be purged yet (see "purgeable").`));
      return;
    }
    const one = chosen.length === 1 ? chosen[0] : null;
    const key = one ? (one.sha256 ?? one.id) : null;
    const question = one
      ? `Purge "${one.what}" for good${one.kind === "source" ? "" : " (its data is deleted)"}?\n\nType the first 6 characters of ${one.sha256 ? "its sha256" : "its id"} to confirm:\n${key}`
      : `Purge these ${chosen.length} items for good? Their data is deleted.\n\nType ${chosen.length} to confirm:`;
    const confirmText = prompt(question);
    if (confirmText === null) return;
    save(status, () => postJson("/api/trash/purge", {
      items: chosen.map((it) => ({ kind: it.kind, id: it.id })), confirm: confirmText,
    }).then((r) => {
      alert(`Purged ${r.purged} item(s); ${r.blobs_deleted} blob(s) deleted, ${fmtSize(r.bytes_freed)} freed.`);
      return r;
    }), showTrash);
  };

  const total = items.reduce((sum, it) => sum + (it.size_bytes ?? 0), 0);
  setDetail(
    h("h2", {}, "Trash"),
    h("p", { class: "muted" },
      "Things here are hidden, and can be taken back out. After 7 days an admin can purge them: " +
      "covers and sources are then deleted, audio files keep their record but lose their audio. " +
      "Lyrics and metadata are never trashed; they have history instead."),
    items.length ? [
      h("table", { class: "trash" },
        h("tr", {}, h("th", {}, ""), h("th", {}, "what"), h("th", {}, "of"), h("th", {}, "size"),
          h("th", {}, "trashed"), h("th", {}, "purgeable")),
        rows.map(({ it, check }) => h("tr", {},
          h("td", {}, check),
          h("td", {}, h("span", { class: "tag" }, it.kind), " ",
            it.kind === "cover" ? h("img", { src: `/api/covers/${it.id}`, alt: "", style: "height: 40px; vertical-align: middle" }) : it.what),
          h("td", {}, ownerLink(it)),
          h("td", {}, it.size_bytes != null ? fmtSize(it.size_bytes) : "—"),
          h("td", {}, fmtTime(it.trashed_at), it.trashed_by ? ` by ${it.trashed_by}` : ""),
          h("td", { class: new Date(it.purgeable_at) <= now ? "ok" : "muted" },
            new Date(it.purgeable_at) <= now ? "now" : fmtTime(it.purgeable_at))))),
      h("p", { class: "muted" }, `${items.length} items, ${fmtSize(total)}.`),
      h("div", { class: "row" },
        h("button", { type: "button", onclick: () => rows.forEach((r) => { r.check.checked = true; }) }, "Select all"),
        h("button", { type: "button", onclick: restore }, "Take out of the trash"),
        can("admin") ? h("button", { type: "button", onclick: purge }, "Purge…") : null,
        status),
    ] : h("p", { class: "muted" }, "The trash is empty."),
    can("admin") ? [h("h3", {}, "Replaced audio files"), await replacedFilesBox()] : null);
}

// Admins: previous files of tracks (replaced by an upgrade), to move to the trash in bulk.
async function replacedFilesBox() {
  const rows = [];
  const table = h("table", { class: "trash" },
    h("tr", {}, h("th", {}, ""), h("th", {}, "file"), h("th", {}, "track"), h("th", {}, "size"), h("th", {}, "replaced")));
  const status = h("span", {});
  const more = h("button", { type: "button" }, "Load more");
  const load = async () => {
    const before = rows.length ? `&before=${rows[rows.length - 1].f.id}` : "";
    const page = await api(`/api/files/replaced?limit=200${before}`);
    for (const f of page) {
      const check = h("input", { type: "checkbox" });
      rows.push({ f, check });
      table.append(h("tr", {}, h("td", {}, check),
        h("td", {}, f.original_filename ?? f.uploaded_filename, h("span", { class: "muted" }, " ", [f.codec, f.bitrate && `${f.bitrate} kbps`].filter(Boolean).join(" "))),
        h("td", {}, h("a", { href: `#/track/${f.track_id}` }, f.track_title)),
        h("td", {}, fmtSize(f.size_bytes)),
        h("td", {}, fmtTime(f.replaced_at))));
    }
    more.hidden = page.length < 200;
  };
  more.addEventListener("click", load);
  await load();
  if (!rows.length) return h("p", { class: "muted" }, "None: every stored audio file is some track's current file.");
  const trash = () => {
    const chosen = rows.filter((r) => r.check.checked).map((r) => r.f);
    if (!chosen.length) return;
    const size = chosen.reduce((sum, f) => sum + f.size_bytes, 0);
    if (!confirm(`Move ${chosen.length} file(s) (${fmtSize(size)}) to the trash? After 7 days an admin can purge them.`)) return;
    save(status, () => postJson("/api/files/trash", { file_ids: chosen.map((f) => f.id) }), showTrash);
  };
  return h("div", {},
    h("p", { class: "muted" }, "Files no track plays anymore. Their record stays even after purging."),
    table, more,
    h("div", { class: "row" },
      h("button", { type: "button", onclick: () => rows.forEach((r) => { r.check.checked = true; }) }, "Select all"),
      h("button", { type: "button", onclick: trash }, "Move to the trash"), status));
}

// #/groups: groups and their permissions (manage_users).
async function showGroups() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let groups, vocab;
  try {
    [groups, vocab] = await Promise.all([getGroups(true), getVocab()]);
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  const editor = (g) => {
    const name = h("input", { value: g?.name ?? "", required: true });
    const description = h("input", { value: g?.description ?? "" });
    const perms = vocab.permissions.map((p) => ({
      name: p.permission,
      input: h("input", { type: "checkbox", checked: g?.permissions.includes(p.permission) ?? false }),
      description: p.description,
    }));
    const status = h("span", {});
    const form = h("form", { class: "stack box" },
      h("label", {}, "name", name),
      h("label", {}, "description", description),
      h("div", {}, perms.map((p) => h("label", { class: "row" }, p.input, h("strong", {}, p.name),
        h("span", { class: "muted" }, p.description)))),
      h("div", { class: "row" },
        h("button", { type: "submit" }, g ? "Save group" : "Create group"),
        g ? h("button", {
          type: "button",
          onclick: () => {
            if (!confirm(`Delete the group "${g.name}"?`)) return;
            save(status, () => api(`/api/groups/${g.id}`, { method: "DELETE" }), showGroups);
          },
        }, "Delete") : null,
        status));
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const body = {
        name: name.value, description: description.value,
        permissions: perms.filter((p) => p.input.checked).map((p) => p.name),
      };
      save(status, () => g ? putJson(`/api/groups/${g.id}`, body) : postJson("/api/groups", body), showGroups);
    });
    return form;
  };

  setDetail(
    h("h2", {}, "Groups"),
    h("p", { class: "muted" },
      "Groups give their members permissions, and restricted tracks are opened to groups. " +
      "People who aren't logged in have no permissions at all; that isn't a group and can't be changed."),
    groups.map((g) => h("div", { class: "box" },
      h("div", { class: "row" }, h("strong", {}, g.name), h("span", { class: "muted" }, g.description)),
      h("div", {}, g.permissions.map((p) => h("span", { class: "tag" }, p))),
      h("div", { class: "muted" }, "members: ", g.members.join(", ") || "none"),
      g.name === "admin" ? null : h("details", {}, h("summary", {}, "Edit…"), editor(g)))),
    h("h3", {}, "New group"),
    editor(null));
}

// ---------------------------------------------------------------------------
// Account: change password, sessions
// ---------------------------------------------------------------------------

async function showAccount() {
  selectedId = null;
  markSelected();

  const current = h("input", { type: "password", autocomplete: "current-password", required: true });
  const next = h("input", { type: "password", autocomplete: "new-password", required: true, minlength: 8 });
  const repeat = h("input", { type: "password", autocomplete: "new-password", required: true });
  const status = h("div", {});
  const passwordForm = h("form", { class: "stack" },
    h("label", {}, "current password", current),
    h("label", {}, "new password (at least 8 characters)", next),
    h("label", {}, "new password again", repeat),
    h("div", { class: "row" }, h("button", { type: "submit" }, "Change password")),
    status);
  passwordForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (next.value !== repeat.value) {
      status.replaceChildren(errorBox("The new passwords don't match."));
      return;
    }
    try {
      await postJson("/api/auth/password", { current_password: current.value, new_password: next.value });
      passwordForm.reset();
      status.replaceChildren(h("span", { class: "ok" }, "Password changed. Your other devices have been logged out."));
      loadSessions();
    } catch (err) {
      status.replaceChildren(errorBox(err));
    }
  });

  const sessionsBox = h("div", {});
  async function loadSessions() {
    try {
      const sessions = await api("/api/auth/sessions");
      const others = sessions.filter((s) => !s.current).length;
      // Wrapped in `h`, which flattens the array (`replaceChildren` doesn't).
      sessionsBox.replaceChildren(h("div", {},
        sessions.map((s) => h("div", { class: "box" },
          h("div", { class: "row" },
            h("strong", {}, s.user_agent || "unknown device"),
            s.current ? h("span", { class: "tag" }, "this device") : null,
            h("button", {
              class: "small",
              onclick: async () => {
                await api(`/api/auth/sessions/${s.id}`, { method: "DELETE" });
                if (s.current) location.reload(); else loadSessions();
              },
            }, "Log out")),
          kv([
            ["address", s.ip || "unknown"],
            ["logged in", fmtTime(s.created_at)],
            ["last used", fmtTime(s.last_used_at)],
            ["expires", fmtTime(s.expires_at)],
          ]))),
        others ? h("div", { class: "row" }, h("button", {
          onclick: async () => {
            await api("/api/auth/sessions", { method: "DELETE" });
            loadSessions();
          },
        }, `Log out all other devices (${others})`)) : null));
    } catch (err) {
      sessionsBox.replaceChildren(errorBox(err));
    }
  }

  setDetail(
    h("h2", {}, me.username),
    h("h3", {}, "Change password"),
    passwordForm,
    h("h3", {}, "Where you're logged in"),
    sessionsBox);
  loadSessions();
}

// "Share…" on a track or album page: creates a link, optionally expiring.
function shareForm(kind, id) {
  const expiry = h("select", {},
    h("option", { value: "" }, "never expires"),
    h("option", { value: "1" }, "expires in 1 day"),
    h("option", { value: "7" }, "expires in 7 days"),
    h("option", { value: "30" }, "expires in 30 days"));
  const result = h("div", {});
  // Without it a link plays a lower-quality copy (Opus), which can't be saved as the original.
  const original = can("download_original")
    ? h("input", { type: "checkbox" })
    : null;
  const form = h("form", { class: "row" }, expiry,
    original ? h("label", { title: "Otherwise the link plays a lower-quality copy" }, original, " give the original file") : null,
    h("button", { type: "submit" }, "Create link"), result);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = { [`${kind}_id`]: id };
    if (expiry.value) body.expires_at = new Date(Date.now() + expiry.value * 86400000).toISOString();
    if (original?.checked) body.original = true;
    try {
      const share = await postJson("/api/shares", body);
      const url = location.origin + share.path;
      const input = h("input", { class: "share-link mono", readonly: true, value: url, onclick: (e) => e.target.select() });
      result.replaceChildren(input, h("span", { class: "muted" }, "Copy it now: it can't be shown again."));
      input.select();
    } catch (err) {
      result.replaceChildren(errorBox(err));
    }
  });
  return h("details", {}, h("summary", {}, "Share…"), form);
}

// #/shares: your share links, with revoke buttons.
async function showShares() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  try {
    const shares = await api("/api/shares");
    setDetail(
      h("h2", {}, "Shares"),
      shares.length ? shares.map((s) => {
        const state = {
          creator_cannot_share: "not working: its creator can't share anymore",
          target_hidden: "not working: its creator can't see it anymore",
        }[s.state] || s.state;
        return h("div", { class: "box" },
          h("div", { class: "row" },
            h("a", { href: s.track_id ? `#/track/${s.track_id}` : `#/album/${s.album_id}` }, s.title),
            h("span", { class: "tag" }, s.track_id ? "track" : "album"),
            h("span", { class: "tag" }, s.original ? "original file" : "lower-quality copy"),
            h("span", { class: s.state === "active" ? "ok" : "muted" }, state),
            s.state !== "revoked" && s.state !== "expired" ? h("button", {
              class: "small",
              onclick: async () => { await api(`/api/shares/${s.id}`, { method: "DELETE" }); showShares(); },
            }, "Revoke") : null),
          kv([
            ["by", s.created_by],
            ["created", fmtTime(s.created_at)],
            ["expires", s.expires_at ? fmtTime(s.expires_at) : "never"],
          ]));
      }) : h("p", { class: "muted" }, "No shares yet. Share a track or album from its page."));
  } catch (err) {
    setDetail(errorBox(err));
  }
}

// /s/<token>: a share link. Logged-in users who can see the shared thing get its normal
// page; everyone else gets the share view (title, artist, cover, player).
async function startShare(token) {
  const shareView = document.getElementById("share-view");
  document.body.classList.add("share-mode");
  shareView.hidden = false;

  // Plain `fetch`, not `api`: a 401 here just means "not logged in", not "show login".
  const response = await fetch(`/api/share/${token}`);
  if (!response.ok) {
    shareView.replaceChildren(h("div", { class: "share" },
      h("h2", {}, "This link doesn't work"),
      h("p", { class: "muted" }, "It may have been revoked or expired.")));
    return;
  }
  const share = await response.json();

  const meResponse = await fetch("/api/auth/me");
  const user = meResponse.ok ? await meResponse.json() : null;
  if (user?.username) {
    // Can we see it the normal way? Then it's a link like any other: the desktop app gets
    // it (or the web UI's page, if the app doesn't take it; see open.html).
    const normal = await fetch(`/api/${share.kind}s/${share.id}`);
    if (normal.ok) {
      location.replace(`/open/${share.kind}/${share.id}`);
      return;
    }
  }

  const base = `/api/share/${token}`;
  const coverSrc = (coverId) => coverId && `${base}/covers/${coverId}`;
  const playTrack = (t) =>
    playSrc(`${base}/tracks/${t.id}/stream`, t.title, t.artist_display, coverSrc(t.cover_id ?? share.cover_id));

  shareView.replaceChildren(h("div", { class: "share" },
    h("div", { class: "share-head" },
      share.cover_id ? h("img", { src: coverSrc(share.cover_id), alt: "" }) : null,
      h("div", {},
        h("h2", {}, share.title),
        h("div", {}, share.artist),
        share.kind === "track"
          ? h("div", { class: "row" }, h("button", { onclick: () => playTrack(share.tracks[0]) }, "▶ Play"))
          : null)),
    share.kind === "album"
      ? share.tracks.map((t) => h("div", { class: "share-track" },
          h("button", { class: "small", onclick: () => playTrack(t) }, "▶"),
          h("div", { class: "grow" }, h("div", {}, t.title), h("div", { class: "muted" }, t.artist_display)),
          h("span", { class: "muted" }, fmtDuration(t.duration_ms))))
      : null));
}

const shareMatch = location.pathname.match(/^\/s\/([0-9a-f]+)$/);
if (shareMatch) startShare(shareMatch[1]);
else start();
