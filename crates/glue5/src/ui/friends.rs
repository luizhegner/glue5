//! Friends, as Social Club's overlay had them: the friends with what they
//! play (Join, Invite), the friend requests, the game invites, and finding
//! players (the ones met in sessions, or by friend code); and its
//! notifications.
use glue5_runtime::friends::{self, AddError, FriendInfo, Notice, Playing, SessionKind, Snapshot};

use super::pages::sheet;
use super::widgets::{Control, action_button, action_buttons, focus, segmented, setting, text_field};
use super::{Action, TextKind, WARN, theme};
use egui::{Color32, RichText, Stroke};

/// The page's tabs, Social Club's: Friends, Friend Requests, Game Invites,
/// Find Players.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Friends,
    Requests,
    Invites,
    Find,
}

/// A notification: title, the friend's name, a line; the tab it opens.
#[derive(Clone, Debug)]
pub struct Toast {
    pub title: &'static str,
    pub name: String,
    pub text: String,
    pub tab: Tab,
}

/// A friends' notification, worded as Social Club's overlay did.
pub fn toast(notice: &Notice) -> Toast {
    let toast = |title, name: &str, text: &str, tab| Toast { title, name: name.to_owned(), text: text.to_owned(), tab };
    match notice {
        Notice::Summary { online, requests } => {
            let text = match (*online, *requests) {
                (0, r) => format!("{r} Request(s)"),
                (o, 0) => format!("{o} Online"),
                (o, r) => format!("{o} Online · {r} Request(s)"),
            };
            let tab = if *requests > 0 { Tab::Requests } else { Tab::Friends };
            Toast { title: "Friends", name: String::new(), text, tab }
        }
        Notice::Online(name) => toast("Friend Online", name, "Is now online.", Tab::Friends),
        Notice::Offline(name) => toast("Friend Offline", name, "Has gone offline.", Tab::Friends),
        Notice::Request(name) => toast("Friend Request", name, "Wants to be your friend.", Tab::Requests),
        Notice::Accepted(name) => toast("Friend Request", name, "Accepted your friend request.", Tab::Friends),
        Notice::Invite(name) => toast("Multiplayer Invite", name, "Sent you a multiplayer invite.", Tab::Invites),
    }
}

/// A notification's card, `width` wide.
pub(super) fn draw_toast(ui: &mut egui::Ui, t: &Toast, width: f32) {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(theme::expressive::SURFACE)
        .stroke(Stroke::new(1.0, theme::expressive::OUTLINE))
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric((14.0 * scale) as i8, (10.0 * scale) as i8))
        .show(ui, |ui| {
            ui.set_width(width - 28.0 * scale);
            ui.horizontal_top(|ui| {
                avatar(ui, &t.name, 34.0 * scale);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0 * scale;
                    ui.label(theme::heading(t.title, 15.0 * scale).color(Color32::WHITE));
                    if !t.name.is_empty() {
                        ui.label(RichText::new(&t.name).size(13.0 * scale).strong().color(Color32::WHITE));
                    }
                    ui.label(RichText::new(&t.text).size(12.0 * scale).color(theme::MUTED));
                });
            });
        });
}

/// A square badge with a name's initial, in the shared UI accent.
fn avatar(ui: &mut egui::Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let initial = name.chars().next().map_or_else(|| "♡".to_owned(), |c| c.to_uppercase().collect());
    ui.painter().rect_filled(rect, 14, theme::expressive::PRIMARY_CONTAINER);
    let font = egui::FontId::proportional(size * 0.5);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, font, theme::expressive::ON_SURFACE);
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct FriendsPage {
    pub(super) tab: Tab,
    /// The code being typed.
    code: String,
    /// What came of the last thing done (and whether it went wrong).
    message: Option<(String, bool)>,
    /// The friend whose removal waits for a second press.
    removing: Option<String>,
}

