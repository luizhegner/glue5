//! Small authenticated control endpoint, not a web panel or arbitrary executor.
use serde_json::{Value,json};
use glue5_runtime::diagnostic_control as control;
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
}
