//! Import UI sounds from the selected Web build on a background thread.
//! A server source is read through its range API, never as a sparse file.
use std::{fs::{self,File},io::{self,Read,Seek,SeekFrom,Write},path::{Path,PathBuf},sync::{Arc,mpsc},time::{SystemTime,UNIX_EPOCH}};
use glue5_resource_api::{Archive,awc,rel};
use crate::sfx::Sound;

const REL_PACK:&str="data/x64/audio/audio_rel.rpf";
const SFX_PACK:&str="data/x64/audio/sfx/RESIDENT.rpf";
const LIMIT:u64=32*1024*1024;
const SET:&str="HUD_FRONTEND_DEFAULT_SOUNDSET";

#[derive(Clone,Debug,PartialEq,Eq)]
struct Identity { root:PathBuf, remote:Option<(String,usize)> }

#[derive(Clone)]
struct Source { identity:Identity, client:Option<Arc<glue5_stream::Client>>, control:Arc<glue5_stream::TransferControl> }
impl Source {
    fn reader(&self,path:&str)->Result<(Reader,u64),String> {
        if let Some(client)=&self.client {
            let id=client.find(path).ok_or_else(||format!("game catalog does not contain {path}"))?;
            let size=client.file_size(id).ok_or_else(||format!("file size unavailable for {path}"))?;
            Ok((Reader::Remote{client:client.clone(),control:self.control.clone(),id,size,at:0},size))
        } else {
            let file=File::open(self.identity.root.join(path)).map_err(|e|format!("opening {path}: {e}"))?;
            let size=file.metadata().map_err(|e|e.to_string())?.len();
            Ok((Reader::Local(file),size))
        }
    }
}

enum Reader { Local(File),Remote{client:Arc<glue5_stream::Client>,control:Arc<glue5_stream::TransferControl>,id:u32,size:u64,at:u64} }
impl Read for Reader {
    fn read(&mut self,buf:&mut[u8])->io::Result<usize> {
        match self {
            Reader::Local(file)=>file.read(buf),
            Reader::Remote{client,control,id,size,at}=>{
                let n=buf.len().min(size.saturating_sub(*at) as usize);
                let read=client.read_cancellable(*id,*at,&mut buf[..n],control).ok_or_else(||io::Error::other("server read unavailable or cancelled"))?;
                *at+=read as u64;Ok(read)
            }
        }
    }
}
impl Seek for Reader {
    fn seek(&mut self,from:SeekFrom)->io::Result<u64> {
        match self {
            Reader::Local(file)=>file.seek(from),
            Reader::Remote{size,at,..}=>{
                let target=match from {SeekFrom::Start(pos)=>pos as i128,SeekFrom::End(delta)=>*size as i128+delta as i128,SeekFrom::Current(delta)=>*at as i128+delta as i128};
                if target<0 || target>u64::MAX as i128 {return Err(io::Error::new(io::ErrorKind::InvalidInput,"invalid seek"));}
                *at=target as u64;Ok(*at)
            }
        }
    }
}

fn archive(source:&Source,path:&str)->Result<Archive<Reader>,String> {
    let keys=glue5_runtime::resource_audio_keys();
    let (reader,size)=source.reader(path)?;
    match Archive::open_reader(reader,size,None) {
        Ok(pack)=>return Ok(pack),
        Err(error) if !error.contains("requires an explicitly supplied")=>return Err(error),
        Err(_)=>{},
    }
    for key in keys {
        let (reader,size)=source.reader(path)?;
        if let Ok(pack)=Archive::open_reader(reader,size,Some(key)) {return Ok(pack);}
    }
    Err(format!("could not unlock {path} using this build's validated candidates"))
}

fn entry(pack:&Archive<Reader>,name:&str)->Result<String,String> {
    let mut matches=pack.entries.iter().filter(|e|e.path.rsplit('/').next().is_some_and(|n|n.eq_ignore_ascii_case(name)));
    let found=matches.next().ok_or_else(||format!("{name} is absent from the selected build"))?;
    if matches.next().is_some() {return Err(format!("more than one {name} entry; refusing ambiguous binding"));}
    Ok(found.path.clone())
}

