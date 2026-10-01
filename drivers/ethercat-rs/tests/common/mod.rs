//! A scripted EtherCAT ring, for the tests that need to decide what the slaves say and see what they were sent.
//!
//! `live_wire` runs the port against the Python driver's slave stack, which is the substrate. What it cannot do is *choose*: a
//! slave that refuses Safe-Op with a code, one that stays in Safe-Op without saying so, a mailbox that fills on the fourth poll
//! or never, a reply about another object, an encoder resolution of zero, a slave that drops off the ring between two cycles, a
//! working counter one too high. Each is a refusal in the master that the simulated drives never provoke. [`Ring`] is that ring,
//! and it is deliberately not clever: frames in, frames out, and a log of every frame it was sent, so a test can say what the
//! master wrote and in what order.
//!
//! It is used two ways. As a [`Segment`], a synchronous in-process ring that answers inside `cycle`, it needs no kernel: what
//! the master and the bridge do with each answer is decided by the test. And on a kernel interface, behind [`serve_on`], it is
//! the other end of the vtable's own `RawSocket`. **It agrees with whoever scripted it** — F15's point — and says nothing about
//! what a real ESC does. Every register, command and controlword below is a number from ETG.1000 or CiA 402 written out, and not
//! one of the crate's own constants, so a constant that is wrong in the crate is not wrong here too.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use skynet_ethercat_rs::frames::{Datagram, Frame};
use skynet_ethercat_rs::segment::{RawSocket, Segment, SegmentError};

/// ETG.1000.4's command codes.
pub const APWR: u8 = 0x02;
pub const FPRD: u8 = 0x04;
pub const FPWR: u8 = 0x05;
pub const BRD: u8 = 0x07;
pub const BWR: u8 = 0x08;
pub const LRW: u8 = 0x0C;

/// CiA 402's controlwords, the ones this driver sends.
pub const DISABLE_VOLTAGE: u16 = 0x0000;
pub const QUICK_STOP: u16 = 0x0002;
pub const SHUTDOWN: u16 = 0x0006;
pub const SWITCH_ON: u16 = 0x0007;
pub const ENABLE_OPERATION: u16 = 0x000F;
pub const FAULT_RESET: u16 = 0x0080;

/// AL states.
pub const INIT: u8 = 0x01;
pub const PRE_OP: u8 = 0x02;
pub const SAFE_OP: u8 = 0x04;
pub const OP: u8 = 0x08;

/// CiA 402's states as a scripted drive moves through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cia {
    SwitchOnDisabled,
    ReadyToSwitchOn,
    SwitchedOn,
    OperationEnabled,
    QuickStopActive,
    FaultReactionActive,
    Fault,
}

impl Cia {
    /// The statusword a drive in this state reports (CiA 402 table 3: bits 0-3, 5 and 6 carry the state).
    pub fn statusword(self) -> u16 {
        match self {
            Cia::SwitchOnDisabled => 0x0240,
            Cia::ReadyToSwitchOn => 0x0231,
            Cia::SwitchedOn => 0x0233,
            Cia::OperationEnabled => 0x0237,
            Cia::QuickStopActive => 0x0217,
            Cia::FaultReactionActive => 0x020F,
            Cia::Fault => 0x0208,
        }
    }
}

/// What a slave puts in its input mailbox when it is asked for an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The object's value, or an abort if the slave has no such object.
    Normal,
    /// An abort, with this code.
    Abort(u32),
    /// A reply about the neighbouring index, the one the master did not ask for.
    OtherObject,
    /// A full mailbox of zeros: the reply that never came.
    Zeros,
    /// Nothing, ever: the full bit never sets.
    Never,
}

