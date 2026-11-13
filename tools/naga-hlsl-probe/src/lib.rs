//! Shared engine GpuRing protocol and reader, extracted from glue5-runtime/gpu.rs.
//! There is one reader implementation for the runtime and native backend.
//! Memory ownership/futex policy belongs to the host, not to the GPU executor.
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use anyhow::{Context, Result, ensure};

pub const RING_DATA: u32 = 64;
const HEAD:u64=0; const TAIL:u64=4; const STATE:u64=8;
const EXECUTED:u64=12; const ERRORS:u64=16; const SLEEPING:u64=20;

/// # Safety
/// Implementations must keep their fixed memory mapping alive for every copy
/// and borrowed command; check_range must reject uncommitted/out-of-bounds
/// addresses. Word addresses must be suitably aligned. The producer must not
/// overwrite payloads before release, and must publish HEAD with Release.
pub unsafe trait RingMemory: Copy + Send + Sync + 'static {
    fn check_range(self, at:u64, len:u64)->Result<()>;
    fn ptr(self, at:u64)->*mut u8;
    fn notify(self, word:&AtomicU32);
    fn wait32(self, word:&AtomicU32, value:u32, timeout:Duration);
    fn backend_failed(self, reason:&str);
    fn atomic_u32<'a>(self, at:u64)->&'a AtomicU32 {
        assert!(at.is_multiple_of(4));
        self.check_range(at,4).expect("checked atomic memory range");
        unsafe { &*self.ptr(at).cast::<AtomicU32>() }
    }
    fn u32(self, at:u64)->u32 {
        self.check_range(at,4).expect("checked ring word");
        unsafe { self.ptr(at).cast::<u32>().read_unaligned() }
    }
    fn store_notify_checked(self, at:u64, value:u32)->Result<()> {
        ensure!(at.is_multiple_of(4),"unaligned fence address");
        self.check_range(at,4)?;
        let word=self.atomic_u32(at); word.store(value,Ordering::SeqCst);
        self.notify(word); Ok(())
    }
}

