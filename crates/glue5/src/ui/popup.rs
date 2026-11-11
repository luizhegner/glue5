//! A warning over a page: a full black screen carries the warning and its
//! choices, the one under the selection solid light with dark text.
//! Nothing behind it answers until it is
//! settled: a destructive line asks here first, instead of acting on the
//! press (or asking "Sure?" on the line itself, a second press away).
//!
//! A page asks with [`ask`]; the frame that drew the page opens it, and the
//! answer comes back as the [`Action`](super::Action) it stood for. The
//! shape follows the alert movies of the game's phone (`CREATE_ALERT`:
//! a title, a dialog box, its options) and the pause menu's warnings,
//! painted with the shell's own blocks and type so the two sit together.
use std::cell::RefCell;

use egui::*;

use super::theme::{self, fonts, metrics, palette};
use crate::sfx::{self, Sound};

/// What is done when the warning is accepted. One entry per line that must
/// ask first; the frame turns it into the page's action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    DiagnosticControl(u64),
    Quit,
    /// Delete the copy of the game this device streamed.
    ClearCopy,
    /// The game's own graphics settings back to the engine's defaults.
    ResetGraphics,
}

/// A warning to show: its words and what accepting it does. The choices are
/// the confirm's own word and a way back; the way back starts selected, so
/// a stray press can do no harm.
pub(super) struct Request {
    title: &'static str,
    body: String,
    confirm: &'static str,
    action: Confirm,
}

impl Request {
    pub(super) fn quit() -> Self {
        Request {
            title: "EXIT GLUE 5",
            body: "This closes the game and GLUE 5. Progress since your last save may be lost. Save in the game before exiting.".into(),
            confirm: "EXIT",
            action: Confirm::Quit,
        }
    }

    pub(super) fn clear_copy() -> Self {
        Request {
            title: "CLEAR THE LOCAL COPY",
            body: "This deletes the copy of the game on this device. Your game folder and your saves stay. \
                   What is gone is streamed again when the game next reads it.".into(),
            confirm: "CLEAR",
            action: Confirm::ClearCopy,
        }
    }

    pub(super) fn reset_graphics() -> Self {
        Request {
            title: "RESTORE GRAPHICS",
            body: "The game's own graphics settings go back to the engine's defaults. \
                   This application's settings (resolution scale, upscaling, frame rate) stay as they are.".into(),
            confirm: "RESET",
            action: Confirm::ResetGraphics,
        }
    }

    /// The two choices' words, confirm first.
    pub(super) fn diagnostic_control(id:u64, client:&str) -> Self {
        Request {
            title: "CONTROLE PELA IA",
            body: format!("{client} pediu controle exclusivo e acesso ao contexto, imagem e áudio do jogo, sob demanda. Não inclui microfone, outros apps ou tela do Android. Seus comandos de jogo ficarão bloqueados. Use Parar IA para encerrar e descartar capturas pendentes. O jogo pode alterar seu progresso: salve antes de autorizar. A autorização vale só para esta sessão."),
            confirm: "AUTORIZAR",
            action: Confirm::DiagnosticControl(id),
        }
    }

    fn options(&self) -> [&'static str; 2] {
        [self.confirm, "BACK"]
    }
}

thread_local! {
    /// A page asked during its frame; the frame opens it after the page.
    static ASK: RefCell<Option<Request>> = const { RefCell::new(None) };
}

/// Ask for a warning over the page being drawn.
pub(super) fn ask(request: Request) {
    ASK.with_borrow_mut(|asked| *asked = Some(request));
}

/// What a page asked for, once per frame.
pub(super) fn take() -> Option<Request> {
    ASK.with_borrow_mut(Option::take)
}

/// The keys a warning answers, taken out of a frame's input before the page
/// under it can see them (an arrow must not also move a slider's value, an
/// Enter not also press the line the warning is asking about).
#[derive(Clone, Copy, Default)]
pub(super) struct Keys {
    /// Steps of the selection, up or left against down or right.
    move_by: i32,
    accept: bool,
}

impl Keys {
    #[cfg(test)]
    fn accept() -> Self {
        Keys { move_by: 0, accept: true }
    }

    #[cfg(test)]
    fn down() -> Self {
        Keys { move_by: 1, accept: false }
    }
}

pub(super) fn take_keys(events: &mut Vec<Event>) -> Keys {
    let mut keys = Keys::default();
    events.retain(|event| {
        let Event::Key { key, pressed, .. } = event else {
            return true;
        };
        match key {
            Key::ArrowUp | Key::ArrowLeft => {
                keys.move_by -= i32::from(*pressed);
                false
            }
            Key::ArrowDown | Key::ArrowRight => {
                keys.move_by += i32::from(*pressed);
                false
            }
            Key::Enter | Key::Space => {
                keys.accept |= *pressed;
                false
            }
            _ => true,
        }
    });
    keys
}

