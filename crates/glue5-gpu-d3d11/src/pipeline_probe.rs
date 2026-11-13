//! Independent synthetic integration: real DXBC artifacts, payload decoder,
//! graphics executor, hardware draws, event completion and staging pixels.
//! Fixture compilation/catalog below is TEST-ONLY, never an executor fallback.
use std::{sync::Arc, ffi::CString};
use anyhow::{Result, Context, ensure};
use windows::{core::{Interface,PCSTR},Win32::Graphics::{Direct3D::{ID3DBlob,ID3DInclude},Direct3D::Fxc::*,Dxgi::{IDXGIDevice,IDXGIAdapter1,DXGI_ADAPTER_FLAG_SOFTWARE}}};
use glue5_gpu_d3d11::{FeatureLevel,pipeline::{GraphicsExecutor,PipelineCommand,PipelineKind as P},shader_resolver::{ShaderResolver,Artifact,Stage,Semantic,Signature}};
use crate::ring_decode::{self,Command,Kind as R};
fn compile(source:&str,profile:&str)->Result<Arc<[u8]>> {
    let entry=CString::new("main")?;let target=CString::new(profile)?;
    let mut blob:Option<ID3DBlob>=None;let mut errors:Option<ID3DBlob>=None;
    let result=unsafe {D3DCompile(source.as_ptr().cast(),source.len(),PCSTR::null(),None,None::<&ID3DInclude>,PCSTR(entry.as_ptr().cast()),PCSTR(target.as_ptr().cast()),D3DCOMPILE_ENABLE_STRICTNESS,0,&mut blob,Some(&mut errors))};
    if let Err(e)=result {let text=errors.as_ref().map(|b|unsafe {String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer().cast(),b.GetBufferSize())).into_owned()}).unwrap_or_default();anyhow::bail!("fixture FXC {profile}: {e}: {text}");}
    let b=blob.context("fixture compiler returned no blob")?;
    Ok(unsafe {std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(),b.GetBufferSize())}.to_vec().into())
}
fn resource(e:&mut GraphicsExecutor,kind:R,p:&[u32])->Result<()> {
    let command=ring_decode::decode(Command{op:kind,payload:p})?.context("expected a resource command")?;
    e.execute(PipelineCommand::Resource(command))
}
fn bind(e:&mut GraphicsExecutor,ps:u32)->Result<()> {
    // Exercise layout-before-VS: layout realization must use the correct VS,
    // not whichever shader happened to be bound at CREATE_LAYOUT time.
    e.submit(P::Layout,&[4])?;e.submit(P::Shader,&[0,1])?;e.submit(P::Shader,&[1,ps])?;
    e.submit(P::Topology,&[4])?;e.submit(P::VertexBuffers,&[0,1,101,12,0])?;
    e.submit(P::IndexBuffer,&[102,57,2])?;
    let mut rt=[0;37];rt[0]=201;rt[3]=28;e.submit(P::RenderTargets,&rt)?;
    e.submit(P::Viewport,&[1,0,0,64f32.to_bits(),64f32.to_bits(),0,1f32.to_bits()])?;Ok(())
}
fn vertices(e:&mut GraphicsExecutor)->Result<()> {
    resource(e,R::CREATE_BUFFER,&[101,72,1,0,0])?;
    let mut p=vec![101,0,72];p.extend([2f32,2.0,0.0,3.0,2.0,0.0,2.0,3.0,0.0,-0.75,-0.75,0.0,0.0,0.75,0.0,0.75,-0.75,0.0].map(f32::to_bits));
    resource(e,R::UPLOAD_BUFFER,&p)
}
fn clear(e:&mut GraphicsExecutor)->Result<()> {resource(e,R::CLEAR_RT,&[201,0,0,0,0,0,1f32.to_bits()])}
fn pixels(e:&mut GraphicsExecutor,label:&str,expected:[u8;4])->Result<()> {
    e.wait_for_gpu()?;
    let r=e.resources();let bytes=r.device().readback_texture(r.texture(201).context("RT missing")?)?;
    ensure!(bytes.len()==64*64*4,"staging byte size mismatch");
    let at=|x:usize,y:usize|->[u8;4]{bytes[(y*64+x)*4..(y*64+x)*4+4].try_into().unwrap()};
    let center=at(32,32);
    for (x,y) in [(24,40),(40,40),(32,24)] {
        ensure!(at(x,y).iter().zip(expected).all(|(a,b)|a.abs_diff(b)<=2),"{label}: interior ({x},{y}) mismatch: {:?}",at(x,y));
    }
    let covered=bytes.chunks_exact(4).filter(|p|p.iter().zip(expected).all(|(a,b)|a.abs_diff(b)<=2)).count();
    ensure!(covered==1152,"{label}: expected 1152 covered pixels, obtained {covered}");
    ensure!(center.iter().zip(expected).all(|(a,b)|a.abs_diff(b)<=2),"{label}: center {center:?}, expected {expected:?}");
    for (x,y) in [(0,0),(63,0),(0,63),(63,63)] {ensure!(at(x,y)==[0,0,0,255],"{label}: corner ({x},{y}) = {:?}",at(x,y));}
    println!("[payload GPU] {label}: expected={expected:?} obtained={center:?}; four corners=[0,0,0,255]; coverage=1152; interiors=4; tolerance=2");Ok(())
}
pub fn run(strict:bool)->Result<()> {
    println!("[payload GPU] independent decoder -> graphics executor; no direct-probe draw calls");
    // Explicit fixture catalog identities, NOT hashes derived from game WGSL
    // or a guessed equivalence between protocol hashes and DXBC checksums.
    let mut resolver=ShaderResolver::default();
    resolver.register(101,Artifact{protocol_hash:101,stage:Stage::Vertex,signature:vec![Semantic{wire:Signature([77,0,0,91]),name:"POSITION".into()}],dxbc:compile("float4 main(float3 p:POSITION):SV_Position{return float4(p,1);}","vs_4_0")?})?;
    for (hash,color) in [(102,"0.25,0.5,0.75,1"),(103,"0.5,0.25,0.125,1")] {
        resolver.register(hash,Artifact{protocol_hash:hash,stage:Stage::Pixel,signature:vec![],dxbc:compile(&format!("float4 main():SV_Target{{return float4({color});}}"),"ps_4_0")?})?;
    }
    let mut e=GraphicsExecutor::open_hardware(resolver)?;
    let dxgi:IDXGIDevice=e.resources().device().raw_device().cast()?;
    let adapter:IDXGIAdapter1=unsafe {dxgi.GetAdapter()?}.cast()?;let desc=unsafe {adapter.GetDesc1()?};
    ensure!(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32==0,"software adapter forbidden");
    if strict {ensure!(desc.VendorId==0x8086 && desc.DeviceId==0x2a42 && e.resources().device().feature_level()==FeatureLevel::Level10_0,"strict GM45/FL10_0 mismatch");}
    println!("[payload GPU] adapter={:04x}:{:04x}, FL={:?}, software=false, no WARP",desc.VendorId,desc.DeviceId,e.resources().device().feature_level());
    ensure!(e.submit(P::CreateShader,&[9,0,999,0,0]).is_err(),"missing artifact accepted");
    ensure!(e.submit(P::CreateShader,&[9,1,101,0,0]).is_err(),"wrong stage accepted");
    ensure!(e.submit(P::CreateShader,&[9,0,101,0,1,77,0,0,92]).is_err(),"wrong signature accepted");
    e.submit(P::CreateShader,&[1,0,101,0,1,77,0,0,91])?;
    e.submit(P::CreateShader,&[2,1,102,0,0])?;e.submit(P::CreateShader,&[3,1,103,0,0])?;
    ensure!(e.submit(P::CreateShader,&[1,0,101,0,1,77,0,0,91]).is_err(),"duplicate ID accepted");
    e.submit(P::CreateLayout,&[4,1,77,0,6,0,0,0,0])?;
    vertices(&mut e)?;resource(&mut e,R::CREATE_BUFFER,&[102,10,2,0,0])?;
    resource(&mut e,R::UPLOAD_BUFFER,&[102,0,10,0xffffffff,0x00050004,6])?;
    resource(&mut e,R::CREATE_TEXTURE,&[201,2,64,64,1,1,1,28,0x28,1])?;
    // Native Draw must skip three offscreen vertices. Native DrawIndexed
    // must skip two sentinel indices using byte offset=2/start_index=1,
    // and apply base_vertex=-1 to indices [4,5,6]. No Draw shortcut passes.
    resource(&mut e,R::CREATE_BUFFER,&[103,10,2,0,0])?;
    ensure!(resource(&mut e,R::CREATE_SRV,&[302,0,1,28,4,0,1,0,1]).is_err(),"shader accepted as SRV parent");
    ensure!(resource(&mut e,R::CREATE_BUFFER,&[1,12,1,0,0]).is_err(),"shader/buffer namespace collision accepted");
    bind(&mut e,2)?;e.submit(P::IndexBuffer,&[103,57,2])?;
    ensure!(e.submit(P::Draw,&[1,3,1,1,u32::MAX,0]).is_err(),"uninitialized index bytes accepted");
    bind(&mut e,2)?;e.submit(P::VertexBuffers,&[0,1,101,8,0])?;
    ensure!(e.submit(P::Draw,&[0,3,1,3,0,0]).is_err(),"incompatible stride accepted");
    bind(&mut e,2)?;e.submit(P::VertexBuffers,&[0,1,101,12,60])?;
    ensure!(e.submit(P::Draw,&[0,3,1,3,0,0]).is_err(),"out-of-range vertex fetch accepted");
    bind(&mut e,2)?;clear(&mut e)?;e.submit(P::Draw,&[0,3,1,3,0,0])?;pixels(&mut e,"Draw",[64,128,191,255])?;
    e.submit(P::Shader,&[1,3])?;clear(&mut e)?;e.submit(P::Draw,&[1,3,1,1,u32::MAX,0])?;pixels(&mut e,"DrawIndexed + PS switch",[128,64,32,255])?;
    ensure!(e.draws==1 && e.indexed_draws==1,"draw counts mismatch");
    ensure!(e.submit(P::Draw,&[1,4,1,1,u32::MAX,0]).is_err(),"out of range IB accepted");
    ensure!(e.submit(P::Draw,&[0,3,1,3,0,0]).is_err(),"state survived error cleanup");
    bind(&mut e,2)?;resource(&mut e,R::DESTROY_OBJECT,&[101])?;
    ensure!(e.submit(P::Draw,&[0,3,1,3,0,0]).is_err(),"destroyed bound VB accepted");
    ensure!(e.submit(P::VertexBuffers,&[0,1,101,12,0]).is_err(),"destroyed VB rebound");
    vertices(&mut e)?;bind(&mut e,2)?;clear(&mut e)?;e.submit(P::Draw,&[0,3,1,3,0,0])?;pixels(&mut e,"VB destroy/recreate + rebind",[64,128,191,255])?;
    resource(&mut e,R::CREATE_SRV,&[301,0,201,28,4,0,1,0,1])?;
    ensure!(resource(&mut e,R::SET_SRVS,&[1,0,1,301]).is_err(),"RT -> SRV conflict accepted");
    resource(&mut e,R::SET_SRVS,&[1,0,1,301])?;
    let mut rt=[0;37];rt[0]=201;rt[3]=28;
    ensure!(e.submit(P::RenderTargets,&rt).is_err(),"SRV -> RT conflict accepted");
    bind(&mut e,3)?;clear(&mut e)?;e.submit(P::Draw,&[1,3,1,1,u32::MAX,0])?;pixels(&mut e,"hazard cleanup + rebind",[128,64,32,255])?;
    ensure!(e.submit(P::Draw,&[0,3,1]).is_err(),"truncated DRAW accepted");
    bind(&mut e,2)?;resource(&mut e,R::DESTROY_OBJECT,&[2])?;
    ensure!(e.submit(P::Shader,&[1,2]).is_err(),"destroyed PS accepted");
    e.submit(P::CreateShader,&[2,1,102,0,0])?;bind(&mut e,2)?;clear(&mut e)?;e.submit(P::Draw,&[0,3,1,3,0,0])?;pixels(&mut e,"PS destroy/recreate + rebind",[64,128,191,255])?;
    ensure!(e.draws==3 && e.indexed_draws==2,"negative test submitted an unexpected draw");
    for id in [1,2,3,4,101,102,103,201,301] {resource(&mut e,R::DESTROY_OBJECT,&[id])?;}
    e.wait_for_gpu()?;
    println!("PASS: payload-driven native Draw=3 DrawIndexed=2; staging pixels, negative cases, cleanup and resource transitions passed");
    println!("NOTE: synthetic catalog and real shared executor; not a live GpuRing reader/game/presentation test");Ok(())
}