/// Opcodes remain identical to wgpu_backend.h and runtime gpu::op.
pub mod op {
    pub const NOP:u32=0; pub const WRAP:u32=1; pub const INIT:u32=2;
    pub const CREATE_TEXTURE:u32=3; pub const DESTROY_OBJECT:u32=4;
    pub const CLEAR_RT:u32=5; pub const CLEAR_DS:u32=6; pub const PRESENT:u32=7;
    pub const FENCE:u32=8; pub const SCREENSHOT:u32=9; pub const LOG:u32=10;
    pub const CREATE_BUFFER:u32=11; pub const UPLOAD_BUFFER:u32=12;
    pub const UPLOAD_TEXTURE:u32=13; pub const CREATE_SHADER:u32=14;
    pub const CREATE_LAYOUT:u32=15; pub const CREATE_STATE:u32=16;
    pub const CREATE_SRV:u32=17; pub const SET_SHADER:u32=18;
    pub const SET_INPUT_LAYOUT:u32=19; pub const SET_TOPOLOGY:u32=20;
    pub const SET_VERTEX_BUFFERS:u32=21; pub const SET_INDEX_BUFFER:u32=22;
    pub const SET_CBUFFERS:u32=23; pub const SET_SRVS:u32=24;
    pub const SET_SAMPLERS:u32=25; pub const SET_RENDER_TARGETS:u32=26;
    pub const SET_BLEND:u32=27; pub const SET_DEPTH_STENCIL:u32=28;
    pub const SET_RASTER:u32=29; pub const SET_VIEWPORTS:u32=30;
    pub const SET_SCISSORS:u32=31; pub const DRAW:u32=32;
    pub const CLEAR_STATE:u32=33; pub const COPY_REGION:u32=34;
    pub const COPY_RESOURCE:u32=35; pub const RESOLVE:u32=36;
    pub const CREATE_UAV:u32=37; pub const SET_UAVS:u32=38;
    pub const DISPATCH:u32=39; pub const CLEAR_UAV:u32=40;
    pub const GENERATE_MIPS:u32=41; pub const READBACK:u32=42;
    pub const COPY_COUNTER:u32=43; pub const CREATE_QUERY:u32=44;
    pub const BEGIN_QUERY:u32=45; pub const END_QUERY:u32=46;
    pub const UPLOAD_BUFFER_NO_OVERWRITE:u32=47;
    pub const SCENE_DONE:u32=64;
}
pub type GpuBackend<M> = Box<dyn FnOnce(GpuRing<M>) + Send>;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u32)]
pub enum State { Starting=0, Running=1, Failed=2 }
#[derive(Clone,Copy)]
pub struct Command<'a> { pub op:u32, pub payload:&'a [u32] }
impl<'a> Command<'a> {
    pub fn w(&self,i:usize)->u32 { self.payload.get(i).copied().unwrap_or(0) }
    pub fn f(&self,i:usize)->f32 { f32::from_bits(self.w(i)) }
    pub fn addr(&self,i:usize)->u64 { self.w(i) as u64 | (self.w(i+1) as u64)<<32 }
    pub fn bytes_from(&self,i:usize)->&'a [u8] {
        let w=&self.payload[i.min(self.payload.len())..];
        unsafe {std::slice::from_raw_parts(w.as_ptr().cast(),w.len()*4)}
    }
}
#[derive(Clone,Copy)]
pub struct GpuRing<M:RingMemory> { mem:M, at:u64, size:u64 }
impl<M:RingMemory> GpuRing<M> {
    pub fn try_new(mem:M, at:u64, size:u64)->Result<Self> {
        ensure!(at.is_multiple_of(4) && size.is_multiple_of(4),"unaligned GpuRing region");
        ensure!(size>=u64::from(RING_DATA)+8 && size<=u64::from(u32::MAX),"invalid GpuRing size");
        at.checked_add(size).context("GpuRing address overflow")?;
        mem.check_range(at,size)?;
        ensure!((mem.ptr(at) as usize).is_multiple_of(4),"unaligned GpuRing mapping");
        Ok(Self{mem,at,size})
    }
    // Runtime validates the producer's region before construction, as before.
    pub fn new(mem:M,at:u64,size:u64)->Self {Self::try_new(mem,at,size).expect("validated GpuRing region")}
    pub fn mem(&self)->M {self.mem}
    pub fn size(&self)->u64 {self.size}
    pub fn base(&self)->u64 {self.at}
    fn word(&self,offset:u64)->&AtomicU32 {self.mem.atomic_u32(self.at+offset)}
    pub fn set_state(&self,state:State) {self.word(STATE).store(state as u32,Ordering::SeqCst);}
    pub fn add_errors(&self,n:u32) {self.word(ERRORS).fetch_add(n,Ordering::Relaxed);}
    pub fn reader(&self)->Reader<M> {
        self.word(TAIL).store(RING_DATA,Ordering::SeqCst);
        Reader{ring:*self,tail:RING_DATA,executed:0}
    }
}
pub struct Reader<M:RingMemory> {ring:GpuRing<M>,tail:u32,executed:u32}
impl<M:RingMemory> Reader<M> {
    pub fn ring(&self)->GpuRing<M> {self.ring}
    pub fn head(&self)->u32 {self.ring.word(HEAD).load(Ordering::Acquire)}
    pub fn is_empty(&self,head:u32)->bool {head==self.tail}
    fn validate_cursor(&self,head:u32)->Result<()> {
        let size=self.ring.size;
        ensure!(size>=u64::from(RING_DATA)+8,"GpuRing smaller than header");
        ensure!(size<=u64::from(u32::MAX),"GpuRing size exceeds cursor range");
        ensure!(head>=RING_DATA && u64::from(head)<=size,"GpuRing head outside range");
        ensure!(head.is_multiple_of(4),"GpuRing head not DWORD-aligned");
        ensure!(self.tail>=RING_DATA && u64::from(self.tail)<=size,"GpuRing tail outside range");
        ensure!(self.tail.is_multiple_of(4),"GpuRing tail not DWORD-aligned"); Ok(())
    }
    /// Same head/physical-end validation and WRAP handling as the runtime reader.
    /// The returned payload borrows the real mapping until release.
    pub fn next(&mut self,head:u32)->Result<Option<Command<'_>>> {
        self.validate_cursor(head)?;
        let size=self.ring.size; let head=u64::from(head);
        loop {
            let tail=u64::from(self.tail);
            if tail==head {return Ok(None);}
            ensure!(tail<size,"GpuRing tail reached end without WRAP");
            let segment_end=if tail<head {head} else {size};
            let available=segment_end.checked_sub(tail).context("invalid cursor order")?;
            ensure!(available>=8,"GpuRing command header truncated at {tail}");
            let base=self.ring.at.checked_add(tail).context("GpuRing base overflow")?;
            let op=self.ring.mem.u32(base); let n=self.ring.mem.u32(base+4);
            if op==op::WRAP {
                ensure!(tail>head,"GpuRing WRAP before published head");
                self.tail=RING_DATA;continue;
            }
            let payload_bytes=u64::from(n).checked_mul(4).context("payload length overflow")?;
            let command_bytes=8u64.checked_add(payload_bytes).context("command length overflow")?;
            let end=tail.checked_add(command_bytes).context("command end overflow")?;
            ensure!(end<=segment_end && end<=size,"GpuRing opcode {op} declares {n} words beyond available data");
            let at=base.checked_add(8).context("payload address overflow")?;
            let payload=unsafe {std::slice::from_raw_parts(self.ring.mem.ptr(at).cast::<u32>(),n as usize)};
            self.tail=u32::try_from(end).context("command exceeds cursor range")?;
            self.executed=self.executed.checked_add(1).context("executed counter overflow")?;
            return Ok(Some(Command{op,payload}));
        }
    }
    pub fn discard_to_head(&mut self,head:u32)->Result<()> {
        self.validate_cursor(head)?;self.tail=head;self.release();Ok(())
    }
    pub fn release(&mut self) {
        let tail=self.ring.word(TAIL);tail.store(self.tail,Ordering::SeqCst);
        self.ring.mem.notify(tail);
        self.ring.word(EXECUTED).fetch_add(std::mem::take(&mut self.executed),Ordering::Relaxed);
    }
    pub fn wait(&mut self,timeout:Duration) {
        self.release();let sleeping=self.ring.word(SLEEPING);
        sleeping.store(1,Ordering::SeqCst);let head=self.ring.word(HEAD);
        if head.load(Ordering::SeqCst)==self.tail {self.ring.mem.wait32(head,self.tail,timeout);}
        sleeping.store(0,Ordering::SeqCst);
    }
}

