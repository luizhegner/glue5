//! Servers on the local network, by Bonjour (mDNS service `_glue5._tcp`).
//!
//! A server announces itself and answers for itself on every system
//! ([`responder`]), beside the system's own responder (mDNSResponder, Avahi,
//! Windows' mDNS), whose APIs differ on each and are not on all.
//!
//! Finding: Apple systems ask the system's responder (`dns_sd`), as iOS and
//! tvOS need (no multicast entitlement, only the service in the app's
//! `NSBonjourServices`); elsewhere [`find`] sends one mDNS query from an
//! ordinary UDP port, which responders answer by unicast (RFC 6762, 6.7), so
//! this query path alone does not need multicast reception. Android still
//! holds a Wi-Fi multicast lock while the app is actively listening for LAN
//! discovery or receiving the game's UDP broadcast traffic.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

pub const SERVICE: &str = "_glue5._tcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The server's name (its computer's).
    pub name: String,
    /// What to connect to: `host:port`.
    pub address: String,
}

/// The servers that answer within `timeout`.
pub fn find(timeout: Duration) -> Vec<Found> {
    #[cfg(target_vendor = "apple")]
    return apple::find(timeout);
    #[cfg(not(target_vendor = "apple"))]
    return query(timeout).unwrap_or_default();
}

/// Announces a server on `port` until dropped.
pub struct Advert {
    _responder: responder::Responder,
}

pub fn advertise(port: u16) -> Option<Advert> {
    advertise_as(None, port)
}

/// [`advertise`] under `name` (the computer's name by default).
fn advertise_as(name: Option<&str>, port: u16) -> Option<Advert> {
    match responder::Responder::start(name, port) {
        Ok(r) => Some(Advert { _responder: r }),
        Err(e) => {
            log::warn!("mdns: no announcement: {e}");
            None
        }
    }
}

#[cfg(target_vendor = "apple")]
mod apple {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::time::{Duration, Instant};

    use super::{Found, SERVICE};

    type SdRef = *mut c_void;
    type BrowseReply =
        extern "C" fn(SdRef, u32, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);
    type ResolveReply =
        extern "C" fn(SdRef, u32, u32, i32, *const c_char, *const c_char, u16, u16, *const u8, *mut c_void);

