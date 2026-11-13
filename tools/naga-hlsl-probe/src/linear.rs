//! Fixed host linear memory, useful for standalone hosts and transport tests.
//! The probe leaks its small allocation for process lifetime, matching the
//! RingMemory mapping-lifetime contract. No GPU behaviour is simulated here.
use std::sync::{Condvar,Mutex,atomic::{AtomicU32,AtomicU64,Ordering}};
use std::time::Duration;
use anyhow::{Result,ensure};
use crate::RingMemory;
pub struct LinearMemory {
    words:Box<[AtomicU32]>, gate:Mutex<()>, changed:Condvar,
    pub notifications:AtomicU64, pub failures:Mutex<Vec<String>>,
}
impl LinearMemory {
    pub fn new(bytes:usize)->Self {
        assert!(bytes.is_multiple_of(4));
        Self{words:(0..bytes/4).map(|_|AtomicU32::new(0)).collect(),
            gate:Mutex::new(()),changed:Condvar::new(),notifications:AtomicU64::new(0),failures:Mutex::new(vec![])}
    }
    pub fn load(&self,at:u64)->u32 {assert!(at.is_multiple_of(4));self.words[at as usize/4].load(Ordering::Acquire)}
    pub fn store(&self,at:u64,value:u32) {assert!(at.is_multiple_of(4));self.words[at as usize/4].store(value,Ordering::Release);}
}
unsafe impl RingMemory for &'static LinearMemory {
    fn check_range(self,at:u64,len:u64)->Result<()> {
        ensure!(at.checked_add(len).is_some_and(|end|end<=self.words.len() as u64*4),"linear memory range out of bounds");Ok(())
    }
    fn ptr(self,at:u64)->*mut u8 {
        self.check_range(at,0).expect("linear pointer bounds");
        unsafe {(self.words.as_ptr() as *mut u8).add(at as usize)}
    }
    fn notify(self,_word:&AtomicU32) {
        let _guard=self.gate.lock().unwrap();
        self.notifications.fetch_add(1,Ordering::Relaxed);self.changed.notify_all();
    }
    fn wait32(self,word:&AtomicU32,value:u32,timeout:Duration) {
        let guard=self.gate.lock().unwrap();
        let _guard=self.changed.wait_timeout_while(guard,timeout,|_|word.load(Ordering::SeqCst)==value).unwrap();
    }
    fn backend_failed(self,reason:&str) {self.failures.lock().unwrap().push(reason.into());}
}
