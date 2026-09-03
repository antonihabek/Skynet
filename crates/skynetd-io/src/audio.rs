//! §49's warning channel — the tone that sounds before autonomous motion.
//!
//! > Standards for robots operating around people expect an audible indication before autonomous
//! > motion. That makes the warning tone a **system function, not an application feature**:
//! > `skynetd` owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and
//! > **a package cannot suppress it.**
//!
//! Two thirds of that sentence were already built and the third was missing, which is a shape worth
//! naming because it is invisible from either half. `admission::WARNING_CHANNEL` reserves
//! `hri.warning` so no grant can confer it, `PackageRejectionCode::CapabilityReserved` refuses the
//! package that asks, and `opstate::warning_required` decides exactly which transitions demand a
//! tone — phrased over `commands_autonomously` so the two spellings cannot come apart.
//!
//! `Transition::warning` then reached `observe_state`, which dropped it on the floor. Every rule
//! about the tone was enforceable and nothing had ever made a sound.
//!
//! # Declared, then attempted
//!
//! Which output is the warning speaker is a fact about the machine, exactly as which interface is
//! the modem is (§34.3): `/sys` will not say whether an ALSA device is a klaxon on the mast or the
//! HDMI port nobody plugged in. So the command is **declared** in `state/audio.json` and this module
//! runs it. A robot with no declaration reports that it has no warning channel rather than
//! inventing one, which is [`crate::connectivity`]'s rule applied to a different piece of hardware.
//!
//! # The one thing this cannot do, stated rather than worked around
//!
//! §49.1 says the tone sounds **before** autonomous motion. This daemon sounds it before it commits
//! the new state to its own fields, which is the strongest ordering available from here and is not
//! the same guarantee: on a robot whose kernel owns the transition, `skynetd` learns about `RUNNING`
//! by observing it, and by then the body is already commanded. Making the tone a precondition of the
//! transition is the kernel's to enforce, across §33.1's line, and no amount of care on this side
//! substitutes for it.
//!
//! What follows from that is the failure rule below. A robot that **cannot** sound its warning is a
//! robot missing a control somebody's risk assessment relied on, and this process cannot refuse to
//! run — so it raises an incident, which §34.4 never drops, and says so on every surface. Reporting
//! loudly is a weaker answer than refusing and it is the honest one for a process that does not hold
//! the switch.

use std::path::Path;

use skynet_contracts::opstate::Transition;
use skynet_json::Json;

/// How long the tone may hold §33.2's loop.
///
/// Three seconds, and the number is a property of the loop rather than of any speaker: while this
/// runs, nothing serves the local API, nothing reconciles and nothing pings the watchdog. A warning
/// long enough to be heard is short enough to fit; a command that hangs is killed and reported as a
/// warning that did not sound, which is the fact an operator needs.
pub const TONE_BUDGET_S: u64 = 3;

/// What a robot has been told about its warning channel (§49.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Channel {
    /// The command to run, argv-style. Empty where nothing has been declared.
    pub command: Vec<String>,
    /// A human name for the surfaces — "mast klaxon", "cabinet buzzer".
    pub label: String,
}

impl Channel {
    /// Read `state/audio.json`, or an empty channel.
    ///
    /// Missing, unreadable and malformed all read as **undeclared**, and that is the safe direction
    /// for once: an undeclared channel is reported as a robot that cannot warn, and the alternative
    /// — falling back to some plausible default device — is a robot that believes it is warning
    /// people and is playing a sound into a disconnected port.
    pub fn of(layout: &crate::layout::Layout) -> Channel {
        Channel::read(&layout.state().join("audio.json"))
    }

    fn read(path: &Path) -> Channel {
        let Some(document) = crate::observe::read_json(path) else {
            return Channel::default();
        };
        let Some(warning) = document.get_present("warning") else {
            return Channel::default();
        };
        let command: Vec<String> = warning
            .get_present("command")
            .and_then(Json::as_arr)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .filter(|part| !part.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Channel {
            command,
            label: warning
                .get_present("label")
                .and_then(Json::as_str)
                .unwrap_or("the declared warning device")
                .to_string(),
        }
    }

    /// Whether this robot has been told how to make a sound.
    pub fn declared(&self) -> bool {
        !self.command.is_empty()
    }

    /// The sentence for a surface.
    pub fn detail(&self) -> String {
        if self.declared() {
            format!(
                "`{}` sounds the warning ({}). §49.1 owns this channel: no grant confers \
                 `hri.warning`, so no package can suppress it.",
                self.command.join(" "),
                self.label
            )
        } else {
            "This robot has no warning channel. §49.1 expects an audible indication before \
             autonomous motion and nothing here can produce one — which output is the warning \
             speaker is a property of the machine, so it is declared in `state/audio.json` and \
             there is no such declaration."
                .to_string()
        }
    }

    /// As a surface reports it.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("declared", Json::Bool(self.declared()));
        out.set("detail", Json::Str(self.detail()));
        if self.declared() {
            out.set("label", Json::Str(self.label.clone()));
            let mut argv = Json::arr();
            for part in &self.command {
                argv.push(Json::Str(part.clone()));
            }
            out.set("command", argv);
        }
        out
    }
}