    // libSystem's dns_sd.
    unsafe extern "C" {
        fn DNSServiceBrowse(
            sd: *mut SdRef,
            flags: u32,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            reply: BrowseReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceResolve(
            sd: *mut SdRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            reply: ResolveReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd: SdRef) -> i32;
        fn DNSServiceProcessResult(sd: SdRef) -> i32;
        fn DNSServiceRefDeallocate(sd: SdRef);
    }

    const ADD: u32 = 0x2;

    /// A `DNSServiceRef`, deallocated when dropped.
    pub struct Ref(SdRef);

    unsafe impl Send for Ref {}
    unsafe impl Sync for Ref {}

    impl Drop for Ref {
        fn drop(&mut self) {
            unsafe { DNSServiceRefDeallocate(self.0) }
        }
    }

    /// Processes `sd`'s answers until `until`, or until `done` says so.
    fn pump(sd: SdRef, until: Instant, done: &dyn Fn() -> bool) {
        let fd = unsafe { DNSServiceRefSockFD(sd) };
        while !done() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut p, 1, left.as_millis().max(1) as i32) };
            if ready <= 0 || unsafe { DNSServiceProcessResult(sd) } != 0 {
                return;
            }
        }
    }

    struct Service {
        name: String,
        regtype: CString,
        domain: CString,
        interface: u32,
    }

    extern "C" fn browsed(
        _: SdRef,
        flags: u32,
        interface: u32,
        err: i32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        context: *mut c_void,
    ) {
        if err != 0 || flags & ADD == 0 {
            return;
        }
        let found = unsafe { &mut *(context as *mut Vec<Service>) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) };
        let name = s(name).to_string_lossy().into_owned();
        if found.iter().any(|f| f.name == name) {
            return;
        }
        found.push(Service { name, regtype: s(regtype).into(), domain: s(domain).into(), interface });
    }

    extern "C" fn resolved(
        _: SdRef,
        _: u32,
        _: u32,
        err: i32,
        _: *const c_char,
        host: *const c_char,
        port: u16,
        _: u16,
        _: *const u8,
        context: *mut c_void,
    ) {
        if err != 0 {
            return;
        }
        let out = unsafe { &mut *(context as *mut Option<String>) };
        let host = unsafe { CStr::from_ptr(host) }.to_string_lossy().trim_end_matches('.').to_string();
        *out = Some(format!("{host}:{}", u16::from_be(port)));
    }

    pub fn find(timeout: Duration) -> Vec<Found> {
        let until = Instant::now() + timeout;
        let regtype = CString::new(SERVICE).unwrap();
        let mut services: Vec<Service> = Vec::new();
        let mut sd: SdRef = std::ptr::null_mut();
        let ctx = &mut services as *mut Vec<Service> as *mut c_void;
        if unsafe { DNSServiceBrowse(&mut sd, 0, 0, regtype.as_ptr(), std::ptr::null(), browsed, ctx) } != 0 {
            return Vec::new();
        }
        let browse = Ref(sd);
        // Most answers come at once; the rest of the time goes to resolving.
        pump(browse.0, Instant::now() + timeout / 2, &|| false);
        drop(browse);
        let mut found = Vec::new();
        for s in &services {
            let name = CString::new(s.name.clone()).unwrap_or_default();
            let mut address: Option<String> = None;
            let ctx = &mut address as *mut Option<String> as *mut c_void;
            let mut sd: SdRef = std::ptr::null_mut();
            let err = unsafe {
                DNSServiceResolve(&mut sd, 0, s.interface, name.as_ptr(), s.regtype.as_ptr(), s.domain.as_ptr(), resolved, ctx)
            };
            if err != 0 {
                continue;
            }
            let resolve = Ref(sd);
            let deadline = until.max(Instant::now() + Duration::from_millis(500));
            pump(resolve.0, deadline, &|| unsafe { (*(ctx as *const Option<String>)).is_some() });
            drop(resolve);
            if let Some(address) = address
                && !found.iter().any(|f: &Found| f.address == address)
            {
                found.push(Found { name: s.name.clone(), address });
            }
        }
        found
    }
}

/// A Multicast DNS responder for one service instance (RFC 6762, 6763):
/// joins the mDNS group beside the system's own responder, announces the
/// instance on each network it is on (with that network's address), answers
/// the questions about it (by multicast, or by unicast to legacy and
/// unicast-response queries), and says goodbye when dropped. No probing for
/// a unique name: the instance is named after the computer (and the port,
/// if not the usual one), its host `glue5-<name>.local`.
pub(crate) mod responder {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use super::{SERVICE, put_name, read_name};

    const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
    const MDNS: u16 = 5353;
    const A: u16 = 1;
    const PTR: u16 = 12;
    const TXT: u16 = 16;
    const AAAA: u16 = 28;
    const SRV: u16 = 33;
    const NSEC: u16 = 47;
    const ANY: u16 = 255;
    /// Class IN, with the cache-flush bit for records only this host has.
    const IN: u16 = 1;
    const FLUSH: u16 = 0x8000;
    /// Record lifetimes: names that point at others, and host records.
    const LONG: u32 = 4500;
    const SHORT: u32 = 120;
    /// How often the host's addresses are looked at again (a laptop moves
    /// to another network).
    const RECHECK: Duration = Duration::from_secs(30);

