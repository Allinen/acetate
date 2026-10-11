// The page for a link to a track, album or artist (/open/<kind>/<id>), sent by someone
// with an account. It's handed to the desktop app (acetate://open/…, the address the app is
// set up for), and the tab closes itself if the browser lets it (browsers only let a page
// close a tab a script opened: otherwise it says it can be closed).

const status = document.getElementById("status");
const hint = document.getElementById("hint");
const appLink = document.getElementById("app-link");

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
      status.textContent = `Didn't the app open? Click “Open in the Acetate app”.`;
      hint.textContent = "For links to open in the app, it has to be set up for them: on Windows it does that itself when it starts, on Linux its install.sh does (Settings → Links in the app says whether it is).";
    }
  }, 1500);
}

open();