/// Resolve script event -> REL sound -> its original AWC container/stream.
/// The set's records can contain processing graphs; unsupported graphs are
/// reported, never silently treated as direct waveforms.
struct Imported { directory:PathBuf, events:usize, files:usize }
fn import(source:&Source,user:&Path)->Result<Imported,String> {
    let mut rel_pack=archive(source,REL_PACK)?;
    let rel_path=entry(&rel_pack,"sounds.dat54.rel").or_else(|_|entry(&rel_pack,"sounds.dat54"))?;
    let rel_bytes=rel_pack.read(&rel_path,LIMIT)?;
    let sounds=rel::Sounds::open(&rel_bytes)?;
    let mut sfx_pack=archive(source,SFX_PACK)?;
    let bank_path=entry(&sfx_pack,"frontend.awc")?;
    let bank_bytes=sfx_pack.read(&bank_path,LIMIT)?;
    let bank=awc::Bank::open(&bank_bytes)?;
    let identity=rel::name_hash(&rel::bank_name(&format!("{SFX_PACK}::{bank_path}"))?)?;
    let mut clips=Vec::new();
    let mut events=0;
    for (event,aliases) in [
        ("NAV_UP_DOWN",&[Sound::Focus,Sound::ToggleOn,Sound::ToggleOff][..]),
        ("SELECT",&[Sound::Confirm,Sound::Launch,Sound::Notify][..]),
        ("BACK",&[Sound::Back,Sound::Close,Sound::Error][..]),
        ("NAV_LEFT_RIGHT",&[Sound::Tab,Sound::Adjust,Sound::Open][..]),
    ] {
        let reference=match sounds.resolve(SET,event) {
            Ok(reference) if reference.container_hash==identity=>reference,
            Ok(_)=>{log::warn!("menu audio: {event} belongs to a different bank");continue;},
            Err(error)=>{log::warn!("menu audio: cannot resolve {event}: {error}");continue;},
        };
        let Some(wave)=bank.streams.iter().find(|s|s.id==reference.stream_id) else {
            log::warn!("menu audio: resolved {event} waveform is missing");continue;
        };
        let decoded=if wave.codec==Some(12) {
            bank.web_opus_ogg(reference.stream_id,glue5_runtime::GAME_BUILD).map(|b|(b,"ogg"))
        } else {bank.wav(reference.stream_id).map(|b|(b,"wav"))};
        let (data,ext)=match decoded {Ok(clip)=>clip,Err(error)=>{log::warn!("menu audio: cannot decode {event}: {error}");continue;}};
        events+=1;
        for &sound in aliases {clips.push((sound,ext,data.clone()));}
    }
    if clips.is_empty() {return Err("no supported, verified menu events in this build".into());}
    let root=crate::paths::cache_dir(user).join("frontend-audio");
    fs::create_dir_all(&root).map_err(|e|e.to_string())?;
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let output=root.join(format!("{}-{stamp}",std::process::id()));
    fs::create_dir(&output).map_err(|e|e.to_string())?;
    let files=clips.len();
    for (sound,ext,bytes) in clips {
        let path=output.join(format!("{}.{ext}",sound.file()));
        let mut file=fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e|e.to_string())?;
        file.write_all(&bytes).map_err(|e|e.to_string())?;
    }
    Ok(Imported{directory:output,events,files})
}

pub enum Change { Unchanged,Reset,Ready{directory:PathBuf,events:usize,files:usize},Failed(String) }
#[derive(Default)]
pub struct Importer { identity:Option<Identity>,pending:Option<mpsc::Receiver<Result<Imported,String>>>,control:Option<Arc<glue5_stream::TransferControl>>,started:bool,failed:bool,ready:bool,partial:bool }
impl Importer {
    pub fn status(&self)->&'static str {
        if self.ready {if self.partial {"PARTIAL"} else {"READY"}}else if self.pending.is_some(){"LOADING"}else if self.failed {"UNAVAILABLE"}else{"WAITING FOR GAME"}
    }
    pub fn is_pending(&self)->bool { self.pending.is_some() }
    pub fn player_failed(&mut self){self.ready=false;self.failed=true;}
    pub fn player_ready(&mut self,loaded:usize,expected:usize,events:usize) {
        self.ready=loaded>0;self.failed=loaded==0;self.partial=events<4 || loaded<expected;
    }
    pub fn poll(&mut self,root:Option<&Path>,client:Option<&Arc<glue5_stream::Client>>,user:&Path)->Change {
        let identity=root.map(|root|Identity{root:root.to_owned(),remote:client.map(|c|(c.server().to_owned(),Arc::as_ptr(c) as usize))});
        if self.identity!=identity {
            if let Some(control)=self.control.take(){control.cancel();}
            self.identity=identity;self.pending=None;self.started=false;self.failed=false;self.ready=false;self.partial=false;
            return Change::Reset;
        }
        if let Some(pending)=&self.pending {
            match pending.try_recv() {
                Ok(result)=>{
                    self.pending=None;
                    return match result {
                        Ok(imported)=>Change::Ready{directory:imported.directory,events:imported.events,files:imported.files},
                        Err(error)=>{self.failed=true;Change::Failed(error)},
                    };
                },
                Err(mpsc::TryRecvError::Disconnected)=>{self.pending=None;self.failed=true;return Change::Failed("audio import worker stopped".into());},
                Err(mpsc::TryRecvError::Empty)=>return Change::Unchanged,
            }
        }
        if !self.started && let Some(identity)=self.identity.clone() {
            self.started=true;
            let control=Arc::new(glue5_stream::TransferControl::default());
            self.control=Some(control.clone());
            let source=Source{identity,client:client.cloned(),control};let user=user.to_owned();
            let (send,receive)=mpsc::sync_channel(1);
            match std::thread::Builder::new().name("glue5-menu-audio".into()).spawn(move||{let _=send.send(import(&source,&user));}) {
                Ok(_)=>self.pending=Some(receive),
                Err(error)=>{self.failed=true;return Change::Failed(error.to_string());},
            }
        }
        Change::Unchanged
    }
}