    pub struct Responder {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Responder {
        /// Starts answering for an instance called `name` (the computer's
        /// name by default) on `port`.
        pub fn start(name: Option<&str>, port: u16) -> io::Result<Responder> {
            let mut name = name.map(str::to_owned).unwrap_or_else(computer_name);
            // Two servers on one computer (the app and glue5-serve).
            if port != crate::PORT {
                name = format!("{name} ({port})");
            }
            let me = Instance::new(name, port, crate::lan_addresses());
            let socket = socket(&me.addresses)?;
            let stop = Arc::new(AtomicBool::new(false));
            let s = stop.clone();
            let thread = std::thread::Builder::new().name("mdns".into()).spawn(move || {
                let mut me = me;
                serve(&socket, &mut me, &s)
            })?;
            Ok(Responder { stop, thread: Some(thread) })
        }
    }

    impl Drop for Responder {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// The service instance: its name, its host's name, its port, and the
    /// host's addresses on the networks it is on.
    pub(crate) struct Instance {
        name: String,
        host: String,
        port: u16,
        addresses: Vec<Ipv4Addr>,
    }

    impl Instance {
        pub(crate) fn new(name: String, port: u16, addresses: Vec<Ipv4Addr>) -> Instance {
            // A host name of its own: the system's responder answers for the
            // computer's.
            let words: Vec<String> = name
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect();
            let mut host = format!("glue5-{}", if words.is_empty() { "server".to_owned() } else { words.join("-") });
            host.truncate(63);
            // A label holds at most 63 bytes, and no dot (it would split it).
            let mut name = name.replace('.', "-");
            while name.len() > 63 {
                name.pop();
            }
            Instance { name, host, port, addresses }
        }

        fn full(&self) -> [&str; 4] {
            [&self.name, "_glue5", "_tcp", "local"]
        }

        /// The addresses to give a querier at `from`: those on its network
        /// (a /24, as at home), else all.
        /// Whether `from` is on one of this device's networks (its /24): a
        /// network of public addresses (a campus's) is a local one too.
        fn on_link(&self, from: SocketAddr) -> bool {
            let IpAddr::V4(ip) = from.ip() else { return false };
            self.addresses.iter().any(|a| a.octets()[..3] == ip.octets()[..3])
        }

        fn near(&self, from: SocketAddr) -> Vec<Ipv4Addr> {
            let IpAddr::V4(ip) = from.ip() else { return self.addresses.clone() };
            let same: Vec<_> = self.addresses.iter().copied().filter(|a| a.octets()[..3] == ip.octets()[..3]).collect();
            if same.is_empty() { self.addresses.clone() } else { same }
        }
    }

    /// The computer's name: as the system shows it on a Mac ("Alex's
    /// MacBook Pro"), its host name without a domain elsewhere.
    fn computer_name() -> String {
        #[cfg(target_os = "macos")]
        if let Some(name) = mac_computer_name() {
            return name;
        }
        #[cfg(unix)]
        let name = {
            let mut buf = [0u8; 256];
            let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
            let end = buf.iter().position(|&b| b == 0).unwrap_or(0);
            ok.then(|| String::from_utf8_lossy(&buf[..end]).into_owned())
        };
        #[cfg(not(unix))]
        let name = std::env::var("COMPUTERNAME").ok();
        name.map(|n| n.split('.').next().unwrap_or_default().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Glue 5".to_owned())
    }

    /// The name of System Settings › General › Sharing.
    #[cfg(target_os = "macos")]
    fn mac_computer_name() -> Option<String> {
        use std::ffi::{c_char, c_void};
        #[link(name = "SystemConfiguration", kind = "framework")]
        unsafe extern "C" {
            fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *const c_void;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFStringGetCString(s: *const c_void, buf: *mut c_char, len: isize, encoding: u32) -> u8;
            fn CFRelease(cf: *const c_void);
        }
        const UTF8: u32 = 0x0800_0100;
        unsafe {
            let s = SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut());
            if s.is_null() {
                return None;
            }
            let mut buf = [0 as c_char; 256];
            let ok = CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0;
            CFRelease(s);
            ok.then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()).filter(|n| !n.is_empty())
        }
    }