/// What one attempt to warn did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sounded {
    /// It played.
    Yes,
    /// There is no declared channel on this robot.
    Undeclared,
    /// There is one and it did not work.
    Failed(String),
}

impl Sounded {
    /// Whether the people near this robot were warned.
    pub fn ok(&self) -> bool {
        *self == Sounded::Yes
    }

    /// The sentence.
    pub fn detail(&self) -> String {
        match self {
            Sounded::Yes => "the warning sounded".to_string(),
            Sounded::Undeclared => "no warning channel is declared in `state/audio.json`, so \
                                    nothing sounded"
                .to_string(),
            Sounded::Failed(why) => {
                format!("the warning channel is declared and did not sound: {why}")
            }
        }
    }
}

/// Something that can make the noise (§49.1).
///
/// A trait for [`crate::ota::Updater`]'s reason: the cases worth testing are a channel that works,
/// one that is not declared, one whose command is missing and one that hangs past its budget, and
/// none of those is reachable through a speaker a test controls.
pub trait Sounder {
    /// Warn, and say whether anybody could have heard it.
    fn warn(&mut self, channel: &Channel) -> Sounded;
}

/// The real one: run the declared command and wait for it.
///
/// **Synchronous, deliberately.** §49.1 puts the tone before the motion, and a fire-and-forget
/// spawn would put it beside it. Bounded by [`TONE_BUDGET_S`] because §33.2's loop is one thread —
/// see the module header for what that costs.
#[derive(Debug, Default)]
pub struct CommandSounder;

impl Sounder for CommandSounder {
    fn warn(&mut self, channel: &Channel) -> Sounded {
        let Some((program, arguments)) = channel.command.split_first() else {
            return Sounded::Undeclared;
        };
        let started = std::time::Instant::now();
        let child = std::process::Command::new(program)
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                return Sounded::Failed(format!(
                    "`{program}` could not be started ({error}). §49.1 makes the warning the \
                     operating system's, and this robot's declaration names a command it does not \
                     have."
                ))
            }
        };

        // Waited on rather than left running, and killed rather than waited on for ever. A tone
        // process that never exits holds §33.2's single thread, and a supervisor that stopped
        // supervising to finish a beep is a worse outcome than a beep that was cut short.
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Sounded::Yes,
                Ok(Some(status)) => {
                    let said = child
                        .stderr
                        .take()
                        .map(|mut pipe| {
                            use std::io::Read;
                            let mut text = String::new();
                            let _ = pipe.read_to_string(&mut text);
                            text.trim().to_string()
                        })
                        .unwrap_or_default();
                    return Sounded::Failed(if said.is_empty() {
                        format!("`{program}` exited {status}")
                    } else {
                        format!("`{program}` exited {status}: {said}")
                    });
                }
                Ok(None) => {}
                Err(error) => return Sounded::Failed(format!("`{program}`: {error}")),
            }
            if started.elapsed().as_secs() >= TONE_BUDGET_S {
                let _ = child.kill();
                let _ = child.wait();
                return Sounded::Failed(format!(
                    "`{program}` was still running after {TONE_BUDGET_S}s and was stopped. §33.2's \
                     loop is one thread, so a tone that does not end is a supervisor that does not \
                     resume."
                ));
            }
            std::thread::yield_now();
        }
    }
}

/// A sounder that never makes a noise, and says so.
///
/// Not a test double: it is what a robot with no declared channel gets, and it exists so that the
/// absence produces the same [`Sounded::Undeclared`] through the same path rather than by a caller
/// remembering to check first.
#[derive(Debug, Default)]
pub struct Silent;

impl Sounder for Silent {
    fn warn(&mut self, _channel: &Channel) -> Sounded {
        Sounded::Undeclared
    }
}

/// Whether this transition demands a tone, and what happened when it was tried.
///
/// The predicate is `opstate::warning_required` and is not re-derived here: it is phrased over
/// [`skynet_contracts::opstate::OperatingState::commands_autonomously`] precisely so that the two
/// spellings of "a policy has begun commanding this body" cannot come apart, and a second opinion
/// in this module would be the third.
pub fn warn_for(
    sounder: &mut dyn Sounder,
    channel: &Channel,
    transition: &Transition,
) -> Option<Sounded> {
    if !transition.warning {
        return None;
    }
    Some(if channel.declared() {
        sounder.warn(channel)
    } else {
        Sounded::Undeclared
    })
}