/// One scripted slave: an ESC's registers the master touches, a mailbox, and a CiA 402 drive behind them.
#[derive(Debug, Clone)]
pub struct Slave {
    /// The configured station address, 0 until a master writes one.
    pub station: u16,
    /// The AL state it holds.
    pub al: u8,
    /// The AL status error bit, and the code behind it.
    pub al_error: bool,
    pub al_code: u16,
    /// Refuses a request for this state, with this code, and says so in the error bit.
    pub refuse: Option<(u8, u16)>,
    /// Never goes above this state and does not say so: the request is acknowledged by nobody.
    pub stuck_at: Option<u8>,
    /// The sync manager blocks and FMMU blocks as written.
    pub sm: [[u8; 8]; 4],
    pub fmmu: [[u8; 16]; 2],
    /// `0x608F:1`, `0x608F:2`, `0x6091:1`, `0x6091:2`; `None` makes the slave abort the read, as a drive without the object does.
    pub units: Option<[u32; 4]>,
    /// `0x607D:1` and `:2`.
    pub travel: Option<(i32, i32)>,
    pub state: Cia,
    pub target: i32,
    pub position: i32,
    /// Latches a fault the moment it is told to enable operation.
    pub fault_on_enable: bool,
    /// Every controlword a serviced LRW carried to this slave, in order.
    pub controlwords: Vec<u16>,
    /// Does not service anything: it fell off the ring (a cable, a power supply).
    pub offline: bool,
    pub reply: Reply,
    /// How many polls of the input mailbox's status find it empty before it says full.
    pub polls_before_full: usize,
    /// Every poll of the input mailbox's status, answered or not.
    pub status_polls: usize,
    /// Every object asked for over the mailbox, as (index, subindex).
    pub sdo_requests: Vec<(u16, u8)>,
    polls_since_request: usize,
    mailbox_in: Option<Vec<u8>>,
    /// The input mailbox has said it is full, and may now be read.
    filled: bool,
}

impl Slave {
    pub fn new() -> Slave {
        Slave {
            station: 0,
            al: INIT,
            al_error: false,
            al_code: 0,
            refuse: None,
            stuck_at: None,
            sm: [[0; 8]; 4],
            fmmu: [[0; 16]; 2],
            // 131072 increments a turn behind a 50:1 gear: 6,553,600 increments a turn of the shaft.
            units: Some([131072, 1, 50, 1]),
            travel: Some((-(1 << 22), 1 << 22)),
            state: Cia::SwitchOnDisabled,
            target: 0,
            position: 0,
            fault_on_enable: false,
            controlwords: Vec::new(),
            offline: false,
            reply: Reply::Normal,
            polls_before_full: 0,
            status_polls: 0,
            sdo_requests: Vec::new(),
            polls_since_request: 0,
            mailbox_in: None,
            filled: false,
        }
    }

    pub fn faulted() -> Slave {
        Slave {
            state: Cia::Fault,
            ..Slave::new()
        }
    }

    /// The controlword's command, evaluated as a level the way the profile does.
    fn command(&mut self, controlword: u16) {
        self.controlwords.push(controlword);
        if controlword & 0x0080 != 0 {
            if matches!(self.state, Cia::Fault | Cia::FaultReactionActive) {
                self.state = Cia::SwitchOnDisabled;
            }
            return;
        }
        if matches!(self.state, Cia::Fault | Cia::FaultReactionActive) {
            return;
        }
        self.state = match (controlword, self.state) {
            (0x0000, _) => Cia::SwitchOnDisabled,
            // Quick stop holds for as long as the controlword says it: the drive stays in `quick stop active`.
            (0x0002, Cia::OperationEnabled | Cia::QuickStopActive) => Cia::QuickStopActive,
            (0x0002, _) => Cia::SwitchOnDisabled,
            (0x0006, _) => Cia::ReadyToSwitchOn,
            (0x0007, Cia::ReadyToSwitchOn | Cia::OperationEnabled) => Cia::SwitchedOn,
            (0x000F, Cia::SwitchedOn | Cia::QuickStopActive) => {
                if self.fault_on_enable {
                    Cia::Fault
                } else {
                    Cia::OperationEnabled
                }
            }
            (_, same) => same,
        };
    }

    fn object(&self, index: u16, subindex: u8) -> Option<(u32, usize)> {
        match (index, subindex) {
            (0x608F, 1) => self.units.map(|u| (u[0], 4)),
            (0x608F, 2) => self.units.map(|u| (u[1], 4)),
            (0x6091, 1) => self.units.map(|u| (u[2], 4)),
            (0x6091, 2) => self.units.map(|u| (u[3], 4)),
            (0x607D, 1) => self.travel.map(|t| (t.0 as u32, 4)),
            (0x607D, 2) => self.travel.map(|t| (t.1 as u32, 4)),
            _ => None,
        }
    }

    /// A slave's answer to an SDO read, in a whole mailbox.
    fn sdo_reply(spec: u8, index: u16, subindex: u8, data: u32) -> Vec<u8> {
        let mut mailbox = vec![0u8; 128];
        mailbox[0..2].copy_from_slice(&10u16.to_le_bytes());
        mailbox[5] = 0x03; // mailbox type: CoE
        mailbox[6..8].copy_from_slice(&(0x03u16 << 12).to_le_bytes()); // CoE: SDO response
        mailbox[8] = spec;
        mailbox[9..11].copy_from_slice(&index.to_le_bytes());
        mailbox[11] = subindex;
        mailbox[12..16].copy_from_slice(&data.to_le_bytes());
        mailbox
    }