    /// Port 5353, shared with the system's responder, in the mDNS group on
    /// each of these interfaces.
    fn socket(addresses: &[Ipv4Addr]) -> io::Result<UdpSocket> {
        use socket2::{Domain, Protocol, Socket, Type};
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        // Shared on Linux and Windows by the address's reuse (Avahi, which
        // runs as another user, sets that); on Apple systems mDNSResponder
        // sets the port's.
        s.set_reuse_address(true)?;
        #[cfg(target_vendor = "apple")]
        s.set_reuse_port(true)?;
        s.bind(&SocketAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS)).into())?;
        s.set_multicast_ttl_v4(255)?;
        let mut joined = false;
        for ip in addresses {
            joined |= s.join_multicast_v4(&GROUP, ip).is_ok();
        }
        if !joined {
            s.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
        }
        s.set_read_timeout(Some(Duration::from_millis(500)))?;
        Ok(s.into())
    }

    fn serve(socket: &UdpSocket, me: &mut Instance, stop: &AtomicBool) {
        log::info!("mdns: announcing \"{}\" on port {} ({}.local)", me.name, me.port, me.host);
        let all = Asked { ptr: true, srv: true, txt: true, a: true, no_aaaa: false };
        // Announced twice, a second apart (RFC 6762, 8.3).
        let twice = || vec![Instant::now(), Instant::now() + Duration::from_secs(1)];
        let mut announce = twice();
        let mut checked = Instant::now();
        let mut buf = [0u8; 9000];
        while !stop.load(Ordering::Acquire) {
            // On other networks now: the old addresses said goodbye to, the
            // group joined where the host is, announced there anew.
            if checked.elapsed() > RECHECK {
                checked = Instant::now();
                let now = crate::lan_addresses();
                if !same_addresses(&now, &me.addresses) {
                    log::info!("mdns: addresses now {now:?} (were {:?})", me.addresses);
                    multicast(socket, me, |addresses| reply(&all, me, addresses, Some(&Query::goodbye())));
                    for ip in me.addresses.iter().filter(|ip| !now.contains(ip)) {
                        let _ = socket.leave_multicast_v4(&GROUP, ip);
                    }
                    for ip in now.iter().filter(|ip| !me.addresses.contains(ip)) {
                        let _ = socket.join_multicast_v4(&GROUP, ip);
                    }
                    me.addresses = now;
                    announce = twice();
                }
            }
            if announce.first().is_some_and(|t| t.elapsed() > Duration::ZERO) {
                announce.remove(0);
                multicast(socket, me, |addresses| reply(&all, me, addresses, None));
            }
            let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
            // Only the local network's (RFC 6762, 11): a query from elsewhere
            // (spoofed, say) would make this a reflector, and learn the
            // device's name and addresses.
            if !on_local_network(from.ip()) && !me.on_link(from) {
                continue;
            }
            let Some(query) = Query::read(&buf[..n], from, me) else { continue };
            if query.unicast {
                let _ = socket.send_to(&reply(&query.asked, me, &me.near(from), Some(&query)), from);
            } else {
                multicast(socket, me, |addresses| reply(&query.asked, me, addresses, None));
            }
        }
        // Goodbye: the records with a lifetime of 0 (RFC 6762, 10.1).
        multicast(socket, me, |addresses| reply(&all, me, addresses, Some(&Query::goodbye())));
    }

    /// Whether `ip` is a local network's: private, link-local, this device,
    /// or a VPN's shared space (100.64/10, Tailscale's).
    fn on_local_network(ip: std::net::IpAddr) -> bool {
        match ip {
            std::net::IpAddr::V4(v) => {
                v.is_private()
                    || v.is_link_local()
                    || v.is_loopback()
                    || (v.octets()[0] == 100 && v.octets()[1] & 0xc0 == 64)
            }
            std::net::IpAddr::V6(v) => v.is_loopback() || v.is_unicast_link_local(),
        }
    }

    /// The same addresses, in any order.
    fn same_addresses(a: &[Ipv4Addr], b: &[Ipv4Addr]) -> bool {
        a.len() == b.len() && a.iter().all(|ip| b.contains(ip))
    }

    /// Sends to the group from each interface, with that interface's address
    /// (a computer on Ethernet and Wi-Fi is reached on both).
    fn multicast(socket: &UdpSocket, me: &Instance, message: impl Fn(&[Ipv4Addr]) -> Vec<u8>) {
        let group = SocketAddrV4::new(GROUP, MDNS);
        let s = socket2::SockRef::from(socket);
        if me.addresses.is_empty() {
            let _ = socket.send_to(&message(&[]), group);
        }
        for ip in &me.addresses {
            if s.set_multicast_if_v4(ip).is_ok() {
                let _ = socket.send_to(&message(&[*ip]), group);
            }
        }
    }

    /// What a query asks about the instance.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Asked {
        ptr: bool,
        srv: bool,
        txt: bool,
        a: bool,
        /// An IPv6 address of the host: there is none (said by NSEC).
        no_aaaa: bool,
    }

    /// A query about the instance.
    pub(crate) struct Query {
        asked: Asked,
        /// Answered to the querier rather than the group.
        unicast: bool,
        /// From another port than mDNS's: a simple resolver's, answered with
        /// its id and question, short lifetimes and no cache flushing (RFC
        /// 6762, 6.7).
        legacy: Option<(u16, u16, Vec<u8>)>,
        /// Records with a lifetime of 0 (the goodbye).
        gone: bool,
    }

    impl Query {
        fn goodbye() -> Query {
            Query { asked: Asked { ptr: true, srv: true, txt: true, a: true, no_aaaa: false }, unicast: false, legacy: None, gone: true }
        }

        /// The query in `msg` from `from`, if it asks about the instance.
        pub(crate) fn read(msg: &[u8], from: SocketAddr, me: &Instance) -> Option<Query> {
            let field = |at: usize| msg.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
            let (id, flags, count) = (field(0)?, field(2)?, field(4)?);
            if flags & 0x8000 != 0 {
                return None;
            }
            let legacy = from.port() != MDNS;
            let service = format!("{SERVICE}.local");
            let full = format!("{}.{SERVICE}.local", me.name).to_ascii_lowercase();
            let host = format!("{}.local", me.host);
            let mut asked = Asked { ptr: false, srv: false, txt: false, a: false, no_aaaa: false };
            let (mut unicast, mut at) = (legacy, 12);
            for _ in 0..count {
                let (name, next) = read_name(msg, at)?;
                let (kind, class) = (field(next)?, field(next + 2)?);
                at = next + 4;
                unicast |= class & 0x8000 != 0;
                let name = name.to_ascii_lowercase();
                let wants = |k| kind == k || kind == ANY;
                if name == service {
                    asked.ptr |= wants(PTR);
                } else if name == full {
                    asked.srv |= wants(SRV);
                    asked.txt |= wants(TXT);
                } else if name == host {
                    asked.a |= wants(A);
                    asked.no_aaaa |= wants(AAAA);
                }
            }
            if asked == (Asked { ptr: false, srv: false, txt: false, a: false, no_aaaa: false }) {
                return None;
            }
            let legacy = legacy.then(|| (id, count, msg[12..at].to_vec()));
            Some(Query { asked, unicast, legacy, gone: false })
        }
    }

    /// The reply to what was asked, with these addresses: the answers, and
    /// what a browser needs next as additional records. `query`: a legacy
    /// query's id and question, or the goodbye.
    fn reply(asked: &Asked, me: &Instance, addresses: &[Ipv4Addr], query: Option<&Query>) -> Vec<u8> {
        let legacy = query.and_then(|q| q.legacy.as_ref());
        let gone = query.is_some_and(|q| q.gone);
        let (ttl, flush) = match (legacy, gone) {
            (_, true) => (0, FLUSH),
            (Some(_), _) => (10, 0),
            (None, _) => (SHORT, FLUSH),
        };
        let long = if legacy.is_some() || gone { ttl } else { LONG };
        let mut m = Message::default();
        if let Some((id, count, questions)) = legacy {
            (m.id, m.questions, m.question_bytes) = (*id, *count, questions.clone());
        }
        let service = asked.ptr || asked.srv || asked.txt;
        if service {
            m.record(asked.ptr, |r| r.ptr(me, long));
            m.record(asked.srv, |r| r.srv(me, ttl, flush));
            m.record(asked.txt, |r| r.txt(me, long, flush));
        }
        for &ip in addresses {
            m.record(asked.a, |r| r.a(me, ip, ttl, flush));
        }
        // The host has no IPv6 address: said, so that a resolver does not
        // wait for one (RFC 6762, 6.1).
        if (asked.no_aaaa || asked.a || service) && !gone {
            m.record(asked.no_aaaa && !asked.a, |r| r.nsec(me, ttl, flush));
        }
        m.finish()
    }

    /// A response being written: its records as answers or additional ones.
    #[derive(Default)]
    struct Message {
        id: u16,
        questions: u16,
        question_bytes: Vec<u8>,
        answers: Vec<Vec<u8>>,
        additional: Vec<Vec<u8>>,
    }

    impl Message {
        fn record(&mut self, answer: bool, f: impl FnOnce(&mut Record)) {
            let mut r = Record(Vec::new());
            f(&mut r);
            if answer { self.answers.push(r.0) } else { self.additional.push(r.0) }
        }

        fn finish(self) -> Vec<u8> {
            let mut out = Vec::with_capacity(512);
            for n in [self.id, 0x8400, self.questions, self.answers.len() as u16, 0, self.additional.len() as u16] {
                out.extend_from_slice(&n.to_be_bytes());
            }
            out.extend_from_slice(&self.question_bytes);
            for r in self.answers.iter().chain(&self.additional) {
                out.extend_from_slice(r);
            }
            out
        }
    }

    struct Record(Vec<u8>);

    fn labels(out: &mut Vec<u8>, name: &[&str]) {
        for label in name {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
    }

    impl Record {
        fn head(&mut self, name: &[&str], kind: u16, class: u16, ttl: u32) {
            labels(&mut self.0, name);
            self.0.extend_from_slice(&kind.to_be_bytes());
            self.0.extend_from_slice(&class.to_be_bytes());
            self.0.extend_from_slice(&ttl.to_be_bytes());
        }

        fn data(&mut self, data: &[u8]) {
            self.0.extend_from_slice(&(data.len() as u16).to_be_bytes());
            self.0.extend_from_slice(data);
        }

        fn ptr(&mut self, me: &Instance, ttl: u32) {
            self.head(&["_glue5", "_tcp", "local"], PTR, IN, ttl);
            let mut target = Vec::new();
            labels(&mut target, &me.full());
            self.data(&target);
        }

        fn srv(&mut self, me: &Instance, ttl: u32, flush: u16) {
            self.head(&me.full(), SRV, IN | flush, ttl);
            let mut data = vec![0, 0, 0, 0];
            data.extend_from_slice(&me.port.to_be_bytes());
            put_name(&mut data, &format!("{}.local", me.host));
            self.data(&data);
        }

        fn txt(&mut self, me: &Instance, ttl: u32, flush: u16) {
            self.head(&me.full(), TXT, IN | flush, ttl);
            self.data(&[0]);
        }

        fn a(&mut self, me: &Instance, ip: Ipv4Addr, ttl: u32, flush: u16) {
            self.head(&[&me.host, "local"], A, IN | flush, ttl);
            self.data(&ip.octets());
        }

        /// The host's only record is its address (type A).
        fn nsec(&mut self, me: &Instance, ttl: u32, flush: u16) {
            self.head(&[&me.host, "local"], NSEC, IN | flush, ttl);
            let mut data = Vec::new();
            labels(&mut data, &[&me.host, "local"]);
            data.extend_from_slice(&[0, 1, 0x40]);
            self.data(&data);
        }
    }

    /// The bytes of the reply to `query` from `from`, and where it goes
    /// (none: to the group), for tests.
    #[cfg(test)]
    pub(crate) fn answer(query: &[u8], from: SocketAddr, me: &Instance) -> Option<(Vec<u8>, Option<SocketAddr>)> {
        let q = Query::read(query, from, me)?;
        let addresses = if q.unicast { me.near(from) } else { me.addresses.clone() };
        Some((reply(&q.asked, me, &addresses, Some(&q)), q.unicast.then_some(from)))
    }
}

