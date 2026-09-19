//! The dashboard server on port 29999 — a Rust port of `drivers/ur_rtde/skynet_ur/dashboard.py`.
//!
//! Line-oriented text, one command per line, one line back. It reaches what RTDE cannot — the
//! power, the brakes, and whether a protective stop is latched — and this driver uses it for the
//! handful of transitions that happen once, at open, plus the program stop in `emergency_stop`.
//! Each transition is verified by reading the mode back, never by the reply: the controller
//! answers `Powering on` before it has done anything.
//!
//! Where the port differs: a reply is read up to its newline rather than as whatever one `recv`
//! returned, which is the same thing whenever a line arrives in one segment and the right thing
//! when it does not.

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// How often the bring-up asks for the mode while it waits: `dashboard.py`'s quarter second.
const MODE_POLL: Duration = Duration::from_millis(250);

/// A dashboard session.
pub struct Dashboard {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// What the server said on connect.
    pub banner: String,
}

/// The dashboard refused, or the robot did not reach the state it was asked for.
#[derive(Debug)]
pub enum DashboardError {
    /// A state the robot reached instead of the one asked for, or a safety status that forbids
    /// bringing it up.
    Refused(String),
    /// The transport failed.
    Io(io::Error),
}

impl From<io::Error> for DashboardError {
    fn from(error: io::Error) -> DashboardError {
        DashboardError::Io(error)
    }
}

impl Dashboard {
    /// Connect and read the banner.
    pub fn connect(stream: TcpStream) -> io::Result<Dashboard> {
        let writer = stream.try_clone()?;
        let mut dashboard = Dashboard {
            reader: BufReader::new(stream),
            writer,
            banner: String::new(),
        };
        dashboard.banner = dashboard.line()?;
        Ok(dashboard)
    }

    /// A second handle on the socket, for the stop path's `stop` — written, never read.
    pub fn stop_socket(&self) -> io::Result<TcpStream> {
        self.writer.try_clone()
    }

    fn line(&mut self) -> io::Result<String> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the dashboard server closed the connection",
            ));
        }
        Ok(line.trim().to_string())
    }

    /// One command, one line back.
    pub fn command(&mut self, text: &str) -> io::Result<String> {
        self.writer.write_all(format!("{text}\n").as_bytes())?;
        self.line()
    }

    /// `POWER_OFF`, `IDLE`, `RUNNING`, … — the word after the colon.
    pub fn robot_mode(&mut self) -> io::Result<String> {
        Ok(after_colon(&self.command("robotmode")?))
    }

    /// `NORMAL`, `PROTECTIVE_STOP`, … — the word after the colon.
    pub fn safety_status(&mut self) -> io::Result<String> {
        Ok(after_colon(&self.command("safetystatus")?))
    }

    /// Power on and release the brakes, waiting for `RUNNING` rather than for a reply, and giving
    /// up with the mode actually reached — which says whether to look at the estop, the brakes or
    /// the power.
    pub fn bring_up(&mut self, timeout: Duration) -> Result<(), DashboardError> {
        let mut mode = self.robot_mode()?;
        if mode == "RUNNING" {
            return Ok(());
        }
        if mode == "POWER_OFF" || mode == "BOOTING" {
            self.command("power on")?;
            mode = self.await_mode(&["IDLE", "RUNNING"], timeout)?;
        }
        if mode == "IDLE" {
            self.command("brake release")?;
            mode = self.await_mode(&["RUNNING"], timeout)?;
        }
        if mode != "RUNNING" {
            let safety = self.safety_status()?;
            return Err(DashboardError::Refused(format!(
                "the robot reached `{mode}` rather than `RUNNING`, with safety `{safety}`. \
                 `PROTECTIVE_STOP` and `EMERGENCY_STOP` are cleared at the machine and not from \
                 here; `FAULT` wants the log on the pendant."
            )));
        }
        Ok(())
    }

    fn await_mode(&mut self, wanted: &[&str], timeout: Duration) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        let mut mode = self.robot_mode()?;
        while !wanted.contains(&mode.as_str()) && Instant::now() < deadline {
            std::thread::sleep(MODE_POLL);
            mode = self.robot_mode()?;
        }
        Ok(mode)
    }
}

/// The word after the last colon, trimmed: `Robotmode: RUNNING` is `RUNNING`.
pub fn after_colon(reply: &str) -> String {
    reply.rsplit(':').next().unwrap_or(reply).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_after_the_colon() {
        assert_eq!(after_colon("Robotmode: RUNNING"), "RUNNING");
        assert_eq!(
            after_colon("Safetystatus: PROTECTIVE_STOP"),
            "PROTECTIVE_STOP"
        );
        assert_eq!(after_colon("no colon"), "no colon");
    }
}
