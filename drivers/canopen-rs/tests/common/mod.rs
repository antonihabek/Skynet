//! A scripted CiA 402 drive, for the tests that need to decide what a drive says and see what it was sent.
//!
//! `live_vcan` runs the port against the Python driver's simulated drives, which is the substrate. What it cannot do is
//! *choose*: a drive whose encoder resolution is zero, whose TPDO2 maps three objects, that latches a fault the moment it is
//! enabled, or that answers a heartbeat in the middle of a SYNC — each is a refusal or a count in the driver that the
//! simulated drives never provoke. [`Model`] is that drive, and it is deliberately not clever: frames in, frames out, and a
//! log of every frame it was sent, so a test can say what the master wrote and in what order.
//!
//! It is used two ways. As a [`DriveBus`], a synchronous in-process [`Bus`] that answers inside `send`, it needs no kernel and
//! no clock: what the bridge does with each answer is decided by the test. And on a kernel CAN interface, behind
//! [`serve_on`], it is the other end of the vtable's own `SocketCan` bus. **It agrees with whoever scripted it** — F15's point
//! — and says nothing about what a real drive does.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use skynet_canopen_rs::bus::{Bus, SocketCan};
use skynet_canopen_rs::frames::{self, Frame};

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
            Cia::FaultReactionActive => 0x000F,
            Cia::Fault => 0x0208,
        }
    }
}

/// One scripted drive.
#[derive(Debug, Clone)]
pub struct Drive {
    pub node: u8,
    pub state: Cia,
    pub operational: bool,
    pub mode: i8,
    /// What `0x6061` says, when it is not what was asked for.
    pub mode_display: Option<i8>,
    pub target: i32,
    pub position: i32,
    /// `0x608F:1`, `0x608F:2`, `0x6091:1`, `0x6091:2`.
    pub units: [u32; 4],
    /// `0x1A01:0`, `:1`, `:2` and `0x1601:0`, `:1`, `:2`.
    pub mapping: [u32; 6],
    /// Keeps answering SDO and never lets its TPDO2 onto the bus.
    pub silent_tpdo2: bool,
    /// Latches a fault the moment it is told to enable operation.
    pub fault_on_enable: bool,
    /// Every controlword written, by SDO or by RPDO2, in order.
    pub controlwords: Vec<u16>,
}

/// The CiA 402 default mapping, as CiA 301 packs an entry.
pub const TPDO2: [u32; 2] = [(0x6041 << 16) | 16, (0x6064 << 16) | 32];
pub const RPDO2: [u32; 2] = [(0x6040 << 16) | 16, (0x607A << 16) | 32];

impl Drive {
    pub fn new(node: u8) -> Drive {
        Drive {
            node,
            state: Cia::SwitchOnDisabled,
            operational: false,
            mode: 0,
            mode_display: None,
            target: 0,
            position: 0,
            // 131072 increments a turn behind a 50:1 gear: 6,553,600 increments a turn of the shaft.
            units: [131072, 1, 50, 1],
            mapping: [2, TPDO2[0], TPDO2[1], 2, RPDO2[0], RPDO2[1]],
            silent_tpdo2: false,
            fault_on_enable: false,
            controlwords: Vec::new(),
        }
    }

    pub fn faulted(mut self) -> Drive {
        self.state = Cia::Fault;
        self
    }