/// One mDNS question for the service from an ordinary port; the answers
/// (pointer, service and address records) come back by unicast.
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
fn query(timeout: Duration) -> std::io::Result<Vec<Found>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let _ = socket.set_multicast_ttl_v4(255);
    let mut msg = vec![0x47, 0x35, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    put_name(&mut msg, &format!("{SERVICE}.local"));
    msg.extend_from_slice(&[0, 12, 0, 1]);
    socket.send_to(&msg, SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353))?;
    let until = Instant::now() + timeout;
    let (mut ptrs, mut srvs, mut addrs) = (Vec::new(), Vec::new(), Vec::new());
    let mut buf = [0u8; 9000];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left))?;
        let Ok((n, from)) = socket.recv_from(&mut buf) else { break };
        parse(&buf[..n], from, &mut ptrs, &mut srvs, &mut addrs);
    }
    let mut found = Vec::new();
    for instance in ptrs {
        let Some((_, port, target)) = srvs.iter().find(|(i, _, _)| *i == instance) else { continue };
        let host = addrs.iter().find(|(h, _)| h == target).map_or_else(|| target.clone(), |(_, ip)| ip.to_string());
        let name = instance.split('.').next().unwrap_or(&instance).to_string();
        let address = format!("{host}:{port}");
        if !found.iter().any(|f: &Found| f.address == address) {
            found.push(Found { name, address });
        }
    }
    Ok(found)
}

