//! Host-approved diagnostic control. Transport authentication lives in Studio.
pub mod session;
pub use session::{Pad,Request};
use std::{sync::{Mutex,OnceLock},time::Instant};
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
    serde_json::json!({"version":1,"enabled":s.enabled,"ready":s.enabled && s.ready,"state":if s.active_id().is_some(){"active"}else if s.pending.is_some(){"awaiting_local_consent"}else{"idle"},
        "session_id":if mine{s.active_id().or_else(||s.pending.as_ref().map(|p|p.id))}else{None},
        "lease_ms":5000,"max_action_ms":session::MAX_ACTION_MS})
}
