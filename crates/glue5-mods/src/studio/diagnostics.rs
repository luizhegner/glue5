//! Small authenticated control endpoint, not a web panel or arbitrary executor.
use serde_json::{Value,json};
use glue5_runtime::diagnostic_control as control;
use base64::Engine;
use std::time::{Duration,Instant};
struct StepPlan {action:Option<(control::Pad,bool,u64)>,wait_ms:u64,image:bool}
fn step_plan(v:&Value)->Result<StepPlan,String>{
    let settle=v.get("settle_ms").map_or(Ok(100),|v|v.as_u64().filter(|n|*n<=1000).ok_or("settle_ms must be 0..1000"))?;
    let image=v.get("with_image").map_or(Ok(true),|v|v.as_bool().ok_or("with_image must be boolean"))?;
    let action=match v.get("action"){
        None=>None,
        Some(a)=>{
            let hold=match a["kind"].as_str(){Some("pad")=>false,Some("hold")=>true,_=>return Err("step action kind must be pad or hold".into())};
            let ms=a["duration_ms"].as_u64().filter(|ms|*ms>0&&*ms<=if hold{3000}else{1000}).ok_or("invalid step duration")?;
            Some((pad(&a["pad"])?,hold,ms))
        }
    };
    let wait_ms=action.map_or(0,|(_,_,ms)|ms)+settle;
    if wait_ms>2500{return Err("step action + settle must be <=2500ms; use standalone hold for longer actions".into());}
    Ok(StepPlan{action,wait_ms,image})
}
fn observation(o:control::Observation)->Value{
    let b64=base64::engine::general_purpose::STANDARD;
    json!({"metadata":o.metadata,"image":o.image.map(|v|json!({"mimeType":"image/png","data":b64.encode(v)})),
        "audio":o.audio.map(|v|json!({"mimeType":"audio/wav","data":b64.encode(v)}))})
}
/// One bounded RPC replaces command/sleep/frame/sleep/observe/context round trips.
/// No auto heartbeat, shell, Lua, consent bypass or inference on the phone.
fn step(owner:&str,id:u64,v:&Value)->Result<Value,String>{
    let plan=step_plan(v)?;control::context(id,owner)?;
    let start=Instant::now();
    if let Some((pad,hold,ms))=plan.action{
        if hold{control::hold(id,owner,pad,ms)?;}else{control::command(id,owner,pad,ms)?;}
    }
    while start.elapsed()<Duration::from_millis(plan.wait_ms){
        control::input_receipt(id,owner)?;std::thread::sleep(Duration::from_millis(25));
    }
    let mut image_request_error=None;
    if plan.image{
        if let Err(error)=control::capture(id,owner,false){image_request_error=Some(error);}
        let until=Instant::now()+Duration::from_millis(800);
        while Instant::now()<until{
            control::input_receipt(id,owner)?;
            if control::image_ready(id,owner)?{break;}
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let mut value=if plan.image{observation(control::observe(id,owner)?)}else{json!({"metadata":{}})};
    value["context"]=control::context(id,owner)?;value["input"]=control::input_receipt(id,owner)?;
    value["metadata"]["step_elapsed_ms"]=json!(start.elapsed().as_millis() as u64);
    value["metadata"]["image_request_error"]=json!(image_request_error);
    Ok(value)
}
fn steps(v:&Value)->Result<Vec<control::Step>,String>{
    let a=v.as_array().filter(|a|!a.is_empty()&&a.len()<=control::session::MAX_STEPS).ok_or("steps requires 1..16 entries")?;
    a.iter().map(|s|Ok(control::Step{pad:pad(&s["pad"])?,ms:s["duration_ms"].as_u64().ok_or("step duration_ms required")?})).collect()
}
fn pad(v:&Value)->Result<control::Pad,String>{
    let axis=|key:&str|->Result<[f32;2],String>{
        let Some(a)=v.get(key) else{return Ok([0.0;2])};
        let a=a.as_array().filter(|a|a.len()==2).ok_or("stick requires two numbers")?;
        Ok([a[0].as_f64().ok_or("invalid stick")? as f32,a[1].as_f64().ok_or("invalid stick")? as f32])
    };
    let trigger=|key:&str|->Result<f32,String>{v.get(key).map_or(Ok(0.0),|v|v.as_f64().map(|n|n as f32).ok_or("invalid trigger".into()))};
    if !v.is_object(){return Err("pad must be an object".into());}
    let buttons=v.get("buttons").map_or(Ok(0),|v|v.as_u64().filter(|&n|n<=u16::MAX as u64).map(|n|n as u16).ok_or("invalid button bits"))?;
    let p=control::Pad{buttons,left:axis("left")?,right:axis("right")?,l2:trigger("l2")?,r2:trigger("r2")?};
    if !p.valid(){return Err("stick/trigger value out of range".into());}Ok(p)
}
pub(super) fn handle(owner:&str,body:&[u8])->(u16,String){
    let result=(||->Result<Value,String>{
        let v:Value=serde_json::from_slice(body).map_err(|_|"invalid JSON")?;
        if !v.is_object(){return Err("expected object".into());}
        let id=||v["session_id"].as_u64().filter(|&n|n!=0).ok_or("session_id required");
        match v["op"].as_str().ok_or("op required")? {
            "request"=>{let id=control::request(owner)?;Ok(json!({"session_id":id,"state":"awaiting_local_consent"}))},
            "status"=>Ok(control::status(owner)),
            "heartbeat"=>{control::heartbeat(id()?,owner)?;Ok(control::status(owner))},
            "pad"=>{let p=pad(&v["pad"])?;let ms=v["duration_ms"].as_u64().ok_or("duration_ms required")?;
                control::command(id()?,owner,p,ms)?;Ok(json!({"accepted":true}))},
            "hold"=>{let p=pad(&v["pad"])?;let ms=v["duration_ms"].as_u64().ok_or("duration_ms required")?;control::hold(id()?,owner,p,ms)?;Ok(json!({"accepted":true,"clock":"game","wall_limit_ms":4000}))},
            "react"=>{let when=match v["when"].as_str(){Some("taxi_near")=>control::session::Condition::TaxiNear,Some("passenger")=>control::session::Condition::Passenger,Some("vehicle")=>control::session::Condition::Vehicle,_=>return Err("unknown reaction condition".into())};
                let ms=v["duration_ms"].as_u64().ok_or("duration_ms required")?;let timeout=v["timeout_ms"].as_u64().ok_or("timeout_ms required")?;
                control::arm(id()?,owner,when,pad(&v["pad"])?,ms,timeout)?;Ok(json!({"armed":true,"once":true,"requires_heartbeats":true}))},
            "input"=>control::input_receipt(id()?,owner),
            "step"=>step(owner,id()?,&v),
            "watch"=>{control::watch(id()?,owner,v["duration_ms"].as_u64().ok_or("duration_ms required")?)?;Ok(json!({"watching":true,"max_fps":1,"buffer":"latest_only","poll":"observe"}))},
            "sequence"=>{control::sequence(id()?,owner,&steps(&v["steps"])?)?;Ok(json!({"accepted":true,"max_total_ms":4000}))},
            "prompt"=>{let input=v["input"].as_u64().filter(|&i|i<363).ok_or("visible input required")? as u32;
                let ms=v["duration_ms"].as_u64().ok_or("duration_ms required")?;control::prompt(id()?,owner,input,ms)?;Ok(json!({"accepted":true}))},
            "context"=>control::context(id()?,owner),
            "frame"|"audio"=>{let ticket=control::capture(id()?,owner,v["op"]=="audio")?;Ok(json!({"ticket":ticket,"state":"pending","poll":"observe"}))},
            "observe"=>Ok(observation(control::observe(id()?,owner)?)),
            "stop"=>{control::stop_owned(id()?,owner)?;Ok(control::status(owner))},
            _=>Err("unknown op".into()),
        }
    })();
    match result {Ok(v)=>(200,v.to_string()),Err(e)=>(400,json!({"error":e}).to_string())}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn pad_protocol_is_bounded_and_strict(){
        assert!(pad(&json!({"left":[-1.0,1.0],"r2":1.0,"buttons":64})).is_ok());
        for bad in [json!([]),json!({"buttons":65536}),json!({"buttons":-1}),json!({"left":[0]}),json!({"right":[2,0]}),json!({"r2":-0.1}),json!({"l2":"1"})] {assert!(pad(&bad).is_err(),"{bad}");}
    }
    #[test] fn malformed_protocol_and_unapproved_commands_are_refused(){
        for data in [b"[".as_slice(),b"[]",br#"{"op":"pad","session_id":7,"pad":{},"duration_ms":100}"#,br#"{"op":"heartbeat","session_id":7}"#] {
            assert_eq!(handle("test-owner",data).0,400);
        }
    }
    #[test] fn observation_and_sequences_require_a_local_grant(){
        for op in ["context","frame","audio","observe","sequence","prompt"]{assert_eq!(handle("test-owner",json!({"op":op,"session_id":7,"steps":[{"pad":{},"duration_ms":10}],"input":179,"duration_ms":10}).to_string().as_bytes()).0,400);}
        assert!(steps(&json!([])).is_err());assert!(steps(&json!([{"pad":{},"duration_ms":"10"}])).is_err());
    }
    #[test] fn composite_steps_are_strict_and_bounded_before_execution(){
        assert!(step_plan(&json!({})).is_ok());
        assert!(step_plan(&json!({"action":{"kind":"hold","duration_ms":2000,"pad":{"buttons":16}},"settle_ms":200})).is_ok());
        for v in [json!({"settle_ms":1001}),json!({"with_image":"true"}),json!({"action":{"kind":"shell"}}),json!({"action":{"kind":"hold","duration_ms":3000,"pad":{}}}),json!({"action":{"kind":"pad","duration_ms":1001,"pad":{}}})]{assert!(step_plan(&v).is_err());}
    }
}
