//! Real shared GpuRing reader/production pump, WGSL-produced catalog and GM45.
//! Producer/storage are explicit fixtures; reader, decoder and GPU are not mocks.
use std::sync::atomic::Ordering;
use anyhow::{Result,ensure,Context};
use windows::{core::Interface,Win32::Graphics::Dxgi::{IDXGIDevice,IDXGIAdapter1,DXGI_ADAPTER_FLAG_SOFTWARE}};
use glue5_ring::{GpuRing,Reader,RingMemory,State,op,linear::LinearMemory};
use glue5_gpu_d3d11::{FeatureLevel,pipeline::GraphicsExecutor,drain_available,open_with_artifacts,wire_fingerprint,wgsl_catalog::{self as catalog,LocationContract,ProducedShader}};
type Mem=&'static LinearMemory;
const BASE:u64=4096;const SIZE:u64=4096;const FENCE:u64=128;
const POSITION:u32=0x5747534c; // Explicit fixture contract token, not game's hash.
const VS:&str="@vertex fn main(@location(0) p:vec3<f32>)->@builtin(position) vec4<f32>{return vec4<f32>(p,1.0);}";
const PS1:&str="@fragment fn main()->@location(0) vec4<f32>{return vec4<f32>(0.25,0.5,0.75,1.0);}";
const PS2:&str="@fragment fn main()->@location(0) vec4<f32>{return vec4<f32>(0.5,0.25,0.125,1.0);}";
fn packet(words:&mut Vec<u32>,op:u32,payload:&[u32]) {words.extend([op,payload.len() as u32]);words.extend_from_slice(payload);}
fn publish(m:Mem,cursor:u32,words:&[u32])->Result<(u32,Vec<u32>)> {
    let end=cursor as usize+words.len()*4;ensure!(end<=SIZE as usize,"producer ring overflow");
    let mut offsets=vec![];let mut at=0;
    while at<words.len() {offsets.push(cursor+(at as u32+2)*4);at+=2+words[at+1] as usize;}
    ensure!(at==words.len(),"producer frame malformed");
    for (i,&w) in words.iter().enumerate() {m.store(BASE+cursor as u64+i as u64*4,w);}
    m.store(BASE,end as u32); // Release publication AFTER all command bytes.
    Ok((end as u32,offsets))
}
fn run_batch(m:Mem,reader:&mut Reader<Mem>,e:&mut GraphicsExecutor,cursor:&mut u32,mut words:Vec<u32>,seq:u32,wrapped:bool)->Result<()> {
    let expected=wire_fingerprint(&words);let count=words.len();let old_executed=m.load(BASE+12);
    let (end,offsets)=publish(m,*cursor,&words)?;
    // Reader must borrow the copied ring storage, never this source Vec.
    words.fill(0xdeadbeef);
    let previous=m.load(FENCE);let (ready_tx,ready_rx)=std::sync::mpsc::sync_channel(0);
    let waiter=std::thread::spawn(move || {
        ready_tx.send(()).unwrap();m.wait32(m.atomic_u32(FENCE),previous,std::time::Duration::from_secs(5));m.load(FENCE)
    });ready_rx.recv()?;
    let proof=drain_available(reader,e)?;
    ensure!(proof.fingerprint==expected && proof.words==count,"reader bytes differ from published producer bytes");
    ensure!(proof.payload_offsets==offsets && proof.wrapped==wrapped,"reader zero-copy offsets/WRAP mismatch");
    ensure!(m.load(BASE+4)==end && m.load(BASE+12)-old_executed==proof.commands as u32,"tail/executed publication mismatch");
    ensure!(proof.opcodes.last()==Some(&op::FENCE),"batch missing actual ring FENCE");
    ensure!(waiter.join().map_err(|_|anyhow::anyhow!("fence waiter panicked"))?==seq && m.load(FENCE)==seq,"GPU FENCE did not publish/notify expected sequence");
    println!("[real ring] commands={} words={} fingerprint={:016x}; zero-copy offsets={:?}; WRAP={}; FENCE={seq}",proof.commands,proof.words,proof.fingerprint,proof.payload_offsets,proof.wrapped);
    *cursor=end;Ok(())
}
fn reject(m:Mem,reader:&mut Reader<Mem>,e:&mut GraphicsExecutor,cursor:&mut u32,request:&[u32],label:&str)->Result<()> {
    let mut words=vec![];packet(&mut words,op::CREATE_SHADER,request);packet(&mut words,op::FENCE,&[FENCE as u32,0,999]);
    let (end,_)=publish(m,*cursor,&words)?;words.fill(0xdeadbeef);
    ensure!(drain_available(reader,e).is_err(),"{label}: invalid ring shader accepted");
    ensure!(m.load(FENCE)==0 && e.draws==0 && e.indexed_draws==0,"{label}: failed packet completed GPU work/fence");
    reader.discard_to_head(end)?;*cursor=end;
    println!("[real ring] negative {label}: rejected through reader/resolver; later FENCE not notified");Ok(())
}
fn bind(words:&mut Vec<u32>,ps:u32) {
    packet(words,op::SET_INPUT_LAYOUT,&[4]);packet(words,op::SET_SHADER,&[0,1]);packet(words,op::SET_SHADER,&[1,ps]);
    packet(words,op::SET_TOPOLOGY,&[4]);packet(words,op::SET_VERTEX_BUFFERS,&[0,1,101,12,0]);packet(words,op::SET_INDEX_BUFFER,&[102,57,2]);
    let mut rt=[0;37];rt[0]=201;rt[3]=28;packet(words,op::SET_RENDER_TARGETS,&rt);
    packet(words,op::SET_VIEWPORTS,&[1,0,0,64f32.to_bits(),64f32.to_bits(),0,1f32.to_bits()]);
}
fn clear(words:&mut Vec<u32>) {packet(words,op::CLEAR_RT,&[201,0,0,0,0,0,1f32.to_bits()]);}
fn pixels(e:&GraphicsExecutor,label:&str,color:[u8;4])->Result<()> {
    let r=e.resources();let bytes=r.device().readback_texture(r.texture(201).context("RT missing")?)?;
    ensure!(bytes.len()==64*64*4,"readback size mismatch");
    let at=|x:usize,y:usize|->[u8;4]{bytes[(y*64+x)*4..(y*64+x)*4+4].try_into().unwrap()};
    for (x,y) in [(32,32),(24,40),(40,40),(32,24)] {ensure!(at(x,y).iter().zip(color).all(|(a,b)|a.abs_diff(b)<=2),"{label}: interior mismatch");}
    for (x,y) in [(0,0),(63,0),(0,63),(63,63)] {ensure!(at(x,y)==[0,0,0,255],"{label}: background mismatch");}
    let count=bytes.chunks_exact(4).filter(|p|p.iter().zip(color).all(|(a,b)|a.abs_diff(b)<=2)).count();ensure!(count==1152,"{label}: coverage {count}, expected 1152");
    println!("[real ring GPU] {label}: expected={color:?} obtained={:?}; coverage={count}; interiors=4; corners=4 black; tolerance=2",at(32,32));Ok(())
}
fn save(root:&std::path::Path,name:&str,wgsl:&str,p:&ProducedShader)->Result<()> {
    let base=root.join(".artifacts");std::fs::create_dir_all(&base)?;
    std::fs::write(base.join(format!("gm45-ring-{name}.wgsl")),wgsl)?;
    std::fs::write(base.join(format!("gm45-ring-{name}.hlsl")),&p.hlsl)?;
    std::fs::write(base.join(format!("gm45-ring-{name}.dxbc")),&p.artifact.dxbc)?;
    let hex=|a:&[u8]|a.iter().map(|b|format!("{b:02x}")).collect::<String>();
    let line=format!("program={:016x} stage={:?} profile={} entry={} wgsl_sha256={} dxbc_sha256={} signature={:?}\n",p.artifact.protocol_hash,p.artifact.stage,p.profile,p.entry,hex(&p.wgsl_sha256),hex(&p.dxbc_sha256),p.artifact.signature);
    std::fs::write(base.join(format!("gm45-ring-{name}.catalog.txt")),&line)?;print!("[WGSL catalog] {line}");Ok(())
}
pub fn run()->Result<()> {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let runtime=std::fs::read_to_string(root.join("crates/glue5-runtime/src/gpu.rs"))?;
    ensure!(runtime.contains("pub type Reader = glue5_ring::Reader<Mem>") && !runtime.contains("pub struct Reader"),"runtime is not wired to shared actual reader");
    let contract=[LocationContract{location:0,wire:[POSITION,0,0,91]}];
    let vs=catalog::compile(catalog::fixture_hash(VS,true,"main",&contract),VS,true,"main",&contract)?;
    let ps1=catalog::compile(catalog::fixture_hash(PS1,false,"main",&[]),PS1,false,"main",&[])?;
    let ps2=catalog::compile(catalog::fixture_hash(PS2,false,"main",&[]),PS2,false,"main",&[])?;
    save(&root,"vs",VS,&vs)?;save(&root,"ps1",PS1,&ps1)?;save(&root,"ps2",PS2,&ps2)?;
    let mut e=open_with_artifacts([&vs,&ps1,&ps2].into_iter().map(|p|(p.artifact.protocol_hash,p.artifact.clone())).collect())?;
    let dxgi:IDXGIDevice=e.resources().device().raw_device().cast()?;
    let adapter:IDXGIAdapter1=unsafe {dxgi.GetAdapter()?}.cast()?;let desc=unsafe {adapter.GetDesc1()?};
    ensure!(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32==0 && desc.VendorId==0x8086 && desc.DeviceId==0x2a42 && e.resources().device().feature_level()==FeatureLevel::Level10_0,"requires physical GM45 8086:2A42 FL10_0; no software/WARP");
    println!("[real ring GPU] adapter=8086:2a42 FL=Level10_0 software=false no WARP; catalog=WGSL/Naga30.0.1/FXC SM4");
    let m:Mem=Box::leak(Box::new(LinearMemory::new(16384)));let ring=GpuRing::new(m,BASE,SIZE);
    m.store(BASE,64);let mut reader=ring.reader();ring.set_state(State::Running);let mut cursor=64;
    let mut bad=vs.create_payload(9);bad[2]^=1;reject(m,&mut reader,&mut e,&mut cursor,&bad,"missing hash")?;
    let mut bad=vs.create_payload(9);bad[1]=1;reject(m,&mut reader,&mut e,&mut cursor,&bad,"wrong stage")?;
    let mut bad=vs.create_payload(9);bad[8]^=1;reject(m,&mut reader,&mut e,&mut cursor,&bad,"wrong opaque signature")?;
    let mut w=vec![];
    for (id,p) in [(1,&vs),(2,&ps1),(3,&ps2)] {packet(&mut w,op::CREATE_SHADER,&p.create_payload(id));}
    packet(&mut w,op::CREATE_LAYOUT,&[4,1,POSITION,0,6,0,0,0,0]);
    packet(&mut w,op::CREATE_BUFFER,&[101,72,1,0,0]);
    let mut vertices=vec![101,0,72];vertices.extend([2f32,2.0,0.0,3.0,2.0,0.0,2.0,3.0,0.0,-0.75,-0.75,0.0,0.0,0.75,0.0,0.75,-0.75,0.0].map(f32::to_bits));packet(&mut w,op::UPLOAD_BUFFER,&vertices);
    packet(&mut w,op::CREATE_BUFFER,&[102,10,2,0,0]);packet(&mut w,op::UPLOAD_BUFFER,&[102,0,10,0xffffffff,0x00050004,6]);
    packet(&mut w,op::CREATE_TEXTURE,&[201,2,64,64,1,1,1,28,0x28,1]);bind(&mut w,2);clear(&mut w);
    packet(&mut w,op::DRAW,&[0,3,1,3,0,0]);packet(&mut w,op::FENCE,&[FENCE as u32,0,1001]);
    run_batch(m,&mut reader,&mut e,&mut cursor,w,1001,false)?;pixels(&e,"Draw start_vertex=3",[64,128,191,255])?;
    let mut w=vec![];packet(&mut w,op::SET_SHADER,&[1,3]);clear(&mut w);packet(&mut w,op::DRAW,&[1,3,1,1,u32::MAX,0]);packet(&mut w,op::FENCE,&[FENCE as u32,0,1002]);
    run_batch(m,&mut reader,&mut e,&mut cursor,w,1002,false)?;pixels(&e,"DrawIndexed offset=2 start=1 base=-1",[128,64,32,255])?;
    // Publish a genuine WRAP marker at the old cursor; keep the SAME reader.
    let old=cursor;m.store(BASE+old as u64,op::WRAP);m.store(BASE+old as u64+4,0);cursor=64;
    let mut w=vec![];packet(&mut w,op::SET_SHADER,&[1,2]);clear(&mut w);packet(&mut w,op::DRAW,&[0,3,1,3,0,0]);packet(&mut w,op::FENCE,&[FENCE as u32,0,1003]);
    ensure!(64+w.len() as u32*4<old,"WRAP fixture must publish head below old tail");
    run_batch(m,&mut reader,&mut e,&mut cursor,w,1003,true)?;pixels(&e,"WRAP + Draw",[64,128,191,255])?;
    ensure!(e.draws==2 && e.indexed_draws==1 && m.load(BASE+8)==State::Running as u32,"unexpected native draw/state count");
    println!("PASS: real shared GpuRing reader -> production pump -> resolver(WGSL catalog) -> native Draw=2 DrawIndexed=1 -> GM45 pixels; FENCE=1003; notifications={}",m.notifications.load(Ordering::Relaxed));
    println!("NOTE: explicit WGSL fixture producer/host linear storage; actual shared runtime reader and GPU; not game execution or presentation; asynchronous worker lifecycle not exercised by bounded pump test.");Ok(())
}