impl Drop for Importer {
    fn drop(&mut self) { if let Some(control)=self.control.take(){control.cancel();} }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(name:&str,payload:&[u8])->Vec<u8> {
        let mut names=vec![0];names.extend_from_slice(name.as_bytes());names.push(0);
        let mut result=b"7FPR".to_vec();
        for value in [2u32,names.len() as u32,0x4e45504f] {result.extend(value.to_le_bytes());}
        for value in [0u32,0x7fffff00,1,1] {result.extend(value.to_le_bytes());}
        let mut entry=[0u8;16];entry[0]=1;entry[5]=1;
        entry[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        result.extend(entry);result.extend(names);result.resize(512,0);result.extend(payload);result
    }

    fn rel_fixture()->Vec<u8> {
        let mut set=vec![32];set.extend(0xaaaaaaaau32.to_le_bytes());
        set.extend(1u32.to_le_bytes());set.extend(rel::name_hash("SELECT").unwrap().to_le_bytes());set.extend(42u32.to_le_bytes());
        let mut sound=vec![12];sound.extend(0xaaaaaaaau32.to_le_bytes());
        sound.extend(rel::name_hash("resident/frontend").unwrap().to_le_bytes());sound.extend(0xe1234567u32.to_le_bytes());sound.push(0);
        let mut data=1u32.to_le_bytes().to_vec();let mut index=Vec::new();
        for (name,record) in [(rel::name_hash(SET).unwrap(),set),(42,sound)] {
            index.extend(name.to_le_bytes());index.extend((data.len() as u32).to_le_bytes());
            index.extend((record.len() as u32).to_le_bytes());data.extend(record);
        }
        let mut out=54u32.to_le_bytes().to_vec();out.extend((data.len() as u32).to_le_bytes());out.extend(data);
        out.extend(4u32.to_le_bytes());out.extend(0u32.to_le_bytes());out.extend(2u32.to_le_bytes());out.extend(index);
        out.extend(0u32.to_le_bytes());out.extend(0u32.to_le_bytes());out
    }

    fn awc_fixture()->Vec<u8> {
        let mut awc=vec![0u8;70];awc[..4].copy_from_slice(b"ADAT");
        for (at,value) in [(4,0xff020001u32),(8,1),(12,48),(16,(2<<29)|0x01234567),(48,1)] {
            awc[at..at+4].copy_from_slice(&value.to_le_bytes());
        }
        awc[20..28].copy_from_slice(&((0xfau64<<56)|(20<<28)|48).to_le_bytes());
        awc[28..36].copy_from_slice(&((0x55u64<<56)|(2<<28)|68).to_le_bytes());
        awc[56..58].copy_from_slice(&48000u16.to_le_bytes());awc[67]=1;
        awc[68..70].copy_from_slice(&1234i16.to_le_bytes());awc
    }

    #[test]
    fn verified_game_events_create_playable_files_without_modifying_sources() {
        let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let temp=std::env::temp_dir().join(format!("glue5-frontend-audio-{}-{stamp}",std::process::id()));
        let game=temp.join("game");let user=temp.join("user");
        fs::create_dir_all(game.join("data/x64/audio/sfx")).unwrap();fs::create_dir_all(&user).unwrap();
        let metadata=pack("sounds.dat54.rel",&rel_fixture());let bank=pack("frontend.awc",&awc_fixture());
        fs::write(game.join(REL_PACK),&metadata).unwrap();fs::write(game.join(SFX_PACK),&bank).unwrap();
        let source=Source{identity:Identity{root:game.clone(),remote:None},client:None,control:Arc::new(glue5_stream::TransferControl::default())};
        let result=import(&source,&user).unwrap();
        assert_eq!((result.events,result.files),(1,3));
        for name in ["confirm.wav","launch.wav","notify.wav"] {
            assert!(fs::read(result.directory.join(name)).unwrap().starts_with(b"RIFF"));
        }
        assert_eq!(fs::read(game.join(REL_PACK)).unwrap(),metadata);
        assert_eq!(fs::read(game.join(SFX_PACK)).unwrap(),bank);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn manager_reports_partial_and_resets_on_source_change() {
        let mut importer=Importer::default();
        let control=Arc::new(glue5_stream::TransferControl::default());
        importer.control=Some(control.clone());
        importer.player_ready(3,3,1);assert_eq!(importer.status(),"PARTIAL");
        importer.player_ready(12,12,4);assert_eq!(importer.status(),"READY");
        assert!(matches!(importer.poll(Some(Path::new("new-game")),None,Path::new("user")),Change::Reset));
        assert!(control.is_cancelled());
        assert_eq!(importer.status(),"WAITING FOR GAME");
    }
}