    fn mailbox_request(&mut self, message: &[u8]) {
        self.mailbox_in = None;
        self.polls_since_request = 0;
        self.filled = false;
        if message.len() < 16 || message[5] & 0x0F != 0x03 {
            return;
        }
        let index = u16::from_le_bytes([message[9], message[10]]);
        let subindex = message[11];
        self.sdo_requests.push((index, subindex));
        self.mailbox_in = match self.reply.clone() {
            Reply::Never => None,
            Reply::Zeros => Some(vec![0u8; 128]),
            Reply::Abort(code) => Some(Slave::sdo_reply(0x80, index, subindex, code)),
            Reply::OtherObject => {
                let (value, _) = self.object(index, subindex).unwrap_or((0, 4));
                Some(Slave::sdo_reply(
                    0x43,
                    index.wrapping_add(1),
                    subindex,
                    value,
                ))
            }
            Reply::Normal => Some(match self.object(index, subindex) {
                Some((value, _)) => Slave::sdo_reply(0x43, index, subindex, value),
                None => Slave::sdo_reply(0x80, index, subindex, 0x0602_0000),
            }),
        };
        // A slave that does not make the master wait has filled the mailbox before anybody asks.
        self.filled = self.mailbox_in.is_some() && self.polls_before_full == 0;
    }

    /// AL control: a request, answered in AL status and never by failing the write.
    fn al_request(&mut self, control: u8) {
        let want = control & 0x0F;
        if want == INIT {
            self.al = INIT;
            self.al_error = false;
            self.al_code = 0;
            return;
        }
        if let Some((state, code)) = self.refuse {
            if state == want {
                self.al_error = true;
                self.al_code = code;
                return;
            }
        }
        if let Some(limit) = self.stuck_at {
            if want > limit {
                return;
            }
        }
        self.al = want;
        self.al_error = false;
        self.al_code = 0;
    }

