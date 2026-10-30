// Mods in and out of the game as zips (used from Rust, src/actions.rs).

// Asks for a .zip and sends it to `url` (the game's /import, with the
// token): the imported mod's name, null when none was chosen.
export function importMod(url) {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".zip,application/zip";
    input.onchange = async () => {
      const file = input.files && input.files[0];
      if (!file) return resolve(null);
      try {
        const r = await fetch(url + "&name=" + encodeURIComponent(file.name), { method: "POST", body: file });
        const text = await r.text();
        if (r.ok) resolve(JSON.parse(text).mod); else reject(text);
      } catch (e) {
        reject(String(e));
      }
    };
    input.click();
  });
}

// Pairs this browser with the game (`POST /pair`): the code shown in the
// game for a token of its own, for `scope` ("studio" or "control").
// Resolves to { token, id, scope, name }; rejects with the HTTP status (401:
// a wrong code), or 0 when the game did not answer.
export function pairWith(code, scope) {
  return fetch("/pair", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, scope }),
  }).then(
    (r) => (r.ok ? r.json() : Promise.reject(r.status)),
    () => Promise.reject(0),
  );
}

// The code editor (editor.js, CodeMirror), loaded the first time the Studio
// needs it: the control page never does.
export function loadEditor() {
  return import("/editor.js").then((m) => {
    globalThis.StudioEditor = m.StudioEditor;
  });
}
