// The on-screen keyboard and the mods' menus (used from Rust, src/editor.rs).
// Chrome on Android resizes the page for the keyboard (the viewport meta's
// interactive-widget); iOS Safari does not: it only shrinks the visual
// viewport. `--kb` on the root is the height the keyboard covers, which the
// menus' padding adds, and a menu's field being typed in is brought into
// view once the keyboard is up.

export function watchKeyboard() {
  const vv = window.visualViewport;
  if (!vv) return;
  const root = document.documentElement;
  let waiting = null;
  const update = () => {
    const kb = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
    root.style.setProperty("--kb", kb + "px");
    if (waiting) {
      const el = waiting;
      waiting = null;
      el.scrollIntoView({ block: "nearest" });
    }
  };
  vv.addEventListener("resize", update);
  vv.addEventListener("scroll", update);
  document.addEventListener("focusin", (e) => {
    const el = e.target;
    if (!(el instanceof HTMLInputElement) || !el.closest(".menus")) return;
    if (el.type !== "text" && el.type !== "number" && el.type !== "search") return;
    // After the keyboard's resize, or soon if none comes (it was up already).
    waiting = el;
    setTimeout(() => {
      if (waiting === el) {
        waiting = null;
        el.scrollIntoView({ block: "nearest" });
      }
    }, 300);
  });
  update();
}
