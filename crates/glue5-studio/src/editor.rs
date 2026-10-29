//! The code editor (js/editor.js: CodeMirror with the language server),
//! loaded when the Studio first needs it ([`load_editor`]; the control page
//! never does): its class is then the page's global `StudioEditor`.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/files.js")]
extern "C" {
    /// Asks for a .zip and sends it to the game's `/import` (`url`, with the
    /// token): the imported mod's name, or null.
    #[wasm_bindgen(js_name = importMod)]
    pub fn import_mod(url: &str) -> js_sys::Promise;
    /// Pairs with the game: the code for a token (`{ token, id, scope, name }`;
    /// rejects with the HTTP status, 0 when the game did not answer).
    #[wasm_bindgen(js_name = pairWith)]
    pub fn pair_with(code: &str, scope: &str) -> js_sys::Promise;
    /// Loads the editor's module (CodeMirror): `StudioEditor` exists once it resolves.
    #[wasm_bindgen(js_name = loadEditor)]
    pub fn load_editor() -> js_sys::Promise;
}

#[wasm_bindgen(module = "/js/viewport.js")]
extern "C" {
    /// Keeps `--kb` on the page's root at the on-screen keyboard's height
    /// (iOS Safari does not resize the page for it) and brings a menu's
    /// field being typed in above it.
    #[wasm_bindgen(js_name = watchKeyboard)]
    pub fn watch_keyboard();
}

#[wasm_bindgen]
extern "C" {
    pub type StudioEditor;

    #[wasm_bindgen(constructor)]
    pub fn new(parent: &web_sys::HtmlElement, lsp_url: &str) -> StudioEditor;
    /// Moves the editor into `parent` (a page made again), with the language
    /// server's new address.
    #[wasm_bindgen(method)]
    pub fn attach(this: &StudioEditor, parent: &web_sys::HtmlElement, lsp_url: &str);
    /// Stops connecting to the language server (the code went stale).
    #[wasm_bindgen(method)]
    pub fn pause(this: &StudioEditor);
    #[wasm_bindgen(method)]
    pub fn open(this: &StudioEditor, path: &str, text: &str, read_only: bool);
    #[wasm_bindgen(method)]
    pub fn close(this: &StudioEditor, path: &str);
    #[wasm_bindgen(method)]
    pub fn text(this: &StudioEditor, path: &str) -> String;
    #[wasm_bindgen(method)]
    pub fn saved(this: &StudioEditor, path: &str, text: &str);
    #[wasm_bindgen(method)]
    pub fn replace(this: &StudioEditor, path: &str, text: &str);
    #[wasm_bindgen(method)]
    pub fn reveal(this: &StudioEditor, line: u32, column: u32);
    #[wasm_bindgen(method)]
    pub fn insert(this: &StudioEditor, text: &str);
    #[wasm_bindgen(method)]
    pub fn focus(this: &StudioEditor);
    /// The console's input, made in (or moved to) `parent`; `on_run` gets what runs.
    #[wasm_bindgen(method)]
    pub fn repl(this: &StudioEditor, parent: &web_sys::HtmlElement, on_run: &Closure<dyn FnMut(String)>);
    #[wasm_bindgen(method, js_name = runRepl)]
    pub fn run_repl(this: &StudioEditor);
    #[wasm_bindgen(method, js_name = focusRepl)]
    pub fn focus_repl(this: &StudioEditor);

    #[wasm_bindgen(method, js_name = onChange)]
    pub fn on_change(this: &StudioEditor, f: &Closure<dyn FnMut(String, bool)>);
    #[wasm_bindgen(method, js_name = onSave)]
    pub fn on_save(this: &StudioEditor, f: &Closure<dyn FnMut(String)>);
    #[wasm_bindgen(method, js_name = onDiagnostics)]
    pub fn on_diagnostics(this: &StudioEditor, f: &Closure<dyn FnMut(String, String)>);
    #[wasm_bindgen(method, js_name = onCursor)]
    pub fn on_cursor(this: &StudioEditor, f: &Closure<dyn FnMut(u32, u32)>);
}