    /// The controlword's command, evaluated as a level the way the profile does.
    fn command(&mut self, controlword: u16) {
        self.controlwords.push(controlword);
        if controlword & 0x0080 != 0 {
            if self.state == Cia::Fault {
                self.state = Cia::SwitchOnDisabled;
            }
            return;
        }
        if matches!(self.state, Cia::Fault | Cia::FaultReactionActive) {
            return;
        }
        self.state = match (controlword, self.state) {
            (0x0000, _) => Cia::SwitchOnDisabled,
            (0x0002, Cia::OperationEnabled) => Cia::QuickStopActive,
            (0x0002, _) => Cia::SwitchOnDisabled,
            (0x0006, _) => Cia::ReadyToSwitchOn,
            (0x0007, Cia::ReadyToSwitchOn) => Cia::SwitchedOn,
            (0x0007, Cia::OperationEnabled) => Cia::SwitchedOn,
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

    fn read(&self, index: u16, subindex: u8) -> Option<(u32, u8)> {
        Some(match (index, subindex) {
            (0x6041, 0) => (self.state.statusword() as u32, 2),
            (0x6061, 0) => (self.mode_display.unwrap_or(self.mode) as u8 as u32, 1),
            (0x608F, 1) => (self.units[0], 4),
            (0x608F, 2) => (self.units[1], 4),
            (0x6091, 1) => (self.units[2], 4),
            (0x6091, 2) => (self.units[3], 4),
            (0x607D, 1) => ((-(1i32 << 22)) as u32, 4),
            (0x607D, 2) => (1u32 << 22, 4),
            (0x6080, 0) => (3000, 4),
            (0x1A01, 0) => (self.mapping[0], 1),
            (0x1A01, 1) => (self.mapping[1], 4),
            (0x1A01, 2) => (self.mapping[2], 4),
            (0x1601, 0) => (self.mapping[3], 1),
            (0x1601, 1) => (self.mapping[4], 4),
            (0x1601, 2) => (self.mapping[5], 4),
            _ => return None,
        })
    }

    fn write(&mut self, index: u16, subindex: u8, value: u32) {
        match (index, subindex) {
            (0x6040, 0) => self.command(value as u16),
            (0x6060, 0) => self.mode = value as u8 as i8,
            _ => {}
        }
    }
}

/// The scripted bus: every drive on it, and everything the master sent.
#[derive(Debug, Default)]
pub struct Model {
    pub drives: Vec<Drive>,
    /// Every frame received from the master (or anybody), in order.
    pub log: Vec<Frame>,
    /// Frames put on the bus after every SYNC's TPDO2s: a heartbeat, an emergency object, a stranger.
    pub after_sync: Vec<Frame>,
    /// Read at every SYNC, for a test that wants to know what the clock said when it left.
    pub sync_clock: Option<fn() -> u64>,
    pub synced_at: Vec<u64>,
}

impl Model {
    pub fn with(nodes: &[u8]) -> Model {
        Model {
            drives: nodes.iter().map(|node| Drive::new(*node)).collect(),
            ..Model::default()
        }
    }

    pub fn get(&self, node: u8) -> &Drive {
        self.drives
            .iter()
            .find(|drive| drive.node == node)
            .expect("a drive on that node")
    }

    pub fn drive(&mut self, node: u8) -> &mut Drive {
        self.drives
            .iter_mut()
            .find(|drive| drive.node == node)
            .expect("a drive on that node")
    }

    /// What the bus says back to `frame`, in the order a bus would.
    pub fn receive(&mut self, frame: &Frame) -> Vec<Frame> {
        self.log.push(*frame);
        let mut out = Vec::new();
        match frame.id {
            frames::NMT_ID => {
                let (command, node) = (frame.data[0], frame.data[1]);
                for drive in self
                    .drives
                    .iter_mut()
                    .filter(|drive| node == 0 || drive.node == node)
                {
                    match command {
                        frames::NMT_START => drive.operational = true,
                        frames::NMT_PRE_OPERATIONAL => drive.operational = false,
                        _ => {}
                    }
                }
            }
            frames::SYNC_ID => {
                if let Some(clock) = self.sync_clock {
                    self.synced_at.push(clock());
                }
                for drive in self
                    .drives
                    .iter_mut()
                    .filter(|drive| drive.operational && !drive.silent_tpdo2)
                {
                    if drive.state == Cia::OperationEnabled {
                        drive.position = drive.target;
                    }
                    let [s0, s1] = drive.state.statusword().to_le_bytes();
                    let [p0, p1, p2, p3] = drive.position.to_le_bytes();
                    out.push(
                        Frame::new(frames::tpdo2_id(drive.node), &[s0, s1, p0, p1, p2, p3])
                            .unwrap(),
                    );
                }
                out.extend(self.after_sync.iter().copied());
            }
            id => {
                for drive in &mut self.drives {
                    if id == frames::sdo_request_id(drive.node) {
                        if let Some(reply) = sdo(drive, frame) {
                            out.push(reply);
                        }
                    } else if id == frames::rpdo2_id(drive.node)
                        && drive.operational
                        && frame.len == 6
                    {
                        drive.command(u16::from_le_bytes([frame.data[0], frame.data[1]]));
                        drive.target = i32::from_le_bytes([
                            frame.data[2],
                            frame.data[3],
                            frame.data[4],
                            frame.data[5],
                        ]);
                    }
                }
            }
        }
        out
    }

    /// Every RPDO2 sent to `node`, as (controlword, target).
    pub fn rpdos(&self, node: u8) -> Vec<(u16, i32)> {
        self.log
            .iter()
            .filter(|frame| frame.id == frames::rpdo2_id(node) && frame.len == 6)
            .map(|frame| {
                (
                    u16::from_le_bytes([frame.data[0], frame.data[1]]),
                    i32::from_le_bytes([
                        frame.data[2],
                        frame.data[3],
                        frame.data[4],
                        frame.data[5],
                    ]),
                )
            })
            .collect()
    }

    /// Every NMT frame, as (command, node).
    pub fn nmt(&self) -> Vec<(u8, u8)> {
        self.log
            .iter()
            .filter(|frame| frame.id == frames::NMT_ID)
            .map(|frame| (frame.data[0], frame.data[1]))
            .collect()
    }
}

fn sdo(drive: &mut Drive, frame: &Frame) -> Option<Frame> {
    let d = frame.data;
    let index = u16::from_le_bytes([d[1], d[2]]);
    let subindex = d[3];
    let id = frames::sdo_response_id(drive.node);
    let abort = |code: u32| {
        let c = code.to_le_bytes();
        Frame::new(id, &[0x80, d[1], d[2], d[3], c[0], c[1], c[2], c[3]])
    };
    if d[0] == 0x40 {
        return match drive.read(index, subindex) {
            Some((value, size)) => {
                let specifier = match size {
                    1 => 0x4F,
                    2 => 0x4B,
                    _ => 0x43,
                };
                let v = value.to_le_bytes();
                Frame::new(id, &[specifier, d[1], d[2], d[3], v[0], v[1], v[2], v[3]])
            }
            None => abort(frames::ABORT_NO_SUCH_OBJECT),
        };
    }
    let size = match d[0] {
        0x2F => 1,
        0x2B => 2,
        0x27 => 3,
        0x23 => 4,
        _ => return abort(frames::ABORT_LENGTH),
    };
    let mut raw = [0u8; 4];
    raw[..size].copy_from_slice(&d[4..4 + size]);
    drive.write(index, subindex, u32::from_le_bytes(raw));
    Frame::new(id, &[0x60, d[1], d[2], d[3], 0, 0, 0, 0])
}

// ── in process ───────────────────────────────────────────────────────────────

/// A scripted bus a test keeps a handle on, so it can change what the drives say after the bridge has been opened over it and
/// read what they were sent after the bridge is gone.
pub type Handle = Arc<Mutex<Model>>;

/// Run `act` on the model behind `handle`.
pub fn with<R>(handle: &Handle, act: impl FnOnce(&mut Model) -> R) -> R {
    act(&mut handle.lock().unwrap_or_else(|p| p.into_inner()))
}

/// The model as a [`Bus`]: a master's `send` is answered at once, and its `recv` pops the answers.
pub struct DriveBus {
    model: Handle,
    queue: VecDeque<Frame>,
    synchronous: bool,
}

impl DriveBus {
    pub fn new(model: Model) -> DriveBus {
        DriveBus {
            model: Arc::new(Mutex::new(model)),
            queue: VecDeque::new(),
            synchronous: true,
        }
    }

    /// A bus that says it is a real one: a frame that is not here now may still arrive.
    pub fn real(model: Model) -> DriveBus {
        DriveBus {
            synchronous: false,
            ..DriveBus::new(model)
        }
    }

    pub fn handle(&self) -> Handle {
        self.model.clone()
    }
}

impl Bus for DriveBus {
    fn synchronous(&self) -> bool {
        self.synchronous
    }

    fn send(&mut self, frame: &Frame) -> io::Result<()> {
        let replies = with(&self.model, |model| model.receive(frame));
        self.queue.extend(replies);
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> io::Result<Option<Frame>> {
        Ok(self.queue.pop_front())
    }
}

// ── on a kernel bus ──────────────────────────────────────────────────────────

/// The model serving a kernel CAN interface from a thread, until dropped.
pub struct Serving {
    pub model: Arc<Mutex<Model>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn serve_on(interface: &str, model: Model) -> Serving {
    let model = Arc::new(Mutex::new(model));
    let stop = Arc::new(AtomicBool::new(false));
    let mut socket =
        SocketCan::open(interface).expect("a socket on the interface for the scripted drives");
    let (shared, flag) = (model.clone(), stop.clone());
    let thread = thread::spawn(move || {
        while !flag.load(Ordering::Acquire) {
            match socket.recv(Duration::from_millis(10)) {
                Ok(Some(frame)) => {
                    let replies = shared
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .receive(&frame);
                    for reply in replies {
                        let _ = socket.write_frame(&reply);
                    }
                }
                Ok(None) => {}
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Serving {
        model,
        stop,
        thread: Some(thread),
    }
}

impl Serving {
    pub fn with<R>(&self, act: impl FnOnce(&mut Model) -> R) -> R {
        act(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Model> {
        self.model.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait until `condition` holds of the model, or fail saying what never happened.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&Model) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition(&self.lock()) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the scripted drives never saw {what} within five seconds");
    }

    /// What the model has seen once it has had time to see everything in flight.
    pub fn settled<R>(&self, read: impl Fn(&Model) -> R) -> R {
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
