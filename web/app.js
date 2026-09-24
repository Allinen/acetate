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
  const params = new URLSearchParams({ limit: PAGE });
  if (lastId) params.set("before", lastId);
  if (search.value.trim()) params.set("q", search.value.trim());

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
  audio.src = `/api/files/${fileId}/stream`;
  audio.play().catch((err) => {
    playerInfo.replaceChildren(errorBox(`Can't play: ${err.message}`));
  });
  playerInfo.replaceChildren(
    h("div", {}, h("strong", {}, title)),
    h("div", { class: "muted" }, artist, note ? ` · ${note}` : ""));
  playerCover.hidden = !coverId;
  if (coverId) playerCover.src = `/api/covers/${coverId}`;
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
          h("button", { onclick: () => play(f.id, t.title, t.artist_display, cover?.id) }, "▶ Play"),
          h("a", { href: `/api/files/${f.id}/stream`, download: f.original_filename }, "Download original")))),

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
      c.credited_as ? h("span", { class: "muted" }, ` credited as "${c.credited_as}"`) : null))
      : h("p", { class: "muted" }, "None."),

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
    await replaceFileForm(id, reload),

    t.file_history.length ? [
      h("h3", {}, "Previous files"),
      t.file_history.map((c) => h("div", { class: "box" },
        h("div", { class: "row" },
          h("strong", {}, c.old_original_filename),
          h("span", { class: "muted" }, [c.old_codec, c.old_bitrate && `${c.old_bitrate} kbps`].filter(Boolean).join(" ")),
          h("button", {
            class: "small",
            onclick: () => play(c.old_file_id, t.title, t.artist_display, cover?.id, "previous file"),
          }, "▶")),
        kv([
          ["replaced", fmtTime(c.replaced_at)],
          ["reason", c.reason],
          ["origin", originView(c.old_origin)],
        ]))),
    ] : null,

    h("h3", {}, "Sources"),
    sourcesView(t.sources),
    await addSourceForm("track", id, reload),

    t.lyrics.length ? [
      h("h3", {}, "Lyrics"),
      t.lyrics.map((l) => h("div", { class: "box" },
        h("div", { class: "row" },
          h("span", { class: "tag" }, l.is_official ? "official" : "unofficial"),
          h("span", { class: "tag" }, l.is_synced ? "synced" : "plain"),
          h("span", { class: "muted" }, l.source)),
        h("pre", {}, l.body))),
    ] : null,

    h("h3", {}, "Revisions"),
    t.revisions.map((r) => h("details", {},
      h("summary", {},
        fmtTime(r.created_at), " · ",
        t.variants.find((v) => v.id === r.variant_id)?.label ?? "?", " · ",
        r.source, r.reason ? ` · ${r.reason}` : "", r.is_canon ? " · canon" : ""),
      h("pre", {}, JSON.stringify(r.data, null, 2)))),

    h("h3", {}, "Embedded tags (verbatim)"),
    (f.embedded_tags.tags || []).map((tag) => h("details", {},
      h("summary", {}, tag.format, ` (${tag.items.length} items)`),
      h("table", {}, tag.items.map((item) => h("tr", {},
        h("td", { class: "mono" }, item.key, item.description ? `:${item.description}` : ""),
        h("td", { class: "mono" },
          typeof item.value === "string" ? item.value : JSON.stringify(item.value))))))),
  );
}

async function showArtist(id) {
  setDetail(h("p", { class: "muted" }, "Loading…"));
  try {
    const a = await api(`/api/artists/${id}`);
    setDetail(
      h("h2", {}, a.name),
      a.disambiguation ? h("div", { class: "muted" }, a.disambiguation) : null,
      kv([
        ["aliases", a.aliases.join(", ")],
        ["artist id", h("span", { class: "mono" }, a.id)],
      ]),
      h("div", { class: "row" },
        h("a", { href: "#/", onclick: () => { search.value = a.name; loadTracks(true); } },
          "Search tracks by this name")),
      h("h3", {}, "Sources"),
      sourcesView(a.sources),
      await addSourceForm("artist", id, () => showArtist(id)));
  } catch (err) {
    setDetail(errorBox(err));
  }
}

async function showAlbum(id) {
  setDetail(h("p", { class: "muted" }, "Loading…"));
  try {
    const a = await api(`/api/albums/${id}`);
    setDetail(
      h("h2", {}, a.title),
      h("div", { class: "row" }, h("span", { class: "tag" }, a.kind)),
      kv([
        ["release date", a.release_date && h("span", { class: "mono" }, a.release_date)],
        ["album id", h("span", { class: "mono" }, a.id)],
      ]),
      h("h3", {}, "Sources"),
      sourcesView(a.sources),
      await addSourceForm("album", id, () => showAlbum(id)));
  } catch (err) {
    setDetail(errorBox(err));
  }
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
  const status = h("select", {},
    ["released", "unreleased", "demo", "unfinished", "deleted"].map((s) => h("option", { value: s }, s)));
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
      body.append("origin", JSON.stringify(o));
      if (sources.length) body.append("sources", JSON.stringify(sources));
      body.append("file", file);
      try {
        const [r] = await api("/api/upload", { method: "POST", body });
        line.lastChild.replaceWith(r.track_id
          ? h("a", { href: `#/track/${r.track_id}`, class: r.status === "created" ? "ok" : "" }, r.status)
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
  else setDetail(h("p", { class: "muted" }, "Select a track. Double-click to play."));
}

window.addEventListener("hashchange", route);
loadTracks(true);
route();