pub mod linear;
#[cfg(test)]
mod reader_tests {
    use super::*;
    use crate::linear::LinearMemory;
    fn memory()->&'static LinearMemory {Box::leak(Box::new(LinearMemory::new(256)))}
    #[test] fn reader_accepts_a_complete_command_and_borrows_original_bytes() {
        let m=memory();let r=GpuRing::new(m,32,128);m.store(96,op::CREATE_BUFFER);m.store(100,2);m.store(104,0x11223344);m.store(108,0x55667788);
        let mut reader=r.reader();let c=reader.next(80).unwrap().unwrap();
        assert_eq!(c.op,11);assert_eq!(c.payload,[0x11223344,0x55667788]);
        assert_eq!(c.payload.as_ptr() as usize,m.ptr(104) as usize);
        assert!(reader.next(80).unwrap().is_none());
    }
    #[test] fn reader_does_not_see_unpublished_bytes() {
        let m=memory();let r=GpuRing::new(m,0,128);m.store(64,op::DRAW);m.store(68,6);m.store(0,64);
        let mut rd=r.reader();assert!(rd.next(rd.head()).unwrap().is_none());
    }
    #[test] fn reader_rejects_truncated_header() {let m=memory();let mut rd=GpuRing::new(m,0,128).reader();assert!(rd.next(68).is_err());}
    #[test] fn reader_rejects_payload_past_published_head() {let m=memory();m.store(64,11);m.store(68,2);let mut rd=GpuRing::new(m,0,128).reader();assert!(rd.next(76).is_err());}
    #[test] fn reader_rejects_payload_past_physical_ring_end() {let m=memory();m.store(64,11);m.store(68,1);let mut rd=GpuRing::new(m,0,72).reader();assert!(rd.next(72).is_err());}
    #[test] fn reader_follows_wrap_without_restarting_shared_tail() {
        let m=memory();let mut rd=GpuRing::new(m,0,128).reader();m.store(64,99);m.store(68,0);m.store(88,op::WRAP);m.store(92,0);rd.tail=88;
        let c=rd.next(72).unwrap().unwrap();assert_eq!(c.op,99);assert_eq!(c.payload.as_ptr() as usize,m.ptr(72) as usize);
        rd.release();assert_eq!(m.load(4),72);assert_eq!(m.load(12),1);
    }
    #[test] fn reader_rejects_wrap_before_published_head() {let m=memory();m.store(64,op::WRAP);let mut rd=GpuRing::new(m,0,128).reader();assert!(rd.next(72).is_err());}
    #[test] fn discard_to_head_releases_space_without_parsing_commands() {let m=memory();let mut rd=GpuRing::new(m,0,128).reader();rd.discard_to_head(80).unwrap();assert_eq!(m.load(4),80);assert!(rd.next(80).unwrap().is_none());}
    #[test] fn reader_rejects_head_outside_ring() {let m=memory();let mut rd=GpuRing::new(m,0,128).reader();assert!(rd.next(132).is_err());assert!(rd.next(65).is_err());}
    #[test] fn checked_fence_notification_validates_alignment_and_memory_bounds() {
        let m=memory();m.store_notify_checked(64,0xaabbccdd).unwrap();assert_eq!(m.load(64),0xaabbccdd);
        assert!(m.store_notify_checked(66,1).is_err());assert!(m.store_notify_checked(256,1).is_err());assert!(m.store_notify_checked(u64::MAX-1,1).is_err());
    }
    #[test] fn release_publishes_tail_and_counts_once() {let m=memory();m.store(64,op::NOP);m.store(68,0);let mut rd=GpuRing::new(m,0,128).reader();rd.next(72).unwrap();assert_eq!(m.load(4),64);rd.release();rd.release();assert_eq!(m.load(4),72);assert_eq!(m.load(12),1);}
    #[test] fn region_rejects_overflow_and_unaligned_base() {let m=memory();assert!(GpuRing::try_new(m,1,128).is_err());assert!(GpuRing::try_new(m,0,68).is_err());assert!(GpuRing::try_new(m,200,128).is_err());}
}