    /// A register read: `Some(bytes)` when the ESC serviced it, `None` when it was denied.
    fn read(&mut self, register: u16, length: usize) -> Option<Vec<u8>> {
        let word = |value: u16| {
            let mut bytes = value.to_le_bytes().to_vec();
            bytes.resize(length, 0);
            bytes
        };
        match register {
            0x0000 => Some(word(0x0004)),
            0x0010 => Some(word(self.station)),
            0x0120 => Some(word(self.al as u16)),
            0x0130 => Some(word(self.al as u16 | if self.al_error { 0x10 } else { 0 })),
            0x0134 => Some(word(self.al_code)),
            // Sync manager 1's status register: bit 3 says the input mailbox holds a message.
            0x080D => {
                self.status_polls += 1;
                if self.mailbox_in.is_some() && self.polls_since_request >= self.polls_before_full {
                    self.filled = true;
                }
                self.polls_since_request += 1;
                Some(word(if self.filled { 0x08 } else { 0x00 }))
            }
            0x0600..=0x061F => {
                let at = (register - 0x0600) as usize;
                let block = self.fmmu.iter().flatten().copied().collect::<Vec<u8>>();
                block.get(at..at + length).map(<[u8]>::to_vec)
            }
            0x0800..=0x081F => {
                let at = (register - 0x0800) as usize;
                let block = self.sm.iter().flatten().copied().collect::<Vec<u8>>();
                block.get(at..at + length).map(<[u8]>::to_vec)
            }
            // The input mailbox: a read of an empty one is denied, which is what an ESC does.
            0x1400 => {
                if self.filled {
                    self.filled = false;
                    self.mailbox_in.take().map(|mut mailbox| {
                        mailbox.resize(length, 0);
                        mailbox
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// A register write: whether the ESC serviced it.
    fn write(&mut self, register: u16, data: &[u8]) -> bool {
        match register {
            0x0010 if data.len() >= 2 => {
                self.station = u16::from_le_bytes([data[0], data[1]]);
                true
            }
            0x0120 if !data.is_empty() => {
                self.al_request(data[0]);
                true
            }
            0x0600..=0x061F => {
                let at = (register - 0x0600) as usize;
                for (offset, byte) in data.iter().enumerate() {
                    let place = at + offset;
                    if place < 32 {
                        self.fmmu[place / 16][place % 16] = *byte;
                    }
                }
                true
            }
            0x0800..=0x081F => {
                let at = (register - 0x0800) as usize;
                for (offset, byte) in data.iter().enumerate() {
                    let place = at + offset;
                    if place < 32 {
                        self.sm[place / 8][place % 8] = *byte;
                    }
                }
                true
            }
            // The output mailbox is live from Pre-Op: before it, the write is denied.
            0x1000 if self.al >= PRE_OP => {
                self.mailbox_request(data);
                true
            }
            _ => false,
        }
    }

    /// The window a configured, activated FMMU maps: (logical start, length).
    fn window(&self, fmmu: usize) -> Option<(usize, usize)> {
        let block = &self.fmmu[fmmu];
        if block[12] != 0x01 {
            return None;
        }
        let logical = u32::from_le_bytes([block[0], block[1], block[2], block[3]]) as usize;
        let length = u16::from_le_bytes([block[4], block[5]]) as usize;
        Some((logical, length))
    }
}

/// The scripted ring: every slave on it, and everything the master sent.
#[derive(Debug, Default)]
pub struct Ring {
    pub slaves: Vec<Slave>,
    /// Every frame received from the master (or anybody), in order.
    pub log: Vec<Frame>,
    /// Added to the working counter of every datagram of this command: a counter one too high, or one short.
    pub wkc_delta: Vec<(u8, i32)>,
    /// How many of the next frames vanish without an answer.
    pub swallow: usize,
    /// Answers nothing from now on: the cable is out.
    pub deaf: bool,
}

impl Ring {
    pub fn of(count: usize) -> Ring {
        Ring {
            slaves: (0..count).map(|_| Slave::new()).collect(),
            ..Ring::default()
        }
    }

    /// Every LRW the master sent, as (index, the image it carried).
    pub fn lrws(&self) -> Vec<(u8, Vec<u8>)> {
        self.log
            .iter()
            .flat_map(|frame| frame.datagrams.iter())
            .filter(|datagram| datagram.command == LRW)
            .map(|datagram| (datagram.index, datagram.data.clone()))
            .collect()
    }

    /// Every datagram the master sent, in order.
    pub fn datagrams(&self) -> Vec<&Datagram> {
        self.log
            .iter()
            .flat_map(|frame| frame.datagrams.iter())
            .collect()
    }

    /// What the ring says back to `frame`; `None` where it says nothing at all.
    pub fn handle(&mut self, frame: &Frame) -> Option<Frame> {
        self.log.push(frame.clone());
        if self.swallow > 0 {
            self.swallow -= 1;
            return None;
        }
        if self.deaf {
            return None;
        }
        let mut answer = frame.clone();
        for datagram in &mut answer.datagrams {
            self.service(datagram);
            for (command, delta) in &self.wkc_delta {
                if *command == datagram.command {
                    datagram.working_counter =
                        (datagram.working_counter as i32 + delta).clamp(0, 0xFFFF) as u16;
                }
            }
        }
        Some(answer)
    }

    fn service(&mut self, datagram: &mut Datagram) {
        let (first, register) = datagram.address;
        let length = datagram.data.len();
        match datagram.command {
            BRD => {
                for slave in self.slaves.iter_mut().filter(|slave| !slave.offline) {
                    if let Some(bytes) = slave.read(register, length) {
                        for (byte, read) in datagram.data.iter_mut().zip(bytes) {
                            *byte |= read;
                        }
                        datagram.working_counter += 1;
                    }
                }
            }
            BWR => {
                let data = datagram.data.clone();
                for slave in self.slaves.iter_mut().filter(|slave| !slave.offline) {
                    if slave.write(register, &data) {
                        datagram.working_counter += 1;
                    }
                }
            }
            APWR => {
                // Auto-increment: the address is minus the position, and the slave at zero services it.
                let data = datagram.data.clone();
                let position = (first as i16).wrapping_neg();
                if position >= 0 {
                    if let Some(slave) = self
                        .slaves
                        .get_mut(position as usize)
                        .filter(|slave| !slave.offline)
                    {
                        if slave.write(register, &data) {
                            datagram.working_counter += 1;
                        }
                    }
                }
            }
            FPRD => {
                if let Some(slave) = self
                    .slaves
                    .iter_mut()
                    .find(|slave| slave.station == first && slave.station != 0 && !slave.offline)
                {
                    if let Some(bytes) = slave.read(register, length) {
                        datagram.data = bytes;
                        datagram.working_counter += 1;
                    }
                }
            }
            FPWR => {
                let data = datagram.data.clone();
                if let Some(slave) = self
                    .slaves
                    .iter_mut()
                    .find(|slave| slave.station == first && slave.station != 0 && !slave.offline)
                {
                    if slave.write(register, &data) {
                        datagram.working_counter += 1;
                    }
                }
            }
            LRW => {
                for slave in self.slaves.iter_mut().filter(|slave| !slave.offline) {
                    let mut serviced = 0;
                    // Outputs are taken in Op only; a write counts two.
                    if slave.al == OP {
                        if let Some((at, len)) = slave.window(0) {
                            if len >= 6 && at + len <= datagram.data.len() {
                                let image = &datagram.data[at..at + 6];
                                slave.target =
                                    i32::from_le_bytes([image[2], image[3], image[4], image[5]]);
                                slave.command(u16::from_le_bytes([image[0], image[1]]));
                                if slave.state == Cia::OperationEnabled {
                                    slave.position = slave.target;
                                }
                                serviced += 2;
                            }
                        }
                    }
                    // Inputs are given from Safe-Op; a read counts one.
                    if slave.al >= SAFE_OP {
                        if let Some((at, len)) = slave.window(1) {
                            if len >= 6 && at + len <= datagram.data.len() {
                                datagram.data[at..at + 2]
                                    .copy_from_slice(&slave.state.statusword().to_le_bytes());
                                datagram.data[at + 2..at + 6]
                                    .copy_from_slice(&slave.position.to_le_bytes());
                                serviced += 1;
                            }
                        }
                    }
                    datagram.working_counter += serviced;
                }
            }
            _ => {}
        }
    }
}

// ── in process ───────────────────────────────────────────────────────────────

/// A scripted ring a test keeps a handle on, so it can change what the slaves say after the bridge has been opened over it and
/// read what they were sent after the bridge is gone.
pub type Handle = Arc<Mutex<Ring>>;

/// Run `act` on the ring behind `handle`.
pub fn with<R>(handle: &Handle, act: impl FnOnce(&mut Ring) -> R) -> R {
    act(&mut handle.lock().unwrap_or_else(|p| p.into_inner()))
}

/// The ring as a [`Segment`]: a master's `cycle` is answered at once.
pub struct RingSegment {
    ring: Handle,
}

impl RingSegment {
    pub fn new(ring: Ring) -> RingSegment {
        RingSegment {
            ring: Arc::new(Mutex::new(ring)),
        }
    }

    pub fn handle(&self) -> Handle {
        self.ring.clone()
    }
}

impl Segment for RingSegment {
    fn cycle(&mut self, frame: &Frame) -> Result<Frame, SegmentError> {
        with(&self.ring, |ring| ring.handle(frame)).ok_or_else(|| {
            SegmentError("no answer from the ring: the scripted ring said nothing".into())
        })
    }
}

// ── on a kernel interface ────────────────────────────────────────────────────

/// The ring serving a kernel interface from a thread, until dropped.
pub struct Serving {
    pub ring: Handle,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn serve_on(interface: &str, ring: Ring) -> Serving {
    let ring = Arc::new(Mutex::new(ring));
    let stop = Arc::new(AtomicBool::new(false));
    let socket =
        RawSocket::open(interface).expect("a socket on the interface for the scripted ring");
    let (shared, flag) = (ring.clone(), stop.clone());
    let thread = thread::spawn(move || {
        while !flag.load(Ordering::Acquire) {
            match socket.recv_raw(Duration::from_millis(10)) {
                Ok(Some(raw)) => {
                    let Ok(frame) = Frame::unpack(&raw) else {
                        continue;
                    };
                    let answer = with(&shared, |ring| ring.handle(&frame));
                    if let Some(answer) = answer {
                        if let Ok(bytes) = answer.pack() {
                            let _ = socket.send_raw(&bytes);
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Serving {
        ring,
        stop,
        thread: Some(thread),
    }
}

impl Serving {
    pub fn with<R>(&self, act: impl FnOnce(&mut Ring) -> R) -> R {
        act(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait until `condition` holds of the ring, or fail saying what never happened.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&Ring) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition(&self.lock()) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the scripted ring never saw {what} within five seconds");
    }

    /// What the ring has seen once it has had time to see everything in flight.
    pub fn settled<R>(&self, read: impl Fn(&Ring) -> R) -> R {
        thread::sleep(Duration::from_millis(150));
        read(&self.lock())
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
