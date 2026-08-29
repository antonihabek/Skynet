//! URDF import — `skynet descriptor from-urdf`, as a library.
//!
//! §63.4 calls this the highest-leverage tool in Part VIII, "because the work is largely already
//! done and sitting in a file the robot already has". The 15-minute milestone in §63.5 is
//! *descriptor drafted from URDF*, and this crate is what has to hold it.
//!
//! Four layers, and the boundaries between them are the design:
//!
//! | Module | Does | Never does |
//! |---|---|---|
//! | [`xml`] | Reads XML | Know what a robot is |
//! | [`model`] | Says what the file declared | Interpret it |
//! | [`suggest`] | Proposes roles | Assign them |
//! | [`descriptor`] | Emits the descriptor | Invent a limit, a mode or an envelope |
//!
//! The split between [`model`] and [`descriptor`] exists so that "what the file says" and "what we
//! concluded from it" are never the same value. The split between [`suggest`] and [`descriptor`]
//! exists because a suggested role that silently became an assigned one is the failure mode §63.4
//! is written to prevent.
//!
//! # `std`, deliberately
//!
//! Unlike the rest of the workspace this crate links `std`. It is a workstation tool: it runs
//! before a robot exists, on a laptop, over a file. Nothing on the real-time path imports it, and
//! the descriptor it produces is consumed by crates that are `no_std`.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let urdf = r#"<robot name="bench arm">
//!   <link name="base_link"/><link name="upper"/>
//!   <joint name="shoulder_pitch" type="revolute">
//!     <parent link="base_link"/><child link="upper"/>
//!     <limit lower="-1.5" upper="1.5" effort="80" velocity="2"/>
//!   </joint></robot>"#;
//!
//! let model = skynet_urdf::model::parse(urdf)?;
//! let options = skynet_urdf::descriptor::ImportOptions {
//!     tool_version: "0.1.0".into(),
//!     ..Default::default()
//! };
//! let import = skynet_urdf::descriptor::build(&model, &options)?;
//!
//! // The two rules the tool exists to hold.
//! assert_eq!(import.descriptor.get("envelope"), Some(&skynet_json::Json::Null));
//! assert_eq!(import.report.suggestions[0].role.as_deref(), None); // no side named
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod descriptor;
pub mod model;
pub mod suggest;
pub mod xml;

pub use descriptor::{build, Import, ImportError, ImportOptions, ImportReport, Note, NoteLevel};
pub use model::{parse, UrdfError, UrdfModel};
pub use suggest::{Confidence, RoleSuggestion, Side};

impl std::error::Error for model::UrdfError {}
impl std::error::Error for xml::XmlError {}
impl std::error::Error for descriptor::ImportError {}

impl std::fmt::Display for descriptor::ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
