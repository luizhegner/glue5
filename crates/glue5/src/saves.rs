//! Save selection never rewrites an imported profile. A launch gets its own
//! writable snapshot; the runtime mounts that profile for this session only.
use std::{fs,io,path::{Path,PathBuf},time::{SystemTime,UNIX_EPOCH}};

#[derive(Clone,Debug)]
pub struct Save {
    pub relative:String,
    pub name:String,
    pub profile:String,
    pub bytes:u64,
    pub modified:Option<SystemTime>,
}
pub fn is_slot(name:&str)->bool {
    name.strip_prefix("SGTA5").is_some_and(|n|n.len()==4 && n.bytes().all(|b|b.is_ascii_digit()))
}
fn collect(user:&Path,dir:&Path,profile:&str,out:&mut Vec<Save>)->io::Result<()> {
    if !dir.exists() {return Ok(());}
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {return Ok(());}
    for file in fs::read_dir(dir)?.take(128) {
        let file=file?; let name=file.file_name().to_string_lossy().into_owned();
        if !is_slot(&name) || !file.file_type()?.is_file() {continue;}
        let meta=file.metadata()?;
        if meta.len()==0 || meta.len()>8*1024*1024 {continue;}
        let relative=file.path().strip_prefix(user).map_err(io::Error::other)?.to_string_lossy().replace('\\',"/");
        out.push(Save{relative,name,profile:profile.into(),bytes:meta.len(),modified:meta.modified().ok()});
    }
    Ok(())
}
pub fn list(user:&Path)->io::Result<Vec<Save>> {
    let root=user.join("Documents/Rockstar Games/GTA V"); let mut out=vec![];
    collect(user,&root.join("Default"),"Default",&mut out)?;
    let profiles=root.join("Profiles");
    if profiles.is_dir() && !fs::symlink_metadata(&profiles)?.file_type().is_symlink() {
        for entry in fs::read_dir(profiles)?.take(64) {
            let entry=entry?;
            if entry.file_type()?.is_dir() {collect(user,&entry.path(),&entry.file_name().to_string_lossy(),&mut out)?;}
        }
    }
    let sessions=user.join("save-sessions");
    if sessions.is_dir() && !fs::symlink_metadata(&sessions)?.file_type().is_symlink() {
        for entry in fs::read_dir(sessions)?.take(64) {
            let entry=entry?;
            if entry.file_type()?.is_dir() {collect(user,&entry.path().join("Profiles/selected"),&format!("Session {}",entry.file_name().to_string_lossy()),&mut out)?;}
        }
    }
    out.sort_by(|a,b|b.modified.cmp(&a.modified).then_with(||a.relative.cmp(&b.relative)));
    Ok(out)
}

pub fn prepare(user:&Path,relative:&str)->io::Result<PathBuf> {
    // Accept a listed primary slot only, never a free-form host path or .bak.
    let selected=list(user)?.into_iter().find(|save|save.relative==relative)
        .ok_or_else(||io::Error::other("The selected save is no longer available. Refresh the list."))?;
    let source=user.join(&selected.relative);
    let canonical=user.canonicalize()?;
    if !source.canonicalize()?.starts_with(&canonical) {return Err(io::Error::other("Save outside the application profile"));}
    let sessions=user.join("save-sessions"); fs::create_dir_all(&sessions)?;
    if !sessions.canonicalize()?.starts_with(&canonical) {return Err(io::Error::other("Session directory outside the application profile"));}
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let session=sessions.join(format!("{stamp}-{}",selected.name));
    fs::create_dir(&session)?;
    let profile=session.join("Profiles/selected"); fs::create_dir_all(&profile)?;
    // The isolated profile has one startup slot, independent of whatever
    // last-slot index the original profile had. Original bytes stay intact.
    let destination=profile.join("SGTA50000");
    // Fresh directory, no imported or existing save is ever replaced.
    let input=fs::File::open(source)?;
    if input.metadata()?.len()!=selected.bytes {return Err(io::Error::other("Save changed while selecting it. Try again."));}
    let mut output=fs::OpenOptions::new().create_new(true).write(true).open(&destination)?;
    let copied=io::copy(&mut input.take(8*1024*1024+1),&mut output)?;
    use io::Write;
    output.flush()?; output.sync_all()?;
    if copied!=selected.bytes {return Err(io::Error::other("Incomplete save snapshot"));}
    Ok(session)
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_primary_slots_are_playable() {
        for name in ["SGTA50000","SGTA50015"] {assert!(is_slot(name));}
        for name in ["SGTA50000.bak","SGTA50000/../x","pc_settings.bin", "SGTA5000"] {assert!(!is_slot(name));}
    }
    #[test]
    fn selecting_a_save_makes_an_independent_snapshot() {
        let user=std::env::temp_dir().join(format!("glue5-save-selection-{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let profile=user.join("Documents/Rockstar Games/GTA V/Profiles/test"); fs::create_dir_all(&profile).unwrap();
        let original=profile.join("SGTA50003"); fs::write(&original,b"synthetic save").unwrap();
        let listed=list(&user).unwrap(); assert_eq!(listed.len(),1);
        let session=prepare(&user,&listed[0].relative).unwrap();
        let copy=session.join("Profiles/selected/SGTA50000"); assert_eq!(fs::read(&copy).unwrap(),b"synthetic save");
        fs::write(copy,b"new progress").unwrap(); assert_eq!(fs::read(original).unwrap(),b"synthetic save");
        assert!(prepare(&user,"../SGTA50003").is_err());
        fs::remove_dir_all(user).unwrap();
    }
}
