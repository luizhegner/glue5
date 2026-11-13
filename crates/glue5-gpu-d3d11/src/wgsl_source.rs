//! Pinned Naga 30.0.1 compiler boundary; no handwritten HLSL fallback.
use anyhow::{Result,ensure,Context};
#[derive(Clone,Copy,Debug)]
pub struct LocationContract {pub location:u32,pub wire:[u32;4]}
pub struct Generated {pub hlsl:String,pub entry:String,pub locations:Vec<u32>}
pub fn generate(source:&str,vertex:bool,entry:&str,contract:&[LocationContract])->Result<Generated> {
    let m=naga::front::wgsl::parse_str(source).map_err(|e|anyhow::anyhow!("WGSL: {}",e.emit_to_string(source)))?;
    // Explicit initial SM4 subset, rather than silently rewriting unsupported
    // resources or sampler heaps from newer Naga output.
    ensure!(m.global_variables.is_empty(),"WGSL catalog initial subset has no global resources");
    let stage=if vertex {naga::ShaderStage::Vertex} else {naga::ShaderStage::Fragment};
    let index=m.entry_points.iter().position(|e|e.name==entry && e.stage==stage).context("WGSL entry/stage absent")?;
    let info=naga::valid::Validator::new(naga::valid::ValidationFlags::all(),naga::valid::Capabilities::empty()).validate(&m)?;
    let mut locations=Vec::new();
    let mut collect=|b:&Option<naga::Binding>|->Result<()> {
        match b {
            Some(naga::Binding::Location{location,..})=>{locations.push(*location);Ok(())},
            Some(naga::Binding::BuiltIn(_))=>anyhow::bail!("WGSL input builtins not supported in this catalog subset"),
            None=>anyhow::bail!("WGSL input missing explicit binding"),
        }
    };
    for arg in &m.entry_points[index].function.arguments {
        if let naga::TypeInner::Struct{members,..}=&m.types[arg.ty].inner {
            for member in members {collect(&member.binding)?;}
        } else {collect(&arg.binding)?;}
    }
    if vertex {
        let mut expected=contract.iter().map(|c|c.location).collect::<Vec<_>>();expected.sort_unstable();
        let mut actual=locations.clone();actual.sort_unstable();
        ensure!(actual==expected && actual.windows(2).all(|p|p[0]!=p[1]),"WGSL location contract mismatch/duplicate");
    } else {ensure!(contract.is_empty() && locations.is_empty(),"PS varying inputs are outside initial catalog subset");}
    let options=naga::back::hlsl::Options {shader_model:naga::back::hlsl::ShaderModel::V5_0,fake_missing_bindings:false,..Default::default()};
    let pipeline=naga::back::hlsl::PipelineOptions{entry_point:Some((stage,entry.into()))};
    let mut hlsl=String::new();
    let reflection=naga::back::hlsl::Writer::new(&mut hlsl,&options,&pipeline).write(&m,&info,None)?;
    let emitted=reflection.entry_point_names.get(index).context("Naga entry reflection absent")?.as_ref().map_err(|e|anyhow::anyhow!("Naga entry reflection: {e:?}"))?.clone();
    for location in &locations {
        // LOCn is the pinned backend ABI (writer.rs LOCATION_SEMANTIC), not
        // a lookup guessed from an engine attribute hash. Check emitted code;
        // the native catalog additionally checks the DXBC input signature.
        ensure!(hlsl.contains(&format!(": LOC{location}")),"Naga semantic ABI changed");
    }
    Ok(Generated{hlsl,entry:emitted,locations})
}
