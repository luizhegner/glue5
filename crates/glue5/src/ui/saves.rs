use std::time::{Duration,Instant,SystemTime};
use super::{Action,Status,shell,widgets,theme};

#[derive(Default)]
pub(super) struct Library {
    refreshed:Option<Instant>,
    entries:Vec<crate::saves::Save>,
    error:Option<String>,
}
impl Library {
    pub fn show(&mut self,ctx:&egui::Context,status:&Status,actions:&mut Vec<Action>)->bool {
        if self.refreshed.is_none_or(|t|t.elapsed()>Duration::from_secs(2)) {
            let result=crate::paths::user_dir().map_err(|error|error.to_string())
                .and_then(|user|crate::saves::list(&user).map_err(|error|error.to_string()));
            match result {Ok(entries)=>{self.entries=entries;self.error=None;},Err(error)=>self.error=Some(error.to_string())}
            self.refreshed=Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        shell::frame(ctx,"STORY MODE · SAVES",|ui| {
            let editable=status.started.is_none() && status.preparing.is_none();
            ui.add_enabled_ui(editable,|ui| {
                if widgets::row(ui,"Import saves","Choose the profile folder on your phone; existing files are backed up by the importer",|ui|
                    widgets::action_value(ui,"IMPORT")).clicked() {actions.push(Action::ImportSave);}
            });
            if widgets::row(ui,"Installed saves","Refresh the files available on this device",|ui|widgets::action_value(ui,"REFRESH")).clicked() {self.refreshed=None;}
            if !editable {ui.label(theme::note("Close the running game before importing or loading another save",12.0));}
            if let Some(error)=&self.error {ui.label(egui::RichText::new(error).color(super::WARN));}
            if self.entries.is_empty() {ui.label("No saves installed. Import a GTA V profile folder to begin.");}
            for save in &self.entries {
                let age=save.modified.and_then(|t|SystemTime::now().duration_since(t).ok()).map_or("date unavailable".to_owned(),|d| {
                    if d.as_secs()<3600 {format!("{} min ago",d.as_secs()/60)}else{format!("{} h ago",d.as_secs()/3600)}
                });
                let label=format!("{} · {}",save.name,save.profile);
                let help=format!("{} KiB · {age} · start Story mode from a separate copy",save.bytes.div_ceil(1024));
                ui.push_id(&save.relative,|ui| {ui.add_enabled_ui(editable,|ui| {
                    if widgets::row(ui,&label,&help,|ui|widgets::action_value(ui,"LOAD STORY")).clicked() {
                        actions.push(Action::LoadSave(save.relative.clone()));
                    }
                });});
            }
            ui.label(theme::note("Original saves are kept. Progress from a selected save is stored in its own Session profile and appears here.",11.5));
        })
    }
}
