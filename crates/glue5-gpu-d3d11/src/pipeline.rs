//! Payload-driven graphics execution. No shader source, compiler, or fallback.
use std::{collections::{HashMap, HashSet}, sync::Arc};
use anyhow::{Result, Context, ensure};
use windows::Win32::Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY, Direct3D11::*, Dxgi::Common::DXGI_FORMAT};
use crate::{D3D11Command, D3D11Executor};
use crate::shader_resolver::{Artifact, ShaderResolver, ShaderRequest, LayoutRequest, ShaderObject, LayoutObject, NativeShader, Stage};

#[derive(Clone, Copy, Debug)]
pub enum PipelineKind { CreateShader, CreateLayout, Shader, Layout, Topology, VertexBuffers, IndexBuffer, RenderTargets, Viewport, Draw, Clear }
pub enum PipelineCommand {
    Resource(D3D11Command), CreateShader(Vec<u32>), CreateLayout(Vec<u32>),
    Shader(u32,u32), Layout(u32), Topology(u32), VertexBuffers(u32,Vec<[u32;3]>),
    IndexBuffer([u32;3]), RenderTarget([u32;4]), Viewport([f32;6]), Draw([u32;6]), Clear,
}
/// Shared by the real ring adapter and independent synthetic integration.
pub fn decode(kind: PipelineKind, p: &[u32]) -> Result<PipelineCommand> {
    use PipelineCommand as C;
    Ok(match kind {
        PipelineKind::CreateShader => { ShaderRequest::decode(p)?; C::CreateShader(p.to_vec()) },
        PipelineKind::CreateLayout => { LayoutRequest::decode(p)?; C::CreateLayout(p.to_vec()) },
        PipelineKind::Shader => { ensure!(p.len()==2 && p[0]<2,"SET_SHADER invalid stage/length"); C::Shader(p[0],p[1]) },
        PipelineKind::Layout => { ensure!(p.len()==1,"SET_INPUT_LAYOUT length"); C::Layout(p[0]) },
        PipelineKind::Topology => { ensure!(p.len()==1 && (1..=5).contains(&p[0]),"unsupported topology"); C::Topology(p[0]) },
        PipelineKind::VertexBuffers => {
            ensure!(p.len()>=2,"SET_VERTEX_BUFFERS header");
            let (start,n)=(p[0],p[1]);
            ensure!(start<=16 && n<=16-start && p.len()==2+3*n as usize,"SET_VERTEX_BUFFERS range/length");
            let mut slots=Vec::new();
            for v in p[2..].chunks_exact(3) { ensure!(v[1]<=2048,"vertex stride exceeds FL10 limit"); slots.push([v[0],v[1],v[2]]); }
            C::VertexBuffers(start,slots)
        },
        PipelineKind::IndexBuffer => {
            ensure!(p.len()==3,"SET_INDEX_BUFFER length");
            ensure!(p[0]==0 || matches!(p[1],42|57),"index format must be R32_UINT/R16_UINT");
            if p[0]!=0 { ensure!(p[2] % if p[1]==42 {4} else {2} ==0,"index offset alignment"); }
            C::IndexBuffer([p[0],p[1],p[2]])
        },
        PipelineKind::RenderTargets => {
            ensure!(p.len()==37,"SET_RENDER_TARGETS length");
            ensure!(p[4..].iter().all(|w|*w==0),"only RT slot 0 without depth/stencil supported");
            ensure!(p[1]==0 && p[2]==0,"RT mip/slice unsupported");
            C::RenderTarget([p[0],p[1],p[2],p[3]])
        },
        PipelineKind::Viewport => {
            ensure!(p.len()==7 && p[0]==1,"one viewport required");
            let v: [f32;6]=std::array::from_fn(|i|f32::from_bits(p[i+1]));
            ensure!(v.iter().all(|f|f.is_finite()),"nonfinite viewport");
            ensure!(v[0]>=0.0 && v[1]>=0.0 && v[2]>0.0 && v[3]>0.0 && v[0]+v[2]<=16384.0 && v[1]+v[3]<=16384.0,"viewport bounds");
            ensure!(v[4]>=0.0 && v[5]<=1.0 && v[4]<=v[5],"viewport depth range");
            C::Viewport(v)
        },
        PipelineKind::Draw => { ensure!(p.len()==6 && p[0]<=1 && p[2]==1 && p[5]==0,"DRAW supports only six-word non-instanced kind 0/1"); C::Draw(p.try_into().unwrap()) },
        PipelineKind::Clear => { ensure!(p.is_empty(),"CLEAR_STATE length"); C::Clear },
    })
}
#[derive(Default)]
struct State { vs:u32, ps:u32, layout:u32, topology:u32, vb:[[u32;3];16], ib:[u32;3], rt:u32, viewport:bool }
impl State {
    fn references(&self,id:u32)->bool { self.vs==id || self.ps==id || self.layout==id || self.rt==id || self.ib[0]==id || self.vb.iter().any(|v|v[0]==id) }
}
struct IndexShadow { bytes:Vec<u8>, initialized:Vec<bool> }
impl IndexShadow {
    fn new(size:u32)->Result<Self> {
        let mut bytes=Vec::new(); let mut initialized=Vec::new();
        bytes.try_reserve_exact(size as usize)?; initialized.try_reserve_exact(size as usize)?;
        bytes.resize(size as usize,0); initialized.resize(size as usize,false);
        Ok(Self{bytes,initialized})
    }
    fn upload(&mut self,offset:u32,data:&[u8])->Result<()> {
        let start=offset as usize; let end=start.checked_add(data.len()).context("index upload overflow")?;
        ensure!(end<=self.bytes.len(),"index upload range");
        self.bytes[start..end].copy_from_slice(data); self.initialized[start..end].fill(true); Ok(())
    }
    fn maximum(&self,ib:[u32;3],draw:[u32;6])->Result<Option<u32>> {
        let width=if ib[1]==42 {4usize} else {2};
        let start=(ib[2] as usize).checked_add((draw[3] as usize).checked_mul(width).context("index start overflow")?).context("index offset overflow")?;
        let end=start.checked_add((draw[1] as usize).checked_mul(width).context("index size overflow")?).context("index end overflow")?;
        ensure!(end<=self.bytes.len(),"indexed draw exceeds buffer");
        ensure!(self.initialized[start..end].iter().all(|v|*v),"indexed draw reads uninitialized bytes");
        let mut maximum=None;
        for b in self.bytes[start..end].chunks_exact(width) {
            let index=if width==4 {u32::from_le_bytes(b.try_into().unwrap())} else {u16::from_le_bytes(b.try_into().unwrap()) as u32};
            let v=i64::from(index)+i64::from(draw[4] as i32);
            ensure!((0..=i64::from(u32::MAX)).contains(&v),"base vertex makes index invalid");
            maximum=Some(maximum.unwrap_or(0).max(v as u32));
        }
        Ok(maximum)
    }
}
/// Owns one fresh resource executor and one coherent native graphics state.
/// No mutable resource-executor escape hatch: every resource ID passes here.
pub struct GraphicsExecutor {
    inner:D3D11Executor, resolver:ShaderResolver, ids:HashSet<u32>,
    shaders:HashMap<u32,Arc<ShaderObject>>, layouts:HashMap<u32,Vec<u32>>,
    native_layouts:HashMap<(u32,u32),LayoutObject>, indices:HashMap<u32,IndexShadow>,
    srv_sources:HashMap<u32,u32>, srv_buffers:HashSet<u32>, srv_slots:[[u32;128];3], state:State,
    pub draws:u64, pub indexed_draws:u64,
}
impl GraphicsExecutor {
    pub fn open_hardware(resolver:ShaderResolver)->Result<Self> {
        Ok(Self { inner:D3D11Executor::open_hardware()?,resolver,ids:HashSet::new(),shaders:HashMap::new(),layouts:HashMap::new(),native_layouts:HashMap::new(),indices:HashMap::new(),srv_sources:HashMap::new(),srv_buffers:HashSet::new(),srv_slots:[[0;128];3],state:State::default(),draws:0,indexed_draws:0 })
    }
    pub fn resources(&self)->&D3D11Executor { &self.inner }
    pub fn register_artifact(&mut self,hash:u64,artifact:Artifact)->Result<()> {self.resolver.register(hash,artifact)}
    pub fn wait_for_gpu(&mut self)->Result<()> {self.inner.wait_for_gpu()}
    pub fn submit(&mut self,kind:PipelineKind,p:&[u32])->Result<()> {
        match decode(kind,p) {Ok(c)=>self.execute(c),Err(e)=>{self.clear()?;Err(e)}}
    }
    fn vacant(&self,id:u32)->Result<()> {ensure!(id!=0 && !self.ids.contains(&id),"resource ID {id} is zero or already exists");Ok(())}
    fn clear(&mut self)->Result<()> {
        // Reset tracked SRV/sampler requests as well as native bindings. A bare
        // ClearState would leave the old executor shadow state inconsistent.
        unsafe {self.inner.device().immediate_context().ClearState();}
        self.srv_slots=[[0;128];3]; self.state=State::default();
        for stage in 0..2 {
            self.inner.execute(D3D11Command::SetSrvs {stage,start:0,views:vec![0;128]})?;
            self.inner.execute(D3D11Command::SetSamplers {stage,start:0,samplers:vec![0;16]})?;
        }
        Ok(())
    }
    pub fn execute(&mut self,c:PipelineCommand)->Result<()> {
        let result=self.execute_inner(c);
        if let Err(error)=result { self.clear().context("clearing state after graphics error")?; return Err(error); }
        Ok(())
    }
    fn layout_for(&mut self,layout:u32,vs:u32)->Result<()> {
        if layout==0 || vs==0 {return Ok(());}
        if !self.native_layouts.contains_key(&(layout,vs)) {
            let shader=self.shaders.get(&vs).context("missing vertex shader")?.clone();
            let payload=self.layouts.get(&layout).context("missing layout")?;
            let object=self.inner.create_layout_from_payload(shader,payload)?;
            self.native_layouts.insert((layout,vs),object);
        }
        Ok(())
    }
    fn execute_inner(&mut self,c:PipelineCommand)->Result<()> {
        use PipelineCommand as C;
        match c {
            C::CreateShader(p)=>{
                let id=ShaderRequest::decode(&p)?.id; self.vacant(id)?;
                let shader=self.inner.create_shader_from_payload(&self.resolver,&p)?;
                self.shaders.insert(id,shader);self.ids.insert(id);
            },
            C::CreateLayout(p)=>{let id=LayoutRequest::decode(&p)?.id;self.vacant(id)?;self.layouts.insert(id,p);self.ids.insert(id);},
            C::Shader(stage,id)=>{
                ensure!(stage<2,"unsupported shader stage");
                if id!=0 {let shader=self.shaders.get(&id).context("missing/destroyed shader")?;ensure!(shader.stage()==if stage==0 {Stage::Vertex}else{Stage::Pixel},"shader stage mismatch");}
                if stage==0 {self.layout_for(self.state.layout,id)?;}
                let ctx=self.inner.device().immediate_context();
                unsafe {
                    if stage==0 {
                        let native=match self.shaders.get(&id).map(|s|s.native()) {Some(NativeShader::Vertex(v))=>Some(v),_=>None};
                        ctx.VSSetShader(native,None);
                        let layout=self.native_layouts.get(&(self.state.layout,id)).and_then(|l|l.native());
                        ctx.IASetInputLayout(layout);self.state.vs=id;
                    } else {
                        let native=match self.shaders.get(&id).map(|s|s.native()) {Some(NativeShader::Pixel(v))=>Some(v),_=>None};
                        ctx.PSSetShader(native,None);self.state.ps=id;
                    }
                }
            },
            C::Layout(id)=>{
                ensure!(id==0 || self.layouts.contains_key(&id),"missing/destroyed layout");self.layout_for(id,self.state.vs)?;
                let native=self.native_layouts.get(&(id,self.state.vs)).and_then(|l|l.native());
                unsafe {self.inner.device().immediate_context().IASetInputLayout(native);}self.state.layout=id;
            },
            C::Topology(value)=>{ensure!((1..=5).contains(&value),"unsupported topology");unsafe {self.inner.device().immediate_context().IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY(value as i32));}self.state.topology=value;},
            C::VertexBuffers(start,slots)=>{
                ensure!(start<=16 && slots.len()<=16-start as usize,"VB slot range");
                let mut buffers=Vec::new();let mut strides=Vec::new();let mut offsets=Vec::new();
                for s in &slots {
                    ensure!(s[1]<=2048,"VB stride");
                    let buffer=if s[0]==0 {None}else {let b=self.inner.buffer(s[0]).context("missing/destroyed VB")?;ensure!(b.bind_flags()&1!=0 && s[2]<=b.size(),"invalid VB binding/offset");Some(b.buffer().clone())};
                    buffers.push(buffer);strides.push(s[1]);offsets.push(s[2]);
                }
                unsafe {self.inner.device().immediate_context().IASetVertexBuffers(start,slots.len() as u32,Some(buffers.as_ptr()),Some(strides.as_ptr()),Some(offsets.as_ptr()));}
                for (i,s) in slots.into_iter().enumerate(){self.state.vb[start as usize+i]=s;}
            },
            C::IndexBuffer(s)=>{
                let native=if s[0]==0 {None} else {let b=self.inner.buffer(s[0]).context("missing/destroyed IB")?;ensure!(b.bind_flags()&2!=0 && matches!(s[1],42|57) && s[2]<=b.size() && s[2] % if s[1]==42 {4}else{2}==0,"invalid IB binding");Some(b.buffer())};
                unsafe {self.inner.device().immediate_context().IASetIndexBuffer(native,DXGI_FORMAT(if s[0]==0 {0}else{s[1] as i32}),s[2]);}self.state.ib=s;
            },
            C::RenderTarget(s)=>{
                ensure!(!self.srv_slots.iter().flatten().any(|id|*id!=0 && self.srv_sources.get(id)==Some(&s[0])),"RT/SRV read-write conflict");
                let rtv=if s[0]==0 {None} else {let t=self.inner.texture(s[0]).context("missing/destroyed RT")?;ensure!(s[1]==0 && s[2]==0 && (s[3]==0 || s[3]==t.format().0 as u32),"RT view mismatch");Some(t.rtv().context("texture lacks RT binding")?.clone())};
                unsafe {self.inner.device().immediate_context().OMSetRenderTargets(Some(&[rtv]),None::<&ID3D11DepthStencilView>);}self.state.rt=s[0];
            },
            C::Viewport(v)=>{let vp=D3D11_VIEWPORT {TopLeftX:v[0],TopLeftY:v[1],Width:v[2],Height:v[3],MinDepth:v[4],MaxDepth:v[5]};unsafe {self.inner.device().immediate_context().RSSetViewports(Some(&[vp]));}self.state.viewport=true;},
            C::Draw(d)=>{
                ensure!(d[0]<=1 && d[2]==1 && d[5]==0,"unsupported DRAW kind/instances");
                ensure!(self.state.vs!=0 && self.state.ps!=0 && self.state.layout!=0 && self.state.rt!=0 && self.state.topology!=0 && self.state.viewport,"incomplete draw bindings");
                let layout=self.native_layouts.get(&(self.state.layout,self.state.vs)).context("layout not realized for bound VS")?;
                let strides=std::array::from_fn(|i|self.state.vb[i][1]); layout.validate_strides(&strides)?;
                let maximum=if d[0]==0 { if d[1]==0 {None} else {Some(d[3].checked_add(d[1]-1).context("vertex range overflow")?)} } else {
                    ensure!(self.state.ib[0]!=0,"indexed draw has no IB");
                    self.indices.get(&self.state.ib[0]).context("missing initialized index shadow")?.maximum(self.state.ib,d)?
                };
                if let Some(maximum)=maximum {
                    for [_,_,format,slot,offset,class,_rate] in LayoutRequest::decode(&self.layouts[&self.state.layout])?.elements {
                        let size=match format {2|3|4=>16,6|7|8=>12,16|17|18=>8,28|41|42|43=>4,_=>anyhow::bail!("unsupported vertex format")};
                        let end=offset.checked_add(size).context("vertex element overflow")?;
                        let binding=self.state.vb[slot as usize];let b=self.inner.buffer(binding[0]).context("required VB missing/destroyed")?;
                        let vertex=if class==0 {maximum}else{0};
                        let limit=u64::from(binding[2])+u64::from(vertex)*u64::from(binding[1])+u64::from(end);
                        ensure!(limit<=u64::from(b.size()),"draw reads past vertex buffer");
                    }
                }
                unsafe {let ctx=self.inner.device().immediate_context();if d[0]==0 {ctx.Draw(d[1],d[3]);self.draws+=1;}else{ctx.DrawIndexed(d[1],d[3],d[4] as i32);self.indexed_draws+=1;}}
            },
            C::Clear=>self.clear()?,
            C::Resource(command)=>self.resource(command)?,
        }
        Ok(())
    }
    fn resource(&mut self,command:D3D11Command)->Result<()> {
        let created=match &command {D3D11Command::CreateBuffer{id,..}|D3D11Command::CreateTexture2D{id,..}|D3D11Command::CreateSrv{id,..}|D3D11Command::CreateSampler{id,..}=>Some(*id),_=>None};
        if let Some(id)=created {self.vacant(id)?;}
        if let D3D11Command::DestroyObject{id}=command {
            ensure!(self.ids.contains(&id),"destroy of missing resource {id}");
            let active=self.state.references(id) || self.native_layouts.get(&(self.state.layout,self.state.vs)).is_some_and(|l|l.shader().id()==id) || self.srv_slots.iter().flatten().any(|v|*v==id || self.srv_sources.get(v)==Some(&id));
            if active {self.clear()?;}
            self.native_layouts.retain(|(l,v),_|*l!=id && *v!=id);self.shaders.remove(&id);self.layouts.remove(&id);self.indices.remove(&id);self.srv_sources.remove(&id);self.srv_buffers.remove(&id);
            self.inner.execute(D3D11Command::DestroyObject{id})?;self.ids.remove(&id);return Ok(());
        }
        let mut shadow=match &command {D3D11Command::CreateBuffer{id,size,bind,..} if bind&2!=0=>Some((*id,IndexShadow::new(*size)?)),_=>None};
        let srv=if let D3D11Command::CreateSrv{id,resource_id,is_buffer,..}=&command {
            ensure!(if *is_buffer {self.inner.buffer(*resource_id).is_some()} else {self.inner.texture(*resource_id).is_some()},"SRV source missing or wrong resource kind");Some((*id,*resource_id,*is_buffer))
        }else{None};
        if let D3D11Command::SetSrvs{stage,start,views}=&command {
            ensure!(*stage<2 && *start<=128 && views.len()<=128-*start as usize,"SRV range");
            for id in views.iter().filter(|id|**id!=0) {
                let source=*self.srv_sources.get(id).context("missing/destroyed SRV")?;
                ensure!(if self.srv_buffers.contains(id) {self.inner.buffer(source).is_some()} else {self.inner.texture(source).is_some()},"SRV parent missing or recreated as wrong kind");
            }
            ensure!(!views.iter().any(|id|*id!=0 && self.state.rt!=0 && self.srv_sources.get(id)==Some(&self.state.rt)),"SRV/RT read-write conflict");
        }
        if let D3D11Command::SetSamplers{stage,start,samplers}=&command {ensure!(*stage<2 && *start<=16 && samplers.len()<=16-*start as usize,"sampler stage/range");}
        let upload=if let D3D11Command::UploadBuffer{id,offset,data}=&command {Some((*id,*offset,data.clone()))}else{None};
        let slots=if let D3D11Command::SetSrvs{stage,start,views}=&command {Some((*stage,*start,views.clone()))}else{None};
        self.inner.execute(command)?;
        if let Some(id)=created {self.ids.insert(id);}
        if let Some((id,s))=shadow.take(){self.indices.insert(id,s);}
        if let Some((id,offset,data))=upload {if let Some(s)=self.indices.get_mut(&id){s.upload(offset,&data)?;}}
        if let Some((id,source,is_buffer))=srv {self.srv_sources.insert(id,source);if is_buffer {self.srv_buffers.insert(id);}}
        if let Some((stage,start,views))=slots {self.srv_slots[stage as usize][start as usize..start as usize+views.len()].copy_from_slice(&views);}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn draw_wire_is_six_words_and_rejects_instancing(){assert!(decode(PipelineKind::Draw,&[0,3,1,0,0,0]).is_ok());assert!(decode(PipelineKind::Draw,&[1,3,1,0,u32::MAX,0]).is_ok());for n in 0..6{assert!(decode(PipelineKind::Draw,&[0,3,1,0,0,0][..n]).is_err());}assert!(decode(PipelineKind::Draw,&[2,3,1,0,0,0]).is_err());assert!(decode(PipelineKind::Draw,&[0,3,2,0,0,0]).is_err());}
    #[test] fn vb_ranges_and_stride_are_bounded(){assert!(decode(PipelineKind::VertexBuffers,&[15,1,7,12,0]).is_ok());assert!(decode(PipelineKind::VertexBuffers,&[16,1,7,12,0]).is_err());assert!(decode(PipelineKind::VertexBuffers,&[0,1,7,2049,0]).is_err());}
    #[test] fn index_formats_and_alignment(){assert!(decode(PipelineKind::IndexBuffer,&[3,57,2]).is_ok());assert!(decode(PipelineKind::IndexBuffer,&[3,42,2]).is_err());assert!(decode(PipelineKind::IndexBuffer,&[3,41,0]).is_err());}
    #[test] fn viewport_rejects_nan_infinity_and_depth(){for f in [f32::NAN,f32::INFINITY,-1.0]{assert!(decode(PipelineKind::Viewport,&[1,0,0,f.to_bits(),64f32.to_bits(),0,1f32.to_bits()]).is_err());}}
    #[test] fn rt_rejects_mrt_depth_and_mips(){let mut p=[0;37];p[0]=2;assert!(decode(PipelineKind::RenderTargets,&p).is_ok());for i in [1,2,4,32,36]{p[i]=1;assert!(decode(PipelineKind::RenderTargets,&p).is_err());p[i]=0;}}
    #[test] fn index_shadow_tracks_upload_and_negative_base(){let mut s=IndexShadow::new(6).unwrap();let d=[1,3,1,0,0,0];assert!(s.maximum([7,57,0],d).is_err());s.upload(0,&[0,0,1,0,2,0]).unwrap();assert_eq!(s.maximum([7,57,0],d).unwrap(),Some(2));assert!(s.maximum([7,57,0],[1,3,1,0,u32::MAX,0]).is_err());assert!(s.maximum([7,57,0],[1,4,1,0,0,0]).is_err());}
    #[test] fn index_start_and_offset_are_both_applied(){let mut s=IndexShadow::new(8).unwrap();s.upload(0,&[99,0,98,0,2,0,4,0]).unwrap();assert_eq!(s.maximum([7,57,2],[1,2,1,1,1,0]).unwrap(),Some(5));}
    #[test] fn shader_layout_and_clear_wire_fail_closed(){assert!(decode(PipelineKind::Shader,&[2,7]).is_err());assert!(decode(PipelineKind::Layout,&[]).is_err());assert!(decode(PipelineKind::Clear,&[1]).is_err());assert!(decode(PipelineKind::CreateShader,&[7,0,1,0,0]).is_ok());}
}