/// What the game lets the page do: `started`, a game runs; `online`, it is
/// GTA Online (friends are invited to its session, joined from it).
pub struct Context {
    pub started: bool,
    pub online: bool,
}

/// True when closed.
pub(super) fn friends_page(ctx: &egui::Context, page: &mut FriendsPage, game: Context, actions: &mut Vec<Action>) -> bool {
    // What the friends do comes from the network: the page follows it.
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let snapshot = friends::snapshot();
    sheet(ctx, "friends-window", "FRIENDS", |ui| {
        let scale = theme::density(ui);
        let count = |label: &str, n: usize| if n > 0 { format!("{label} {n}") } else { label.to_owned() };
        let invites = snapshot.friends.iter().filter(|f| f.invited).count();
        let tabs = [
            (Tab::Friends, "FRIENDS".to_owned()),
            (Tab::Requests, count("REQUESTS", snapshot.requests.len())),
            (Tab::Invites, count("INVITES", invites)),
            (Tab::Find, "FIND".to_owned()),
        ];
        let tabs: Vec<(Tab, &str)> = tabs.iter().map(|(t, l)| (*t, l.as_str())).collect();
        if segmented(ui, &mut page.tab, &tabs, 40.0 * scale) {
            page.message = None;
        }
        ui.add_space(10.0 * scale);
        if let Some((text, wrong)) = &page.message {
            ui.label(RichText::new(text).size(12.0 * scale).color(if *wrong { WARN } else { theme::MUTED }));
        }
        match page.tab {
            Tab::Friends => friends_tab(ui, &snapshot, page, &game, actions),
            Tab::Requests => requests_tab(ui, &snapshot, page),
            Tab::Invites => invites_tab(ui, &snapshot, &game, actions),
            Tab::Find => find_tab(ui, &snapshot, page),
        }
    })
}

fn muted(ui: &mut egui::Ui, text: &str) {
    let scale = theme::density(ui);
    ui.label(RichText::new(text).size(12.0 * scale).color(theme::MUTED));
}

fn heading(ui: &mut egui::Ui, text: &str) {
    let scale = theme::density(ui);
    ui.add_space(4.0 * scale);
    ui.label(theme::heading(text, 17.0 * scale).color(theme::MUTED));
}

fn friends_tab(ui: &mut egui::Ui, snapshot: &Snapshot, page: &mut FriendsPage, game: &Context, actions: &mut Vec<Action>) {
    let online = snapshot.friends.iter().filter(|f| f.playing.is_some()).count();
    heading(ui, &format!("ONLINE {online} OF {}", snapshot.friends.len()));
    if snapshot.friends.is_empty() {
        muted(ui, "No friends yet. Find Players: the players you meet in sessions, or a friend's code.");
    }
    for f in &snapshot.friends {
        friend_row(ui, f, page, game, actions);
    }
    ui.add_space(4.0);
    muted(
        ui,
        "Friends find each other directly, with no server: they see when you play, invite you and join you. \
         What you say to each other is encrypted; like any player you meet online, a friend sees your Internet \
         address.",
    );
}

