//! Host-approved diagnostic control. Transport authentication lives in Studio.
pub mod session;
pub use session::{Pad,Request};
pub use session::{Step, media};
use std::{sync::{Mutex,OnceLock,atomic::{AtomicBool,Ordering}},time::Instant};
static CAPTURE:AtomicBool=AtomicBool::new(false);
static FRAME_IN_FLIGHT:AtomicBool=AtomicBool::new(false);
fn state()->&'static Mutex<session::Control>{static STATE:OnceLock<Mutex<session::Control>>=OnceLock::new();STATE.get_or_init(Default::default)}
pub fn available(ready:bool){state().lock().unwrap().availability(ready,Instant::now());}
pub fn enable(enabled: bool) { state().lock().unwrap().enable(enabled); }
pub fn enabled() -> bool { state().lock().unwrap().enabled }
pub fn request(owner:&str)->Result<u64,String>{
    let mut bytes=[0u8;8];getrandom::fill(&mut bytes).map_err(|_|"cannot generate session ID")?;
    let id=(u64::from_le_bytes(bytes)&((1u64<<53)-1)).max(1);
    state().lock().unwrap().request(id,owner,Instant::now())?;Ok(id)
}
pub fn pending()->Option<Request>{let mut s=state().lock().unwrap();s.tick(Instant::now());s.pending.clone()}
pub fn approve(id:u64)->bool{state().lock().unwrap().approve(id,Instant::now())}
pub fn reject(id:u64){state().lock().unwrap().reject(id)}
pub fn heartbeat(id:u64,owner:&str)->Result<(),String>{state().lock().unwrap().heartbeat(id,owner,Instant::now())}
pub fn command(id:u64,owner:&str,pad:Pad,ms:u64)->Result<(),String>{state().lock().unwrap().command(id,owner,pad,ms,Instant::now())}
pub fn sequence(id:u64,owner:&str,steps:&[Step])->Result<(),String>{state().lock().unwrap().sequence(id,owner,steps,Instant::now())}
pub fn context(id:u64,owner:&str)->Result<serde_json::Value,String>{
    let mut s=state().lock().unwrap();s.authorize(id,owner,Instant::now())?;
    let mut v=crate::input::diagnostic_context(s.generation);
    // Engine pad constants, not inferred semantic actions (bindings vary by context).
    use crate::input::button as b;
    v["buttons"]=serde_json::json!({"south":b::SOUTH,"east":b::EAST,"west":b::WEST,"north":b::NORTH,
        "l1":b::L1,"r1":b::R1,"l3":b::L3,"r3":b::R3,"select":b::SELECT,"start":b::START,
        "up":b::UP,"right":b::RIGHT,"down":b::DOWN,"left":b::LEFT});
    if v["available"]==true{
        v["prompts"]=serde_json::json!(crate::prompts::prompts().into_iter().take(32).map(|p|{
            let pads:Vec<_>=p.pads.into_iter().map(|p|match p{
                crate::prompts::Pad::Buttons(bits)=>serde_json::json!({"buttons":bits}),
                crate::prompts::Pad::Stick{right,x,y}=>serde_json::json!({"stick":if right{"right"}else{"left"},"x":x,"y":y})}).collect();
            serde_json::json!({"input":p.input,"label":p.label.chars().take(256).collect::<String>(),"pads":pads})
        }).collect::<Vec<_>>());
    }
    Ok(v)
}
pub fn capture(id:u64,owner:&str,audio:bool)->Result<u64,String>{
    let now=Instant::now();let mut s=state().lock().unwrap();s.authorize(id,owner,now)?;
    let ticket=s.media.request(audio,now)?;CAPTURE.store(true,Ordering::Release);Ok(ticket)
}
/// Nonblocking on the render/audio paths. No work besides an atomic read when idle.
pub fn take_capture_frame()->Option<u64>{
    if !CAPTURE.load(Ordering::Acquire){return None;}
    let mut s=state().try_lock().ok()?;let now=Instant::now();s.tick(now);s.media.expire(now);
    CAPTURE.store(s.media.frame.is_some()||s.media.audio.is_some(),Ordering::Release);
    if FRAME_IN_FLIGHT.load(Ordering::Acquire){return None;}
    let ticket=s.media.take_frame(now)?;FRAME_IN_FLIGHT.store(true,Ordering::Release);Some(ticket)
}
pub fn finish_capture_frame(ticket:u64,result:Result<Vec<u8>,String>){
    let mut s=state().lock().unwrap();let now=Instant::now();s.tick(now);s.media.finish_frame(ticket,result,now);
    FRAME_IN_FLIGHT.store(false,Ordering::Release);
}
pub fn capture_audio(samples:&[f32]){
    if !CAPTURE.load(Ordering::Acquire){return;}
    let Ok(mut s)=state().try_lock() else{return;};let now=Instant::now();s.tick(now);s.media.push_audio(samples,now);
    CAPTURE.store(s.media.frame.is_some()||s.media.audio.is_some(),Ordering::Release);
}
pub struct Observation{pub metadata:serde_json::Value,pub image:Option<Vec<u8>>,pub audio:Option<Vec<u8>>}
pub fn observe(id:u64,owner:&str)->Result<Observation,String>{
    let now=Instant::now();let mut s=state().lock().unwrap();s.authorize(id,owner,now)?;s.media.expire(now);
    let mut image=None;let mut audio=None;let mut error=None;
    if s.media.frame.as_ref().is_some_and(|f|matches!(f.value,media::Frame::Ready(_)|media::Frame::Failed(_))){
        match s.media.frame.take().unwrap().value{media::Frame::Ready(v)=>image=Some(v),media::Frame::Failed(e)=>error=Some(e),_=>{}}
    }
    if s.media.audio.as_ref().is_some_and(|a|a.value.len()==media::AUDIO_SAMPLES){audio=Some(media::wav(&s.media.audio.take().unwrap().value));}
    let metadata=serde_json::json!({"image_pending":s.media.frame.is_some(),"audio_pending":s.media.audio.is_some(),"image_error":error,
        "image_source":"game_backbuffer_before_glue_overlay","audio_source":"game_mixer_16000hz_mono","retention_ms":3000});
    CAPTURE.store(s.media.frame.is_some()||s.media.audio.is_some(),Ordering::Release);
    Ok(Observation{metadata,image,audio})
}
pub fn stop_owned(id:u64,owner:&str)->Result<(),String>{state().lock().unwrap().stop_owned(id,owner)}
pub fn stop(){state().lock().unwrap().stop()}
pub fn frame()->Option<Pad>{state().lock().unwrap().frame(Instant::now())}
pub fn active()->bool{frame().is_some()}
pub fn generation()->u64{let mut s=state().lock().unwrap();s.tick(Instant::now());s.generation}
pub fn owner()->Option<String>{let mut s=state().lock().unwrap();s.tick(Instant::now());s.owner().map(str::to_owned)}
pub fn take_pause()->bool{std::mem::take(&mut state().lock().unwrap().pause_pending)}
pub fn status(owner:&str)->serde_json::Value{
    let mut s=state().lock().unwrap();s.tick(Instant::now());
    let mine=s.owner()==Some(owner);
    serde_json::json!({"version":2,"capabilities":["pad","sequence","context","frame","audio","observe"],"enabled":s.enabled,"ready":s.enabled && s.ready,"state":if s.active_id().is_some(){"active"}else if s.pending.is_some(){"awaiting_local_consent"}else{"idle"},
        "session_id":if mine{s.active_id().or_else(||s.pending.as_ref().map(|p|p.id))}else{None},
        "lease_ms":5000,"max_action_ms":session::MAX_ACTION_MS})
}