fn put_name(msg: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0);
}

/// A name at `at` (with compression), and the offset after it.
fn read_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let (mut parts, mut end, mut jumps) = (Vec::new(), None, 0);
    loop {
        let len = *msg.get(at)? as usize;
        if len & 0xc0 == 0xc0 {
            let ptr = (len & 0x3f) << 8 | *msg.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            jumps += 1;
            if jumps > 32 {
                return None;
            }
            at = ptr;
        } else if len == 0 {
            return Some((parts.join("."), end.unwrap_or(at + 1)));
        } else {
            parts.push(String::from_utf8_lossy(msg.get(at + 1..at + 1 + len)?).into_owned());
            at += 1 + len;
        }
    }
}

type Srv = (String, u16, String);

fn parse(msg: &[u8], from: SocketAddr, ptrs: &mut Vec<String>, srvs: &mut Vec<Srv>, addrs: &mut Vec<(String, Ipv4Addr)>) {
    let count = |i: usize| msg.get(i..i + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let (questions, records) = (count(4), count(6) + count(8) + count(10));
    let mut at = 12;
    for _ in 0..questions {
        let Some((_, next)) = read_name(msg, at) else { return };
        at = next + 4;
    }
    for _ in 0..records {
        let Some((name, next)) = read_name(msg, at) else { return };
        let Some(head) = msg.get(next..next + 10) else { return };
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let data = next + 10;
        let Some(rdata) = msg.get(data..data + len) else { return };
        let service = format!("{SERVICE}.local").to_ascii_lowercase();
        match kind {
            12 if name.eq_ignore_ascii_case(&service) => {
                if let Some((instance, _)) = read_name(msg, data)
                    && !ptrs.contains(&instance)
                {
                    ptrs.push(instance);
                }
            }
            33 if rdata.len() >= 7 => {
                let port = u16::from_be_bytes([rdata[4], rdata[5]]);
                if let Some((target, _)) = read_name(msg, data + 6) {
                    srvs.push((name, port, target));
                }
            }
            1 if rdata.len() == 4 => addrs.push((name, Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]))),
            _ => {}
        }
        at = data + len;
    }
    // A responder that sent only a pointer: its sender is the server.
    if srvs.is_empty() && !ptrs.is_empty() && let SocketAddr::V4(a) = from {
        for p in ptrs.iter() {
            srvs.push((p.clone(), crate::PORT, a.ip().to_string()));
        }
    }
}