fn friend_row(ui: &mut egui::Ui, f: &FriendInfo, page: &mut FriendsPage, game: &Context, actions: &mut Vec<Action>) {
    let status = match (f.playing, f.kind) {
        (None, _) => "Offline",
        (Some(Playing::Menu), _) => "Online",
        (Some(Playing::Story), _) => "Playing Grand Theft Auto V, Story Mode",
        (Some(Playing::Online), Some(SessionKind::Friends)) => "Playing GTA Online, Closed Friend Session",
        (Some(Playing::Online), Some(SessionKind::Closed)) => "Playing GTA Online, Invite Only Session",
        (Some(Playing::Online), Some(SessionKind::Solo)) => "Playing GTA Online, Solo Session",
        (Some(Playing::Online), _) => "Playing GTA Online, Public Session",
    };
    // The font has no round dot: its bullet.
    let dot = if f.playing.is_some() { "• " } else { "" };
    let sure = page.removing.as_deref() == Some(f.key.as_str());
    // Its session, when friends may join it (or it invited); an invite to
    // this player's, playing GTA Online.
    let can_join = f.lobby.is_some() && f.playing == Some(Playing::Online) && (!game.started || game.online);
    let can_invite = game.online && f.playing.is_some();
    ui.push_id(&f.key, |ui| {
        setting(ui, &format!("{dot}{}", f.name), Control::Actions, Some(status), |ui| {
            // A phone's row has room for short words.
            let short = ui.available_width() < 250.0 * theme::density(ui);
            let mut labels = vec![if sure { "Remove?" } else { "🗙" }];
            labels.extend(can_join.then_some(if short { "Join" } else { "Join game" }));
            labels.extend(can_invite.then_some(if short { "Invite" } else { "Invite to game" }));
            let mut buttons = action_buttons(ui, &labels).into_iter();
            if buttons.next().is_some_and(|b| b.on_hover_text("Remove friend").clicked()) {
                if sure {
                    friends::remove(&f.key);
                    page.message = Some((format!("{} removed from your friends.", f.name), false));
                    page.removing = None;
                } else {
                    page.removing = Some(f.key.clone());
                }
            }
            if can_join && buttons.next().is_some_and(|b| b.clicked()) {
                join(f, game, actions);
                page.message = Some((format!("Joining {}…", f.name), false));
            }
            if can_invite && buttons.next().is_some_and(|b| b.clicked()) {
                friends::invite(&f.key);
                page.message = Some((format!("Game Invite Sent to {}", f.name), false));
            }
        });
    });
}

fn requests_tab(ui: &mut egui::Ui, snapshot: &Snapshot, page: &mut FriendsPage) {
    heading(ui, "FRIEND REQUESTS");
    if snapshot.requests.is_empty() {
        muted(ui, "No pending friend requests.");
    }
    for (key, name) in &snapshot.requests {
        ui.push_id(key, |ui| {
            setting(ui, name, Control::Actions, Some("Wants to be your friend."), |ui| {
                let [decline, accept] = <[_; 2]>::try_from(action_buttons(ui, &["Decline", "Accept"])).unwrap();
                if decline.clicked() {
                    friends::answer(key, false);
                    page.message = Some((format!("Friend request from {name} declined."), false));
                }
                if accept.clicked() {
                    friends::answer(key, true);
                    page.message = Some((format!("Friend request from {name} accepted."), false));
                }
            });
        });
    }
    if !snapshot.asked.is_empty() {
        heading(ui, "SENT");
        for a in &snapshot.asked {
            let state = match (a.declined, a.reached) {
                (true, _) => "Declined",
                (false, true) => "Friend Request Pending",
                (false, false) => "Looking for them: they need Glue 5 open",
            };
            ui.push_id(&a.id, |ui| {
                setting(ui, &a.label, Control::Action, Some(state), |ui| {
                    if action_button(ui, if a.declined { "Dismiss" } else { "Cancel" }).clicked() {
                        friends::cancel(&a.id);
                    }
                });
            });
        }
    }
}

fn invites_tab(ui: &mut egui::Ui, snapshot: &Snapshot, game: &Context, actions: &mut Vec<Action>) {
    let invites: Vec<&FriendInfo> = snapshot.friends.iter().filter(|f| f.invited).collect();
    heading(ui, "GAME INVITES");
    if invites.is_empty() {
        muted(ui, "No pending game invites.");
    }
    for f in invites {
        ui.push_id(("invite", &f.key), |ui| {
            setting(ui, &f.name, Control::Actions, Some("Sent you a multiplayer invite."), |ui| {
                // Accepted from the start page or GTA Online (not story mode).
                let can_accept = !game.started || game.online;
                let labels: &[&str] = if can_accept { &["Decline", "Accept"] } else { &["Decline"] };
                let buttons = action_buttons(ui, labels);
                if buttons[0].clicked() {
                    friends::ignore_invite(&f.key);
                }
                if buttons.get(1).is_some_and(|b| b.clicked()) {
                    join(f, game, actions);
                }
            });
        });
    }
}