/// The selection after a step of `delta` among `count` choices: it wraps,
/// as the game's lists do.
fn moved(selected: usize, delta: i32, count: usize) -> usize {
    (selected as i32 + delta).rem_euclid(count.max(1) as i32) as usize
}

/// A warning on show: the request and the choice under the selection.
pub(super) struct Open {
    request: Request,
    /// 0 confirms; the last way back starts selected.
    selected: usize,
}

impl Open {
    pub(super) fn diagnostic_id(&self)->Option<u64> {
        if let Confirm::DiagnosticControl(id)=self.request.action {Some(id)}else{None}
    }
    pub(super) fn new(request: Request) -> Self {
        let selected = request.options().len() - 1;
        Open { request, selected }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Still waiting for an answer.
    Open,
    Confirmed(Confirm),
    Cancelled,
}

/// Draws the warning over the whole screen and answers for it: the background
/// covers `screen` edge to edge, while its content stays inside `safe`,
/// the content rect the pages live in, so it never runs under a cut-out or
/// the on-screen keyboard. Dismissing it from outside (the Back button) is
/// the frame's own: dropping the [`Open`] is a cancel.
pub(super) fn show(ctx: &Context, open: &mut Open, screen: Rect, safe: Rect, keys: Keys) -> Outcome {
    if keys.move_by != 0 {
        let next = moved(open.selected, keys.move_by, open.request.options().len());
        if next != open.selected {
            open.selected = next;
            sfx::want(Sound::Focus);
        }
    }
    let mut outcome = Outcome::Open;
    Area::new(Id::new("popup"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            ctx.memory_mut(|m| m.set_modal_layer(ui.layer_id()));
            ui.set_width(screen.width());
            ui.set_height(screen.height());
            ui.set_clip_rect(screen);
            let painter = ui.painter().clone();
            // The whole screen goes black, corners and cut-out alike, and
            // the press that lands anywhere on it lands on the warning,
            // not on a line behind it.
            painter.rect_filled(screen, 0, Color32::BLACK);
            let _ = ui.interact(screen, Id::new("popup-backdrop"), Sense::click());

            let scale = theme::menu_scale(ctx, safe);
            let pad = 16.0 * scale;
            let width = (560.0 * scale).min((safe.width() - 32.0 * scale).max(1.0));
            let body = painter.layout(
                open.request.body.to_owned(),
                FontId::new(12.5 * scale, fonts::regular()),
                palette::TEXT_MID,
                (width - 2.0 * pad).max(1.0),
            );
            let header_h = 40.0 * scale;
            let row_h = metrics::ROW * scale;
            let gap = 2.0 * scale;
            let options = open.request.options();
            let options_h = row_h; // Both actions remain reachable above a landscape keyboard.
            let footer_h = 12.0 * scale;
            let height = header_h + pad + body.size().y + pad + options_h + pad + footer_h;
            let height = height.min(safe.height());
            // Centred on the screen, then slid back inside the safe area
            // if a cut-out or the keyboard would cover an edge of it.
            let half = vec2(width, height) * 0.5;
            let center = pos2(
                screen.center().x.clamp(safe.left() + half.x, (safe.right() - half.x).max(safe.left() + half.x)),
                screen.center().y.clamp(safe.top() + half.y, (safe.bottom() - half.y).max(safe.top() + half.y)),
            );
            let box_rect = Rect::from_center_size(center, vec2(width, height));

            // --- the title, and where the selection stands -----------------
            let mut title_job = text::LayoutJob::simple_singleline(open.request.title.into(), FontId::new(21.0 * scale, fonts::heavy()), Color32::from_rgb(240, 190, 50));
            title_job.wrap = text::TextWrapping::truncate_at_width((width - 2.0 * pad).max(1.0));
            let title = painter.layout_job(title_job);
            painter.galley(pos2(box_rect.center().x - title.size().x * 0.5, box_rect.top() + (header_h - title.size().y) * 0.5), title, Color32::from_rgb(240, 190, 50));

            // Keep actions at the bottom; only the explanation scrolls.
            let actions_top = (box_rect.bottom() - footer_h - pad - row_h).max(box_rect.top());
            let body_rect = Rect::from_min_max(
                pos2(box_rect.left() + pad, (box_rect.top() + header_h + pad).min(actions_top)),
                pos2(box_rect.right() - pad, (actions_top - pad).max((box_rect.top() + header_h + pad).min(actions_top))),
            );
            ui.scope_builder(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.set_clip_rect(body_rect.intersect(safe));
                ScrollArea::vertical().id_salt("confirmation-body").auto_shrink([false, false])
                    .max_height(body_rect.height()).show(ui, |ui| {
                        ui.add(Label::new(RichText::new(open.request.body.as_str()).font(FontId::new(12.5 * scale, fonts::regular())).color(palette::TEXT_MID)).wrap());
                    });
            });
            let action_width = ((width - 2.0 * pad - gap) * 0.5).min(150.0 * scale);
            for (index, word) in options.iter().enumerate() {
                let rect = Rect::from_min_size(pos2(box_rect.right() - pad - 2.0 * action_width - gap + index as f32 * (action_width + gap), actions_top), vec2(action_width, row_h));
                let response = ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
                    theme::apply_density(ui, scale);
                    super::widgets::tab_button(ui, word, index == open.selected, rect.size())
                }).inner;
                // Hover is not consent: keep the safe default until explicit input.
                if response.clicked() {
                    open.selected = index;
                    outcome = if index == 0 { Outcome::Confirmed(open.request.action) } else { Outcome::Cancelled };
                }
            }

        });
    if outcome == Outcome::Open && keys.accept {
        outcome = if open.selected == 0 {
            Outcome::Confirmed(open.request.action)
        } else {
            Outcome::Cancelled
        };
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme;

    #[test]
    fn the_selection_wraps_at_both_ends() {
        assert_eq!(moved(0, -1, 2), 1);
        assert_eq!(moved(1, 1, 2), 0);
        assert_eq!(moved(0, 1, 2), 1);
        assert_eq!(moved(1, -1, 2), 0);
        assert_eq!(moved(1, 0, 2), 1);
    }

    #[test]
    fn its_keys_leave_the_rest_of_the_input_alone() {
        let key = |key: Key, pressed: bool| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let mut events = vec![
            key(Key::ArrowDown, true),
            key(Key::ArrowDown, false),
            key(Key::Tab, true),
            key(Key::Enter, true),
            Event::PointerMoved(pos2(1.0, 2.0)),
        ];
        let keys = take_keys(&mut events);
        assert_eq!(keys.move_by, 1);
        assert!(keys.accept);
        assert_eq!(events.len(), 2, "Tab and the pointer stay for the page: {events:?}");
    }

    #[test]
    fn a_warning_starts_on_the_way_back_and_answers() {
        let ctx = Context::default();
        theme::install(&ctx);
        let area = Rect::from_min_size(Pos2::ZERO, vec2(844.0, 390.0));
        let mut open = Open::new(Request::clear_copy());
        let mut outcome = Outcome::Open;
        for (frame, keys) in [(0, Keys::default()), (1, Keys::accept())] {
            let mut output = ctx.run_ui(
                RawInput { screen_rect: Some(area), ..Default::default() },
                |root| {
                    outcome = show(root.ctx(), &mut open, area, area, keys);
                    let _ = frame;
                },
            );
            output.textures_delta.clear();
            if frame == 0 {
                assert_eq!(outcome, Outcome::Open, "unanswered, it stays");
            }
        }
        assert_eq!(outcome, Outcome::Cancelled, "accepting the opening selection is a cancel");

        // A step up from the way back lands on the confirm.
        let mut open = Open::new(Request::reset_graphics());
        let mut outcome = Outcome::Open;
        for keys in [Keys { move_by: -1, accept: false }, Keys::accept()] {
            let mut output = ctx.run_ui(
                RawInput { screen_rect: Some(area), ..Default::default() },
                |root| outcome = show(root.ctx(), &mut open, area, area, keys),
            );
            output.textures_delta.clear();
        }
        assert_eq!(outcome, Outcome::Confirmed(Confirm::ResetGraphics));
    }

    #[test]
    fn exit_requires_explicit_confirmation() {
        let open = Open::new(Request::quit());
        assert_eq!(open.selected, 1, "Back is the safe initial choice");
        assert_eq!(open.request.action, Confirm::Quit);
        assert!(open.request.body.contains("last save"));
    }

    #[test]
    fn diagnostic_consent_is_per_request_and_starts_on_decline() {
        let open=Open::new(Request::diagnostic_control(77,"Agente de teste"));
        assert_eq!(open.selected,1);
        assert_eq!(open.diagnostic_id(),Some(77));
        assert!(open.request.body.contains("Agente de teste"));
        assert_eq!(open.request.action,Confirm::DiagnosticControl(77));
        assert_eq!(Open::new(Request::quit()).diagnostic_id(),None);
    }

    #[test]
    fn a_step_down_wraps_the_same_way() {
        let mut open = Open::new(Request::clear_copy());
        open.selected = moved(open.selected, Keys::down().move_by, open.request.options().len());
        assert_eq!(open.selected, 0, "down from the last choice wraps to the first");
    }
}