#[test]
fn names() {
    let mut msg = vec![0u8; 12];
    put_name(&mut msg, "_glue5._tcp.local");
    let at = msg.len();
    msg.extend_from_slice(&[3, b'M', b'a', b'c', 0xc0, 12]);
    assert_eq!(read_name(&msg, 12).unwrap().0, "_glue5._tcp.local");
    assert_eq!(read_name(&msg, at).unwrap(), ("Mac._glue5._tcp.local".to_string(), at + 6));
}

/// The client's query, answered by the responder: what `find` reads from
/// it is the server.
#[test]
fn the_responder_answers_the_query_of_find() {
    let me = responder::Instance::new(
        "Desktop PC".into(),
        47100,
        vec![Ipv4Addr::new(100, 64, 0, 7), Ipv4Addr::new(192, 168, 1, 20)],
    );
    let mut query = vec![0x47, 0x35, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    put_name(&mut query, &format!("{SERVICE}.local"));
    query.extend_from_slice(&[0, 12, 0, 1]);
    let from: SocketAddr = "192.168.1.30:53000".parse().unwrap();
    let (reply, to) = responder::answer(&query, from, &me).expect("an answer");
    assert_eq!(to, Some(from), "a legacy query is answered by unicast");
    assert_eq!(&reply[..2], &[0x47, 0x35], "with the query's id");
    let (mut ptrs, mut srvs, mut addrs) = (Vec::new(), Vec::new(), Vec::new());
    parse(&reply, from, &mut ptrs, &mut srvs, &mut addrs);
    assert_eq!(ptrs, ["Desktop PC._glue5._tcp.local"]);
    assert_eq!(srvs, [("Desktop PC._glue5._tcp.local".to_string(), 47100, "glue5-desktop-pc.local".to_string())]);
    assert_eq!(addrs, [("glue5-desktop-pc.local".to_string(), Ipv4Addr::new(192, 168, 1, 20))]);

    // An mDNS querier's question (from port 5353): to the group; others
    // are not answered.
    let from: SocketAddr = "192.168.1.30:5353".parse().unwrap();
    assert!(matches!(responder::answer(&query, from, &me), Some((_, None))));
    let mut other = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    put_name(&mut other, "_airplay._tcp.local");
    other.extend_from_slice(&[0, 12, 0, 1]);
    assert!(responder::answer(&other, from, &me).is_none());

    // The host's IPv6 address: none, said with an NSEC record (type 47).
    let mut aaaa = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    put_name(&mut aaaa, "glue5-desktop-pc.local");
    aaaa.extend_from_slice(&[0, 28, 0, 1]);
    let (reply, _) = responder::answer(&aaaa, from, &me).expect("an answer");
    assert_eq!(&reply[6..8], &[0, 1], "one answer");
    let (_, at) = read_name(&reply, 12).unwrap();
    assert_eq!(&reply[at..at + 2], &[0, 47]);
}

#[cfg(target_vendor = "apple")]
#[test]
fn finds_an_announced_server() {
    let port = 47_199;
    // Its own name: another server on the network has the computer's.
    let _advert = advertise_as(Some(&format!("glue5 test {}", std::process::id())), port).expect("announced");
    let found = find(Duration::from_secs(3));
    assert!(found.iter().any(|f| f.address.ends_with(&format!(":{port}"))), "found {found:?}");
    // The query other systems send.
    let found = query(Duration::from_secs(2)).unwrap();
    assert!(found.iter().any(|f| f.address.ends_with(&format!(":{port}"))), "the query found {found:?}");
}