fn find_tab(ui: &mut egui::Ui, snapshot: &Snapshot, page: &mut FriendsPage) {
    heading(ui, "PLAYERS YOU MET");
    if snapshot.recent.is_empty() {
        muted(ui, "The players you meet in GTA Online's sessions show here: send them a friend request.");
    }
    for r in snapshot.recent.iter().take(30) {
        let state = match (r.asked, r.here) {
            (true, _) => "Friend Request Pending",
            (false, true) => "In your session",
            (false, false) => "Played with you",
        };
        ui.push_id(("recent", &r.key), |ui| {
            setting(ui, &r.name, Control::Action, Some(state), |ui| {
                // Asked while it plays in one of this player's lobbies.
                if !r.asked && r.here && action_button(ui, "Send friend request").clicked() {
                    friends::ask_player(&r.key);
                    page.message = Some((format!("Friend Request Sent to {}", r.name), false));
                }
            });
        });
    }
    heading(ui, "FRIEND CODE");
    your_code(ui, snapshot, page);
    ask(ui, page);
}

/// Joins a friend's session: from the start page into its lobby first;
/// from GTA Online, the game asks whether to leave this session.
fn join(f: &FriendInfo, game: &Context, actions: &mut Vec<Action>) {
    if game.started {
        friends::join(&f.key);
        actions.push(Action::CloseFriends);
    } else if let Some(lobby) = &f.lobby {
        actions.push(Action::JoinFriend { key: f.key.clone(), lobby: lobby.clone() });
    }
}

fn your_code(ui: &mut egui::Ui, snapshot: &Snapshot, page: &mut FriendsPage) {
    let code = snapshot.code.clone().or_else(friends::my_code).unwrap_or_default();
    let what = "Your friend code: with it a player can send you a friend request";
    setting(ui, &code, Control::Actions, Some(what), |ui| {
        let [new, copy] = <[_; 2]>::try_from(action_buttons(ui, &["New", "Copy"])).unwrap();
        if new.on_hover_text("A new code: the old one stops working").clicked() {
            friends::new_code();
            page.message = Some(("A new code: the old one no longer reaches you.".into(), false));
        }
        if copy.clicked() {
            ui.ctx().copy_text(code.clone());
            page.message = Some(("Code copied.".into(), false));
        }
    });
}

fn ask(ui: &mut egui::Ui, page: &mut FriendsPage) {
    let scale = theme::density(ui);
    ui.horizontal(|ui| {
        let button = 92.0 * scale;
        let field = ui.available_width() - button - ui.spacing().item_spacing.x;
        let typed = ui
            .allocate_ui(egui::vec2(field, 44.0 * scale), |ui| {
                text_field(ui, "Code", TextKind::Code, &mut page.code, "A friend's code", 24)
            })
            .inner;
        let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let send = egui::Button::new(RichText::new("Send").size(14.0 * scale))
            .corner_radius(14)
            .min_size(egui::vec2(button, 44.0 * scale));
        let sent = focus(ui.add_enabled(!page.code.trim().is_empty(), send));
        if sent.clicked() || enter && !page.code.trim().is_empty() {
            page.message = Some(match friends::ask(&page.code) {
                Ok(()) => {
                    page.code.clear();
                    ("Friend Request Sent: they will see it once they open Glue 5.".into(), false)
                }
                Err(AddError::Invalid) => ("That is not a friend code: check it again.".into(), true),
                Err(AddError::Own) => ("That is your own code.".into(), true),
                Err(AddError::Asked) => ("A friend request is already pending.".into(), true),
            });
        }
    });
}
