// The page for a link to a track, album or artist (/open/<kind>/<id>), sent by someone
// with an account. Logged in here: it's handed to the desktop app (acetate://open/…, the
// address the app is set up for), and the tab closes itself if the browser lets it
// (browsers only let a page close a tab a script opened: otherwise it says it can be
// closed). Not logged in, or the app didn't take it: the web UI's page for it, which asks
// to log in first if needed.

const status = document.getElementById("status");
const hint = document.getElementById("hint");
const appLink = document.getElementById("app-link");
const webLink = document.getElementById("web-link");

async function open() {
  const match = location.pathname.match(/^\/open\/(track|album|artist)\/([0-9a-f-]{36})\/?$/);
  if (!match) {
    status.textContent = "This isn't a link Acetate knows.";
    appLink.hidden = true;
    return;
  }
  const [, kind, id] = match;
  const app = `acetate://open/${kind}/${id}?server=${encodeURIComponent(location.origin)}`;
  appLink.href = app;
  webLink.href = `/#/${kind}/${id}`;

  let me = null;
  try {
    const response = await fetch("/api/auth/me");
    if (response.ok) me = await response.json();
  } catch {
    // The server can't be reached: the buttons still work.
  }
  if (!me?.username) {
    status.textContent = `A ${kind} on this Acetate server. Log in to see it (in the browser), or open it in the app.`;
    webLink.textContent = "Log in and open it";
    return;
  }

  status.textContent = `Opening the ${kind} in Acetate…`;
  // The app took it if this tab lost the focus to it.
  let left = false;
  window.addEventListener("blur", () => { left = true; });
  document.addEventListener("visibilitychange", () => { if (document.hidden) left = true; });
  location.href = app;
  setTimeout(() => {
    if (left) {
      status.textContent = "Opened in Acetate. You can close this tab.";
      window.close();
    } else {
      status.textContent = `Didn't the app open? Open the ${kind} in the browser instead.`;
      hint.textContent = "For links to open in the app, it has to be set up for them: on Windows it does that itself when it starts, on Linux its install.sh does (Settings → Links in the app says whether it is).";
    }
  }, 1500);
}

open();
