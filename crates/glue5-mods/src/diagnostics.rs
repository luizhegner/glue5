//! Curated READ-ONLY natives, on the game's script thread. No remote Lua/code.
use std::{cell::RefCell,collections::HashMap,sync::LazyLock,time::{Duration,Instant}};
use mlua::{Lua,MultiValue,Value};
use serde_json::{json,Value as Json};
use glue5_game::Env;
use glue5_runtime::diagnostic_control as control;
use crate::natives;

static READERS:LazyLock<HashMap<&'static str,usize>>=LazyLock::new(||natives::NATIVES.iter().enumerate()
    .filter(|(_,n)|n.name.starts_with("GET_")||n.name.starts_with("IS_")||n.name.starts_with("DOES_")||n.name=="PLAYER_PED_ID")
    .map(|(i,n)|(n.name,i)).collect());
thread_local!{static LUA:RefCell<Option<Lua>>=const{RefCell::new(None)};}
struct Reader<'a>{lua:&'a Lua,env:*mut Env,start:Instant}
impl Reader<'_>{
    fn call(&self,name:&str,args:Vec<Value>)->Option<MultiValue>{
        if self.start.elapsed()>Duration::from_millis(2){return None;}
        natives::call(self.lua,*READERS.get(name)?,MultiValue::from_vec(args)).ok()
    }
    fn one(&self,name:&str,args:Vec<Value>)->Value{self.call(name,args).and_then(|v|v.front().cloned()).unwrap_or(Value::Nil)}
    fn int(&self,name:&str,args:Vec<Value>)->Option<i32>{match self.one(name,args){Value::Integer(n)=>Some(n as i32),_=>None}}
    fn num(&self,name:&str,args:Vec<Value>)->Option<f32>{match self.one(name,args){Value::Number(n)if n.is_finite()=>Some(n as f32),Value::Integer(n)=>Some(n as f32),_=>None}}
    fn boolean(&self,name:&str,args:Vec<Value>)->Option<bool>{match self.one(name,args){Value::Boolean(v)=>Some(v),_=>None}}
    fn text(&self,name:&str,args:Vec<Value>)->Option<String>{match self.one(name,args){Value::String(s)=>Some(s.to_string_lossy().chars().take(128).collect()),_=>None}}
    fn position(&self,name:&str,args:Vec<Value>)->Option<[f32;3]>{match self.one(name,args){Value::Vector(v)=>{let p=[v.x(),v.y(),v.z()];p.iter().all(|v|v.is_finite()).then_some(p)},_=>None}}
    fn coords(&self,entity:i32)->Option<[f32;3]>{self.position("GET_ENTITY_COORDS",vec![i(entity),Value::Boolean(true)])}
}
fn i(v:i32)->Value{Value::Integer(v.into())}
fn n(v:f32)->Value{Value::Number(v.into())}
fn vec3(p:[f32;3])->Value{Value::Vector(mlua::Vector::new(p[0],p[1],p[2]))}
fn sample(r:&Reader<'_>)->Json{
    let Some(ped)=r.int("PLAYER_PED_ID",vec![]).filter(|p|*p!=0)else{return json!({"available":false,"reason":"no player ped"});};
    let Some(pos)=r.coords(ped)else{return json!({"available":false,"reason":"position unavailable"});};
    let heading=r.num("GET_ENTITY_HEADING",vec![i(ped)]);
    let camera=r.position("GET_GAMEPLAY_CAM_ROT",vec![i(2)]);
    let zone=r.text("GET_NAME_OF_ZONE",vec![vec3(pos)]);
    let region=zone.as_ref().and_then(|z|r.lua.create_string(z).ok()).and_then(|s|r.text("GET_FILENAME_FOR_AUDIO_CONVERSATION",vec![Value::String(s)]));
    let streets=r.call("GET_STREET_NAME_AT_COORD",pos.into_iter().map(n).collect());
    let street=streets.as_ref().and_then(|v|v.front()).cloned().and_then(|hash|r.text("GET_STREET_NAME_FROM_HASH_KEY",vec![hash]));
    let car=r.int("GET_VEHICLE_PED_IS_IN",vec![i(ped),Value::Boolean(false)]).filter(|v|*v!=0);
    let driver=car.and_then(|car|r.int("GET_PED_IN_VEHICLE_SEAT",vec![i(car),i(-1),Value::Boolean(false)]));
    let seat=car.map(|_|if driver==Some(ped){"driver"}else if driver.is_some(){"passenger"}else{"unknown"});
    let enter=json!({"input":23,"pressed":r.boolean("IS_CONTROL_PRESSED",vec![i(0),i(23)]),
        "just_pressed":r.boolean("IS_CONTROL_JUST_PRESSED",vec![i(0),i(23)])});
    let mut taxis=Vec::new();
    // Existing pool filter makes at most 4 handles, only actual taxi models near us.
    let handles=if r.start.elapsed()<Duration::from_millis(2){unsafe{
        crate::world::handles(r.env,crate::world::Pool::Vehicles,&crate::world::Filter{max:4,near:Some((pos,120.0)),model:Some(natives::joaat("taxi"))})
    }}else{Vec::new()};
    for handle in handles{
        let Some(at)=r.coords(handle)else{continue;};
        let relative=heading.and_then(|h|control::spatial::relative(pos,at,h));
        let driver=r.int("GET_PED_IN_VEHICLE_SEAT",vec![i(handle),i(-1),Value::Boolean(false)]);
        taxis.push(json!({"handle":handle,"position":at,"distance_m":relative.map(|v|v.0),"bearing_relative_deg":relative.map(|v|v.1),
            "speed_mps":r.num("GET_ENTITY_SPEED",vec![i(handle)]),"has_driver":driver.map(|p|p!=0),
            "is_player_vehicle":car==Some(handle),"dispatch_confirmed":null}));
    }
    let mut blips=Vec::new();
    // Actual taxi entity blips, not every yellow icon inferred to be a taxi.
    for taxi in &taxis{
        let handle=taxi["handle"].as_i64().unwrap() as i32;
        if let Some(blip)=r.int("GET_BLIP_FROM_ENTITY",vec![i(handle)]).filter(|v|*v!=0){
            blips.push(json!({"handle":blip,"entity":handle,"kind":"nearby_taxi","sprite":r.int("GET_BLIP_SPRITE",vec![i(blip)]),"colour":r.int("GET_BLIP_COLOUR",vec![i(blip)]),"position":taxi["position"]}));
        }
    }
    let waypoint=r.int("GET_WAYPOINT_BLIP_ENUM_ID",vec![]).and_then(|kind|r.int("GET_FIRST_BLIP_INFO_ID",vec![i(kind)]))
        .filter(|b|*b!=0).and_then(|b|r.position("GET_BLIP_INFO_ID_COORD",vec![i(b)]));
    json!({"available":true,"source":"curated_read_only_game_natives","position":pos,"heading_deg":heading,"camera_rotation_deg":camera,
        "zone_code":zone,"region":region,"street":street,"vehicle_handle":car,"vehicle_seat":seat,
        "waypoint":waypoint,"nearby_taxis":taxis,"map_blips":blips,"enter_control":enter,"scope":"nearby taxi blips and waypoint, not all map markers",
        "budget_exhausted":r.start.elapsed()>Duration::from_millis(2)})
}
/// # Safety
/// Inside a valid engine script update; never called from a network worker.
pub(crate) unsafe fn update(env:*mut Env){
    if !control::active()||natives::TAINTED.load(std::sync::atomic::Ordering::Relaxed){return;}
    if let Some(ms)=unsafe{natives::call_int(env,"GET_GAME_TIMER")}{control::game_clock(ms as u32);}
    let Some(generation)=control::world_sample_due()else{return;};
    let value=natives::with_env(env,||LUA.with_borrow_mut(|slot|{
        if slot.is_none(){let lua=Lua::new();let _=lua.set_memory_limit(2*1024*1024);*slot=Some(lua);}
        sample(&Reader{lua:slot.as_ref().unwrap(),env,start:Instant::now()})
    }));
    control::publish_world(generation,value);
}
#[cfg(test)] mod tests{
    use super::*;
    #[test] fn sampler_does_not_expose_mutating_natives(){
        for name in ["PLAYER_PED_ID","GET_ENTITY_COORDS","GET_ENTITY_HEADING","GET_GAME_TIMER"]{assert!(READERS.contains_key(name),"missing required native {name}");}
        for name in ["TASK_ENTER_VEHICLE","SET_ENTITY_COORDS","CREATE_VEHICLE","SET_NEW_WAYPOINT"]{assert!(!READERS.contains_key(name));}
    }
}
