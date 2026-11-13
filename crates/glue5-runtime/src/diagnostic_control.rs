//! Host-approved diagnostic control. Transport authentication lives in Studio.
pub mod session;
pub mod spatial;
pub use session::{Pad,Request};
pub use session::{Step, media};
use std::{sync::{Mutex,OnceLock,atomic::{AtomicBool,Ordering}},time::Instant};
static CAPTURE:AtomicBool=AtomicBool::new(false);
static FRAME_IN_FLIGHT:AtomicBool=AtomicBool::new(false);
type World = Option<(Instant,u64,serde_json::Value)>;
fn world()->&'static Mutex<World>{static WORLD:OnceLock<Mutex<World>>=OnceLock::new();WORLD.get_or_init(Default::default)}
/// A sampler runs only during local consent; late publication cannot cross grants.
pub fn world_sample_due()->Option<u64>{
    let mut s=state().lock().unwrap();s.tick(Instant::now());s.active_id()?;
    let generation=s.generation;
    let due=world().lock().unwrap().as_ref().is_none_or(|(at,g,_)|*g!=generation||at.elapsed().as_millis()>=250);
    due.then_some(generation)
}
pub fn publish_world(generation:u64,value:serde_json::Value){
    let mut s=state().lock().unwrap();s.tick(Instant::now());
    if s.active_id().is_some()&&s.generation==generation{
        let now=Instant::now();
        let taxis=value["nearby_taxis"].as_array().is_some_and(|ts|ts.iter().any(|t|t["is_player_vehicle"]==false&&t["has_driver"]==true
            &&t["distance_m"].as_f64().is_some_and(|d|d<=8.0)&&t["speed_mps"].as_f64().is_some_and(|v|v<=0.5)));
        s.react(session::Condition::TaxiNear,taxis,now);
        s.react(session::Condition::Passenger,value["vehicle_seat"]=="passenger",now);
        s.react(session::Condition::Vehicle,value["vehicle_handle"].as_i64().is_some(),now);
        *world().lock().unwrap()=Some((now,generation,value));
    }
}
pub fn arm(id:u64,owner:&str,condition:session::Condition,pad:Pad,ms:u64,timeout:u64)->Result<(),String>{
    state().lock().unwrap().arm(id,owner,condition,pad,ms,timeout,Instant::now())
}
fn state()->&'static Mutex<session::Control>{static STATE:OnceLock<Mutex<session::Control>>=OnceLock::new();STATE.get_or_init(Default::default)}
pub fn available(ready:bool){state().lock().unwrap().availability(ready,Instant::now());}

