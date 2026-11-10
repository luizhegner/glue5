//! Exclusive input ownership: explicit local consent, expiring lease, no queues.
//! Pure policy, also tested by the lightweight CI harness without the game.
use std::time::{Duration,Instant};
pub const LEASE: Duration = Duration::from_secs(5);
pub const CONSENT_WAIT: Duration = Duration::from_secs(30);
pub const MAX_ACTION_MS: u64 = 1000;
#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub struct Pad {
    pub buttons:u16, pub left:[f32;2], pub right:[f32;2], pub l2:f32, pub r2:f32,
}
impl Pad {
    pub fn valid(self)->bool {
        self.left.into_iter().chain(self.right).all(|v|v.is_finite()&&(-1.0..=1.0).contains(&v))
            && [self.l2,self.r2].into_iter().all(|v|v.is_finite()&&(0.0..=1.0).contains(&v))
    }
}
#[derive(Clone,Debug)]
pub struct Request { pub id:u64,pub owner:String,pub until:Instant }
struct Active { request:Request,command:Pad,command_until:Instant }
#[derive(Default)]
pub struct Control {
    pub enabled: bool,
    pub ready:bool,
    pub pending:Option<Request>,
    active:Option<Active>,
    pub generation:u64,
    pub pause_pending:bool,
}
impl Control {
    /// Local opt-in lasts only for this app run; it is not consent to control.
    pub fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled { self.stop(); }
    }
    pub fn tick(&mut self,now:Instant) {
        if self.pending.as_ref().is_some_and(|p|now>=p.until){self.pending=None;}
        if self.active.as_ref().is_some_and(|a|now>=a.request.until){self.stop();}
    }
    pub fn availability(&mut self,ready:bool,now:Instant) {
        self.tick(now);self.ready=ready;
        if !ready {self.stop();}
    }
    pub fn request(&mut self,id:u64,owner:&str,now:Instant)->Result<(),String> {
        self.tick(now);
        if !self.enabled {return Err("enable AI diagnostics on the phone first".into());}
        if !self.ready {return Err("game must be running in the foreground without another menu".into());}
        if self.pending.is_some()||self.active.is_some(){return Err("another request or control session exists".into());}
        self.pending=Some(Request{id,owner:owner.to_owned(),until:now+CONSENT_WAIT});Ok(())
    }
    pub fn approve(&mut self,id:u64,now:Instant)->bool {
        self.tick(now);
        if !self.enabled||!self.ready||self.pending.as_ref().is_none_or(|r|r.id!=id){return false;}
        let mut request=self.pending.take().unwrap();request.until=now+LEASE;
        self.active=Some(Active{request,command:Pad::default(),command_until:now});
        self.generation=self.generation.wrapping_add(1);true
    }
    pub fn reject(&mut self,id:u64) {
        if self.pending.as_ref().is_some_and(|r|r.id==id){self.pending=None;}
    }
    pub fn heartbeat(&mut self,id:u64,owner:&str,now:Instant)->Result<(),String> {
        self.tick(now);
        let a=self.active.as_mut().filter(|a|a.request.id==id&&a.request.owner==owner).ok_or("no approved session for this client")?;
        a.request.until=now+LEASE;Ok(())
    }
    pub fn command(&mut self,id:u64,owner:&str,pad:Pad,ms:u64,now:Instant)->Result<(),String> {
        self.tick(now);
        if !pad.valid()||ms==0||ms>MAX_ACTION_MS{return Err("invalid pad state or duration (1..1000 ms)".into());}
        let a=self.active.as_mut().filter(|a|a.request.id==id&&a.request.owner==owner).ok_or("no approved session for this client")?;
        a.command=pad;a.command_until=now+Duration::from_millis(ms);Ok(())
    }
    pub fn frame(&mut self,now:Instant)->Option<Pad> {
        self.tick(now);
        self.active.as_ref().map(|a|if now<a.command_until{a.command}else{Pad::default()})
    }
    pub fn owner(&self)->Option<&str>{self.active.as_ref().map(|a|a.request.owner.as_str()).or_else(||self.pending.as_ref().map(|p|p.owner.as_str()))}
    pub fn active_id(&self)->Option<u64>{self.active.as_ref().map(|a|a.request.id)}
    pub fn stop_owned(&mut self,id:u64,owner:&str)->Result<(),String>{
        if self.active.as_ref().is_some_and(|a|a.request.id==id&&a.request.owner==owner)
            || self.pending.as_ref().is_some_and(|p|p.id==id&&p.owner==owner) {
            self.stop();Ok(())
        }else{Err("session does not belong to this client".into())}
    }
    pub fn stop(&mut self) {
        self.pending=None;
        if self.active.take().is_some(){self.generation=self.generation.wrapping_add(1);self.pause_pending=true;}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ready(t:Instant)->Control{let mut c=Control::default();c.enable(true);c.availability(true,t);c}
    fn active(t:Instant)->Control{let mut c=ready(t);c.request(7,"agent",t).unwrap();assert!(c.approve(7,t));c}
    #[test] fn pairing_request_is_not_consent(){
        let t=Instant::now();let mut c=ready(t);c.request(7,"agent",t).unwrap();
        assert_eq!(c.frame(t),None);assert!(c.heartbeat(7,"agent",t).is_err());
        assert!(!c.approve(8,t));assert!(c.approve(7,t));assert_eq!(c.frame(t),Some(Pad::default()));
    }
    #[test] fn reject_and_expired_consent_never_acquire(){
        let t=Instant::now();let mut c=ready(t);c.request(7,"agent",t).unwrap();c.reject(7);assert!(!c.approve(7,t));
        c.request(8,"agent",t).unwrap();assert!(!c.approve(8,t+CONSENT_WAIT));assert_eq!(c.frame(t+CONSENT_WAIT),None);
    }
    #[test] fn action_expires_even_with_healthy_connection(){
        let t=Instant::now();let mut c=active(t);let p=Pad{r2:1.0,left:[1.0,0.0],..Default::default()};
        c.command(7,"agent",p,100,t).unwrap();assert_eq!(c.frame(t),Some(p));
        c.heartbeat(7,"agent",t+Duration::from_secs(1)).unwrap();assert_eq!(c.frame(t+Duration::from_secs(1)),Some(Pad::default()));
    }
    #[test] fn lost_lease_releases_and_requires_fresh_consent(){
        let t=Instant::now();let mut c=active(t);assert_eq!(c.frame(t+LEASE),None);assert!(c.pause_pending);
        assert!(c.heartbeat(7,"agent",t+LEASE).is_err());assert!(c.command(7,"agent",Pad::default(),1,t+LEASE).is_err());
        assert!(!c.approve(7,t+LEASE));
    }
    #[test] fn stop_and_background_clear_actions_and_request_pause(){
        for background in [false,true]{let t=Instant::now();let mut c=active(t);
            c.command(7,"agent",Pad{buttons:0xffff,..Default::default()},1000,t).unwrap();
            if background {c.availability(false,t);}else{c.stop();}
            assert_eq!(c.frame(t),None);assert!(c.pause_pending);assert!(c.pending.is_none());
        }
    }
    #[test] fn foreign_owner_and_invalid_commands_cannot_take_over(){
        let t=Instant::now();let mut c=active(t);
        assert!(c.request(9,"other",t).is_err());assert!(c.command(7,"other",Pad::default(),1,t).is_err());
        assert!(c.stop_owned(7,"other").is_err());assert!(c.heartbeat(8,"agent",t).is_err());
        for ms in [0,1001]{assert!(c.command(7,"agent",Pad::default(),ms,t).is_err());}
        assert!(c.command(7,"agent",Pad{l2:f32::NAN,..Default::default()},1,t).is_err());
    }

    #[test] fn losing_foreground_invalidates_pending_consent_too() {
        let t = Instant::now();
        let mut c = ready(t);
        c.request(7, "agent", t).unwrap();
        c.availability(false, t);
        c.availability(true, t);
        assert!(!c.approve(7, t));
        assert_eq!(c.frame(t), None);
        assert!(!c.pause_pending); // No ownership was granted.
    }

    #[test] fn previous_session_commands_cannot_affect_a_new_grant() {
        let t = Instant::now();
        let mut c = active(t);
        c.command(7, "agent", Pad { r2: 1.0, ..Default::default() }, 1000, t).unwrap();
        c.stop();
        c.request(8, "agent", t).unwrap();
        assert!(c.approve(8, t));
        assert!(c.command(7, "agent", Pad::default(), 1000, t).is_err());
        assert!(c.heartbeat(7, "agent", t).is_err());
        assert!(c.stop_owned(7, "agent").is_err());
        assert_eq!(c.active_id(), Some(8));
        assert_eq!(c.frame(t), Some(Pad::default()));
    }

    #[test] fn opt_in_defaults_off_and_disabling_cancels_ownership() {
        let t = Instant::now();
        let mut c = Control::default();
        c.availability(true, t);
        assert!(!c.enabled);
        assert!(c.request(7, "agent", t).is_err());
        c.enable(true);
        c.request(7, "agent", t).unwrap();
        assert_eq!(c.frame(t), None); // Enabling is never consent.
        assert!(c.approve(7, t));
        c.enable(false);
        assert_eq!(c.frame(t), None);
        assert!(c.pause_pending);
        c.enable(true);
        assert!(!c.approve(7, t));
        assert!(c.heartbeat(7, "agent", t).is_err());
    }
}
