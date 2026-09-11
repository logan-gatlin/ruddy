//! A reference interpreter for linked Ruddy artifacts.
//!
//! The JavaScript backend is one execution of the portable artifact; this
//! crate is a second, with its own value representation, so that what a
//! program means can be told apart from what one backend happens to do. It
//! executes the linked LIR directly: functions, continuations, handlers,
//! runtime type descriptors and their intrinsics, the checked conversions at
//! the foreign boundary, and the standard library's primitive contracts.
//! Externs outside those contracts, such as a platform's file system, are
//! reported as unsupported when a program reaches them.

pub mod value;

mod convert;
mod machine;
pub mod number;
pub mod prim;
mod reflect;
pub mod render;
mod types;

use std::{collections::BTreeMap, fmt};

use ruddy::{artifact::Artifact, types::Domains};

pub use value::Value;

/// Why a program could not be loaded or run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Only a linked artifact is executable: dependencies must have been
    /// copied in.
    Unlinked,
    /// The artifact refers outside its own tables.
    Load(String),
    /// The program reached something the interpreter does not implement.
    Unsupported(String),
    /// The program failed while running.
    Runtime(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unlinked => f.write_str("the interpreter requires a linked artifact"),
            Self::Load(message) => write!(f, "invalid artifact: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
            Self::Runtime(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// A loaded program: its globals initialized, ready to be called.
pub struct Program {
    machine: machine::Machine,
    /// Each exported value's path relative to the bundle, and its qualified name.
    exports: BTreeMap<String, String>,
}

impl Program {
    /// Load a linked artifact and run every global's initializer in order.
    pub fn load(artifact: &Artifact) -> Result<Self, Error> {
        if !artifact.header().dependencies.is_empty() {
            return Err(Error::Unlinked);
        }
        let prefix = format!(
            "{}@{}::",
            artifact.header().identity.name,
            artifact.header().identity.version
        );
        let exports = artifact
            .header()
            .values
            .iter()
            .filter_map(|value| {
                value
                    .name
                    .strip_prefix(&prefix)
                    .map(|relative| (relative.to_string(), value.name.clone()))
            })
            .collect();
        let mut machine = machine::Machine::load(artifact)?;
        machine.initialize()?;
        Ok(Self { machine, exports })
    }

    /// The integer domains the program was compiled against.
    pub fn domains(&self) -> Domains {
        self.machine.domains
    }

    /// The paths of the exported values, relative to the bundle, in name order.
    pub fn exports(&self) -> impl Iterator<Item = &str> {
        self.exports.keys().map(String::as_str)
    }

    /// An exported value by its path relative to the bundle, such as
    /// `parse` or `util::parse`.
    pub fn export(&self, path: &str) -> Result<Value, Error> {
        let qualified = self
            .exports
            .get(path)
            .ok_or_else(|| Error::Runtime(format!("no exported value at `{path}`")))?;
        self.global(qualified)
            .ok_or_else(|| Error::Runtime(format!("exported value `{path}` has no definition")))
    }

    /// A global by its qualified name, such as `app@0.1.0::parse`.
    pub fn global(&self, qualified: &str) -> Option<Value> {
        self.machine.globals.get(qualified).cloned()
    }

    /// Call a function value with one argument and run it to completion.
    pub fn call(&mut self, function: &Value, argument: Value) -> Result<Value, Error> {
        self.machine.call(function.clone(), argument)
    }
}
