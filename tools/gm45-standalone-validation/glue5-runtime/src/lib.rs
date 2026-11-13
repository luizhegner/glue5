//! Test double for the public glue5-runtime GPU ring API.
//! The command queue and notifications below are only for the local GM45
//! contract probe; production builds use the real runtime crate.

pub mod exit {
    use std::sync::atomic::{AtomicBool, Ordering};

    static FAILED: AtomicBool = AtomicBool::new(false);

    pub fn fail(_why: impl Into<String>) {
        FAILED.store(true, Ordering::Release);
    }

    pub fn failed() -> bool {
        FAILED.load(Ordering::Acquire)
    }

    pub fn reset() {
        FAILED.store(false, Ordering::Release);
    }
}

pub mod gpu {
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    };
    use std::time::Duration;

    pub mod op {
        pub const NOP: u32 = 0;
        pub const WRAP: u32 = 1;
        pub const INIT: u32 = 2;
        pub const CREATE_TEXTURE: u32 = 3;
        pub const LOG: u32 = 10;
        pub const SCENE_DONE: u32 = 64;
        pub const DESTROY_OBJECT: u32 = 4;
        pub const CLEAR_RT: u32 = 5;
        pub const FENCE: u32 = 8;
        pub const CREATE_BUFFER: u32 = 11;
        pub const UPLOAD_BUFFER: u32 = 12;
        pub const UPLOAD_BUFFER_NO_OVERWRITE: u32 = 47;
        pub const UPLOAD_TEXTURE: u32 = 13;
        pub const CREATE_SRV: u32 = 17;
        pub const SET_SRVS: u32 = 24;
        pub const CREATE_UAV: u32 = 37;
        pub const SET_UAVS: u32 = 38;
        pub const CREATE_STATE: u32 = 16;
        pub const SET_SAMPLERS: u32 = 25;
    }

    pub type GpuBackend = Box<dyn FnOnce(GpuRing) + Send>;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum State {
        Starting = 0,
        Running = 1,
        Failed = 2,
    }

    #[derive(Clone, Copy)]
    pub struct Command<'a> {
        pub op: u32,
        pub payload: &'a [u32],
    }

    impl<'a> Command<'a> {
        pub fn w(&self, i: usize) -> u32 {
            self.payload.get(i).copied().unwrap_or(0)
        }

        pub fn f(&self, i: usize) -> f32 {
            f32::from_bits(self.w(i))
        }

        pub fn addr(&self, i: usize) -> u64 {
            self.w(i) as u64 | ((self.w(i + 1) as u64) << 32)
        }

        pub fn bytes_from(&self, i: usize) -> &'a [u8] {
            let words = &self.payload[i.min(self.payload.len())..];
            unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, words.len() * 4) }
        }
    }

    #[derive(Clone)]
    struct TestCommand {
        op: u32,
        payload: Vec<u32>,
    }

    static COMMANDS: OnceLock<Mutex<Vec<TestCommand>>> = OnceLock::new();
    static NOTIFICATIONS: OnceLock<Mutex<Vec<(u64, u32)>>> = OnceLock::new();
    static STATE: AtomicU32 = AtomicU32::new(State::Starting as u32);
    static ERRORS: AtomicU32 = AtomicU32::new(0);
    static EXECUTED: AtomicU32 = AtomicU32::new(0);
    static TAIL: AtomicU32 = AtomicU32::new(0);

    fn commands() -> &'static Mutex<Vec<TestCommand>> {
        COMMANDS.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn notifications() -> &'static Mutex<Vec<(u64, u32)>> {
        NOTIFICATIONS.get_or_init(|| Mutex::new(Vec::new()))
    }

    pub fn set_test_commands(commands_in: Vec<(u32, Vec<u32>)>) {
        *commands().lock().unwrap() = commands_in
            .into_iter()
            .map(|(op, payload)| TestCommand { op, payload })
            .collect();
        notifications().lock().unwrap().clear();
        STATE.store(State::Starting as u32, Ordering::SeqCst);
        ERRORS.store(0, Ordering::SeqCst);
        EXECUTED.store(0, Ordering::SeqCst);
        TAIL.store(0, Ordering::SeqCst);
        crate::exit::reset();
    }

    pub fn test_notifications() -> Vec<(u64, u32)> {
        notifications().lock().unwrap().clone()
    }

    pub fn test_state() -> State {
        match STATE.load(Ordering::Acquire) {
            1 => State::Running,
            2 => State::Failed,
            _ => State::Starting,
        }
    }

    pub fn test_errors() -> u32 {
        ERRORS.load(Ordering::Acquire)
    }

    pub fn test_executed() -> u32 {
        EXECUTED.load(Ordering::Acquire)
    }

    pub fn test_tail() -> u32 {
        TAIL.load(Ordering::Acquire)
    }

    #[derive(Clone, Copy)]
    pub struct Mem;

    impl Mem {
        pub fn store_notify(self, address: u64, value: u32) {
            notifications().lock().unwrap().push((address, value));
        }

        pub fn store_notify_checked(self, address: u64, value: u32) -> anyhow::Result<()> {
            anyhow::ensure!(address.is_multiple_of(4), "unaligned notification address");
            notifications().lock().unwrap().push((address, value));
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    pub struct GpuRing;

    impl GpuRing {
        pub fn mem(&self) -> Mem {
            Mem
        }

        pub fn set_state(&self, state: State) {
            STATE.store(state as u32, Ordering::SeqCst);
        }

        pub fn add_errors(&self, count: u32) {
            ERRORS.fetch_add(count, Ordering::Relaxed);
        }

        pub fn reader(&self) -> Reader {
            Reader {
                commands: commands().lock().unwrap().clone(),
                index: 0,
                executed: 0,
            }
        }
    }

    pub struct Reader {
        commands: Vec<TestCommand>,
        index: usize,
        executed: u32,
    }

    impl Reader {
        pub fn head(&self) -> u32 {
            self.commands.len() as u32
        }

        pub fn is_empty(&self, head: u32) -> bool {
            self.index >= head as usize
        }

        pub fn next(&mut self, head: u32) -> anyhow::Result<Option<Command<'_>>> {
            if self.index >= head as usize {
                return Ok(None);
            }
            let Some(command) = self.commands.get(self.index) else {
                anyhow::bail!("test reader index outside command list")
            };
            self.index += 1;
            self.executed += 1;
            Ok(Some(Command {
                op: command.op,
                payload: &command.payload,
            }))
        }

        pub fn discard_to_head(&mut self, head: u32) -> anyhow::Result<()> {
            self.index = self.index.max(head as usize).min(self.commands.len());
            self.release();
            Ok(())
        }

        pub fn release(&mut self) {
            TAIL.store(self.index as u32, Ordering::Release);
            EXECUTED.fetch_add(std::mem::take(&mut self.executed), Ordering::AcqRel);
        }

        pub fn wait(&mut self, timeout: Duration) {
            self.release();
            std::thread::sleep(timeout);
        }
    }
}
