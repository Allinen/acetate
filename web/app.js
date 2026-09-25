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
    const message = body && body.error ? body.error : `${response.status} ${response.statusText}`;
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
async function sourceWidget(kind) {
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
      const input = h("input", {
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
  render();

  const element = h("div", { class: "stack" }, h("label", {}, "platform", platform), fieldsBox);

  function read() {
    if (!platform.value) return null;
    const data = {};
    for (const [name, { input, field }] of Object.entries(inputs)) {
      if (input.value === "") continue;
      data[name] = field.type === "int" ? Number(input.value) : input.value;
    }
    const source = { platform: platform.value, data };
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

function play(fileId, title, artist, coverId, note) {
  playSrc(`/api/files/${fileId}/stream`, title, artist, coverId && `/api/covers/${coverId}`, note);
}

// Plays any stream URL (normal files, or through a share link).
function playSrc(src, title, artist, coverSrc, note) {
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

async function playTrackById(id) {
  const t = await api(`/api/tracks/${id}`);
  play(t.file.id, t.title, t.artist_display, t.covers[0]?.id);
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

function sourcesView(sources) {
  if (!sources.length) return h("p", { class: "muted" }, "None.");
  return sources.map((s) => h("div", { class: "box" },
    h("div", { class: "row" },
      h("span", { class: "tag" }, s.platform),
      s.external_id ? h("span", { class: "mono" }, s.external_id) : null,
      h("span", { class: "muted mono" }, `source ${s.id}`)),
    s.snapshots.map((snap, i) => h("details", { open: i === 0 },
      h("summary", {}, fmtTime(snap.captured_at), i === 0 ? " (latest)" : ""),
      snapshotData(snap.data)))));
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
  const reload = () => showTrack(id);
  const cover = t.covers[0];
  const f = t.file;
  for (const c of t.credits) {
    names.artist.set(c.artist_id, c.artist_name);
    if (c.alias_id) names.alias.set(c.alias_id, c.alias);
  }
  for (const a of t.albums) names.album.set(a.album_id, a.title);
  for (const r of t.relations) names.track.set(r.track_id, r.title);
  const variantLabel = (variantId) => t.variants.find((v) => v.id === variantId)?.label ?? "variant";

  setDetail(
    h("div", { class: "detail-head" },
      cover ? h("img", { src: `/api/covers/${cover.id}`, alt: "" }) : null,
      h("div", {},
        h("h2", {}, t.title),
        h("div", {}, t.artist_display),
        h("div", { class: "row" },
          h("span", { class: "tag" }, t.status),
          t.is_restricted ? h("span", { class: "tag" }, "restricted") : null,
          t.genres.map((g) => h("span", { class: "tag" }, g))),
        h("div", { class: "row" },
          can("stream") ? h("button", { onclick: () => play(f.id, t.title, t.artist_display, cover?.id) }, "▶ Play") : null,
          can("download_original")
            ? h("a", { href: `/api/files/${f.id}/stream`, download: f.original_filename }, "Download original")
            : null),
        can("share") ? shareForm("track", id) : null)),

    can("manage_access") ? [h("h3", {}, "Access"), await trackAccessBox(id, reload)] : null,

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
        ["original filename", f.original_filename],
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
      ])),
    can("replace_file") ? await replaceFileForm(id, reload) : null,

    // Only there with the `view_history` permission.
    t.file_history?.length ? [
      h("h3", {}, "Previous files"),
      t.file_history.map((c) => h("div", { class: "box" },
        h("div", { class: "row" },
          h("strong", {}, c.old_original_filename),
          h("span", { class: "muted" }, [c.old_codec, c.old_bitrate && `${c.old_bitrate} kbps`].filter(Boolean).join(" ")),
          can("stream") ? h("button", {
            class: "small",
            onclick: () => play(c.old_file_id, t.title, t.artist_display, cover?.id, "previous file"),
          }, "▶") : null),
        kv([
          ["replaced", fmtTime(c.replaced_at)],
          ["reason", c.reason],
          ["origin", originView(c.old_origin)],
        ]))),
    ] : null,

    h("h3", {}, "Sources"),
    sourcesView(t.sources),
    can("edit_sources") ? await addSourceForm("track", id, reload) : null,

    t.lyrics.length ? [
      h("h3", {}, "Lyrics"),
      t.lyrics.map((l) => h("div", { class: "box" },
        h("div", { class: "row" },
          h("span", { class: "tag" }, l.is_official ? "official" : "unofficial"),
          h("span", { class: "tag" }, l.is_synced ? "synced" : "plain"),
          h("span", { class: "muted" }, l.source)),
        h("pre", {}, l.body))),
    ] : null,

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
  const addRelation = picker("related track…", findTracks, (x) => [x.title, h("span", { class: "muted" }, " · ", x.artist_display)], (x) => {
    if (x.id === t.id) return;
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
    const aliasRows = a.aliases.map((al) => ({ id: al.id, input: h("input", { value: al.name }) }));
    const aliasList = h("div", {});
    function renderAliases() {
      aliasList.replaceChildren(h("div", { class: "stack" }, aliasRows.map((r, i) => h("div", { class: "row" },
        r.input,
        h("button", { type: "button", class: "small", onclick: () => { aliasRows.splice(i, 1); renderAliases(); } }, "✕")))));
    }
    renderAliases();
    const addAlias = h("button", {
      type: "button", class: "small",
      onclick: () => { aliasRows.push({ id: null, input: h("input", { placeholder: "exact spelling" }) }); renderAliases(); },
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
    const choose = picker("merge into…", findArtists,
      (x) => [x.name, x.disambiguation ? ` (${x.disambiguation})` : ""],
      (x) => {
        if (x.id === a.id) return;
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
    sourcesView(a.sources),
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
  names.album.set(a.id, a.title);
  for (const c of a.credits) names.artist.set(c.artist_id, c.artist_name);
  const reload = () => showAlbum(id);

  let editor = null;
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
  }

  setDetail(
    h("h2", {}, a.title),
    h("div", {}, a.credits.map((c) => c.alias ?? c.artist_name).join(", ")),
    h("div", { class: "row" }, h("span", { class: "tag" }, a.kind)),
    can("share") ? shareForm("album", id) : null,
    can("manage_access") ? await albumAccessBox(id, reload) : null,
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
    editor ? [h("h3", {}, "Edit"), editor] : null,
    h("h3", {}, "Sources"),
    sourcesView(a.sources),
    can("edit_sources") ? await addSourceForm("album", id, reload) : null,
    a.revisions ? [h("h3", {}, "History"),
      await historyView(a.revisions, () => "album", (r) => `/api/album-revisions/${r.id}/restore`, reload)] : null);
}

// #/review: things ingest flagged for a person to look at.
async function showReview() {
  selectedId = null;
  markSelected();
  setDetail(h("p", { class: "muted" }, "Loading…"));
  let flags;
  try {
    flags = await api("/api/review");
  } catch (err) {
    setDetail(errorBox(err));
    return;
  }
  setDetail(
    h("h2", {}, "Review"),
    flags.length ? flags.map((f) => {
      const resolution = h("input", { placeholder: "what you did (optional)" });
      const status = h("span", {});
      return h("div", { class: "box stack" },
        h("div", { class: "row" },
          h("span", { class: "tag" }, { possible_duplicate: "possible duplicate", ambiguous_artist: "ambiguous artist", merged_artist: "artist merged" }[f.kind] ?? f.kind),
          h("a", { href: `#/track/${f.track_id}` }, f.track_title),
          h("span", { class: "muted" }, f.track_artist, " · ", fmtTime(f.created_at))),
        f.kind === "possible_duplicate"
          ? h("div", {}, "Similar titles by the same artist: ",
              f.related_tracks.map((x) => [h("a", { href: `#/track/${x.id}` }, x.title), " "]))
          : f.kind === "merged_artist"
          ? h("div", {}, `Was credited to "${f.related.from_name}", which was merged into `,
              h("a", { href: `#/artist/${f.related.into}` }, f.related_artists.find((x) => x.id === f.related.into)?.name ?? "another artist"),
              ". Check the credits are still right (e.g. a featured artist that was part of the old name).")
          : h("div", {}, `"${f.related.name}" matches several artists; the oldest was credited: `,
              f.related_artists.map((x) => [h("a", { href: `#/artist/${x.id}` }, x.name,
                x.disambiguation ? ` (${x.disambiguation})` : "", x.id === f.related.chosen ? " ✓" : ""), " "])),
        h("div", { class: "row" }, resolution,
          h("button", {
            type: "button", class: "small",
            onclick: () => save(status, () => postJson(`/api/review/${f.id}/resolve`, { resolution: resolution.value }), showReview),
          }, "Resolve"), status));
    }) : h("p", { class: "muted" }, "Nothing to review."));
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
  const [, kind, id] = location.hash.split("/");
  if (kind === "track" && id) showTrack(id);
  else if (kind === "artist" && id) showArtist(id);
  else if (kind === "album" && id) showAlbum(id);
  else if (kind === "shares") showShares();
  else if (kind === "account") showAccount();
  else if (kind === "review") showReview();
  else if (kind === "groups") showGroups();
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
  document.getElementById("review-link").hidden = !can("edit_metadata");
  document.getElementById("groups-link").hidden = !can("manage_users");
  window.addEventListener("hashchange", route);
  loadTracks(true);
  route();
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

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
  const form = h("form", { class: "row" }, expiry, h("button", { type: "submit" }, "Create link"), result);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = { [`${kind}_id`]: id };
    if (expiry.value) body.expires_at = new Date(Date.now() + expiry.value * 86400000).toISOString();
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
        const state = s.revoked_at ? "revoked"
          : s.expires_at && new Date(s.expires_at) < new Date() ? "expired" : "active";
        return h("div", { class: "box" },
          h("div", { class: "row" },
            h("a", { href: s.track_id ? `#/track/${s.track_id}` : `#/album/${s.album_id}` }, s.title),
            h("span", { class: "tag" }, s.track_id ? "track" : "album"),
            h("span", { class: state === "active" ? "ok" : "muted" }, state),
            state === "active" ? h("button", {
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
    // Can we see it the normal way? Then go there instead.
    const normal = await fetch(`/api/${share.kind}s/${share.id}`);
    if (normal.ok) {
      location.replace(`/#/${share.kind}/${share.id}`);
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