/// Compatibility fallbacks observed in this process. Entries are clues to
/// investigate, not a confirmed inventory of absent or compatible assets.
pub fn missing_asset_diagnostics()->Vec<String> {
    crate::script_assets::missing_asset_diagnostics()
}
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
pub fn hold(id:u64,owner:&str,pad:Pad,ms:u64)->Result<(),String>{state().lock().unwrap().hold(id,owner,pad,ms,Instant::now())}
pub fn game_clock(ms:u32){state().lock().unwrap().game_clock(ms,Instant::now());}
pub fn consume_pad()->Option<Pad>{state().lock().unwrap().consume(Instant::now())}
pub fn input_receipt(id:u64,owner:&str)->Result<serde_json::Value,String>{
    let mut s=state().lock().unwrap();s.authorize(id,owner,Instant::now())?;let r=s.receipt;
    Ok(serde_json::json!({"revision":r.revision,"applied_frames":r.applied_frames,"non_neutral_frames":r.non_neutral_frames,"last_buttons":r.last_buttons,"reaction":s.reaction_state,
        "meaning":"host pad reads, not proof that a game action succeeded"}))
}
pub fn prompt(id:u64,owner:&str,input:u32,ms:u64)->Result<(),String>{
    let mut s=state().lock().unwrap();let now=Instant::now();s.authorize(id,owner,now)?;
    let c=crate::input::diagnostic_context(s.generation);
    if c["available"]!=true||c["loading_or_faded"]==true||!crate::prompts::prompts().iter().any(|p|p.input==Some(input)){
        return Err("input must be a currently observed prompt in a fresh game context".into());
    }
    s.prompt(id,owner,input,ms,now)
}
pub(crate) fn prompt_input(visible:&[u32])->Option<u32>{state().lock().unwrap().prompt_input(visible,Instant::now())}
pub fn context(id:u64,owner:&str)->Result<serde_json::Value,String>{
    let mut s=state().lock().unwrap();s.authorize(id,owner,Instant::now())?;
    let mut v=crate::input::diagnostic_context(s.generation);
    if let Some((at,_,data))=world().lock().unwrap().as_ref().filter(|(at,g,_)|*g==s.generation&&at.elapsed().as_secs_f32()<1.0){
        v["world"]=data.clone();v["world_age_ms"]=serde_json::json!(at.elapsed().as_millis() as u64);
        if data["available"]==true&&v["loading_or_faded"]!=true{
            v["position"]=data["position"].clone();v["heading_deg"]=data["heading_deg"].clone();v["location_name"]=data["region"].clone();
        }
    }else{v["world"]=serde_json::json!({"available":false,"reason":"native script sampler unavailable or stale"});}
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
pub fn watch(id:u64,owner:&str,ms:u64)->Result<(),String>{
    let now=Instant::now();let mut s=state().lock().unwrap();s.authorize(id,owner,now)?;s.media.watch(ms,now)?;
    CAPTURE.store(true,Ordering::Release);Ok(())
}
/// Nonblocking on the render/audio paths. No work besides an atomic read when idle.
pub fn take_capture_frame()->Option<u64>{
    if !CAPTURE.load(Ordering::Acquire){return None;}
    let mut s=state().try_lock().ok()?;let now=Instant::now();s.tick(now);s.media.expire(now);
    CAPTURE.store(s.media.wanted(),Ordering::Release);
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
    CAPTURE.store(s.media.wanted(),Ordering::Release);
}
pub struct Observation{pub metadata:serde_json::Value,pub image:Option<Vec<u8>>,pub audio:Option<Vec<u8>>}
pub fn image_ready(id:u64,owner:&str)->Result<bool,String>{
    let mut s=state().lock().unwrap();s.authorize(id,owner,Instant::now())?;
    Ok(s.media.frame.as_ref().is_some_and(|f|matches!(f.value,media::Frame::Ready(_)|media::Frame::Failed(_))))
}
pub fn observe(id:u64,owner:&str)->Result<Observation,String>{
    let now=Instant::now();let mut s=state().lock().unwrap();s.authorize(id,owner,now)?;s.media.expire(now);
    let timing=|until:Instant|now.saturating_duration_since(until-media::TTL).as_millis() as u64;
    let image_ticket=s.media.frame.as_ref().map(|f|f.ticket);let image_age=s.media.frame.as_ref().map(|f|timing(f.until));
    let audio_ticket=s.media.audio.as_ref().map(|f|f.ticket);let audio_age=s.media.audio.as_ref().map(|f|timing(f.until));
    let mut image=None;let mut audio=None;let mut error=None;
    if s.media.frame.as_ref().is_some_and(|f|matches!(f.value,media::Frame::Ready(_)|media::Frame::Failed(_))){
        match s.media.frame.take().unwrap().value{media::Frame::Ready(v)=>image=Some(v),media::Frame::Failed(e)=>error=Some(e),_=>{}}
    }
    if s.media.audio.as_ref().is_some_and(|a|a.value.len()==media::AUDIO_SAMPLES){audio=Some(media::wav(&s.media.audio.take().unwrap().value));}
    let metadata=serde_json::json!({"image_pending":s.media.frame.is_some(),"audio_pending":s.media.audio.is_some(),"image_error":error,
        "image_ticket":image_ticket,"audio_ticket":audio_ticket,"image_request_age_ms":image_age,"audio_request_age_ms":audio_age,
        "image_source":"game_backbuffer_before_glue_overlay","audio_source":"game_mixer_16000hz_mono","retention_ms":3000});
    CAPTURE.store(s.media.wanted(),Ordering::Release);
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
    serde_json::json!({"version":3,"capabilities":["pad","hold","react","input","step","watch","sequence","prompt","context","frame","audio","observe"],"enabled":s.enabled,"ready":s.enabled && s.ready,"state":if s.active_id().is_some(){"active"}else if s.pending.is_some(){"awaiting_local_consent"}else{"idle"},
        "session_id":if mine{s.active_id().or_else(||s.pending.as_ref().map(|p|p.id))}else{None},
        "lease_ms":5000,"max_action_ms":session::MAX_ACTION_MS,"max_hold_game_ms":session::MAX_HOLD_MS,"max_sequence_ms":session::MAX_SEQUENCE_MS,"max_steps":session::MAX_STEPS})
}
