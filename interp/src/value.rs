//! The interpreter's values.
//!
//! This is deliberately not the JavaScript backend's representation: integers
//! are Rust integers of their own width rather than doubles, records and sums
//! are tagged Rust data rather than null-prototype objects, arrays are shared
//! vectors rather than relaxed-radix trees, and functions, continuations,
//! handler identities, and runtime type descriptors are distinct variants
//! rather than callables with marker properties. What the two share is the
//! artifact they execute and the primitive contracts they implement.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use indexmap::IndexMap;
use ruddy::{backend::host::NativeTemplate, externs, reification::Node, types::FixedInt};

/// One interpreter value.
#[derive(Clone, Debug)]
pub enum Value {
    /// No value at all: what a missing record field reads as, and what the
    /// compiler's "absent representation" of a conditional evidence slot is.
    /// It is the interpreter's `undefined`.
    Absent,
    /// A target-sized natural number, within the bound domain when it came
    /// from a literal or crossed a checked boundary.
    Nat(u64),
    /// A target-sized signed integer.
    Int(i64),
    /// A fixed-width integer of the given kind. Every kind fits in `i128`.
    Fixed(FixedInt, i128),
    Real(f64),
    Str(Rc<str>),
    Bool(bool),
    /// A record. The unit value is the empty record.
    Record(Rc<Record>),
    /// An immutable array.
    Array(Rc<Vec<Value>>),
    /// One case of a sum: its tag and, unless it is a bare case, its payload.
    Sum(Rc<Sum>),
    /// A Ruddy function paired with the values of its captures.
    Closure(Rc<Closure>),
    /// A saved destination and its explicit environment.
    Cont(Rc<Cont>),
    /// A mutable cell.
    Cell(Rc<RefCell<Value>>),
    /// A handler identity, minted once per dynamic evaluation of a `handle`.
    Tag(Rc<Tag>),
    /// A runtime type descriptor, a mirror once a mirror intrinsic made it
    /// authentic, or a conversion plan.
    Type(Rc<Type>),
    /// A host value the standard library declares as an extern: one entry of
    /// the portable primitive table, possibly partially applied.
    Primitive(Rc<Primitive>),
    /// A function the interpreter itself implements: a typed view's
    /// operation, or a converted function at a foreign boundary.
    Native(Rc<Native>),
    /// A value of a hidden type sealed for the host: nothing of it is visible
    /// outside, and only a package this program made can be opened.
    Package(Rc<Value>),
    /// A Ruddy function handed to the host under a fixed completion contract.
    Callback(Rc<Callback>),
}

/// A record's fields in insertion order.
pub type Record = IndexMap<Key, Value>;

/// A record field key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Named(Rc<str>),
    /// The unnamed operation of an effect handler record.
    Unnamed,
}

#[derive(Debug)]
pub struct Sum {
    pub tag: Rc<str>,
    pub payload: Option<Value>,
}

#[derive(Debug)]
pub struct Closure {
    pub function: usize,
    pub captures: Vec<Value>,
}

/// A continuation: where a value goes next.
#[derive(Debug)]
pub enum Cont {
    /// The end of a run: the value is the run's result.
    Root,
    /// A block of a function, entered with these captures and then the value,
    /// under the handler context saved when the continuation was made.
    Code {
        function: usize,
        block: usize,
        captures: Vec<Value>,
        handler: Handler,
    },
    /// A conversion pending on a host function's result before it reaches
    /// the parent continuation.
    Converting {
        plan: Rc<Type>,
        to: u32,
        callable: bool,
        parent: Value,
    },
}

/// The innermost active handler, or none outside every `handle`.
pub type Handler = Option<Rc<Tag>>;

/// A handler identity. Entering a handler activates it under its parent and
/// records where leaving it resumes; the run that entered it owns it, so a
/// nested run leaving it aborts out to that owner.
#[derive(Debug, Default)]
pub struct Tag {
    pub active: Cell<bool>,
    pub parent: RefCell<Handler>,
    pub continuation: RefCell<Option<Value>>,
    pub owner: Cell<u64>,
}

/// A runtime type: a finite descriptor graph. A mirror is one a mirror
/// intrinsic made authentic; a plan is one a native template instantiated,
/// which may still hold parameters and knows which fields the host may omit.
#[derive(Debug)]
pub struct Type {
    pub nodes: Vec<Node>,
    pub plan: Option<Plan>,
    pub mirror: Cell<bool>,
}

#[derive(Debug)]
pub struct Plan {
    pub template: NativeTemplate,
    pub arguments: Vec<Value>,
    pub optional: BTreeMap<u32, BTreeSet<String>>,
}

/// One entry of the primitive table, with the arguments applied so far. A
/// curried extern is called one argument at a time; a marked one all at once.
#[derive(Debug)]
pub struct Primitive {
    pub target: Rc<str>,
    pub applied: Vec<Value>,
}

/// A function the interpreter itself implements.
#[derive(Debug)]
pub enum Native {
    /// A host value converted for Ruddy: the argument is converted out at
    /// `from`, the host value is called, and its result is converted in at
    /// `to`. `slots` are the plan's parameters a call must still supply.
    Incoming {
        plan: Rc<Type>,
        from: u32,
        to: u32,
        function: Value,
        slots: Vec<u32>,
    },
    /// A Ruddy closure converted for the host: the argument is converted in
    /// at `from`, the closure is called, and its result converted out at `to`.
    Outgoing {
        plan: Rc<Type>,
        from: u32,
        to: u32,
        inner: Value,
        callable: bool,
        host_export: bool,
    },
    /// An operation of a typed view. A mirror proved the type, so these
    /// convert nothing: they take values apart and put them together.
    Builtin(Builtin),
}

#[derive(Debug)]
pub enum Builtin {
    /// Hand the value back unchanged.
    PassThrough,
    /// Read one field of a record as `#Some`.
    ReadField(Rc<str>),
    /// Bind a value to one field of a record type, as evidence a builder checks.
    Bind {
        record: Rc<Type>,
        name: Rc<str>,
        field: Rc<Type>,
    },
    /// Build a record of a type from an array of bindings.
    Build(Rc<Type>),
    /// Project one case of a sum: its payload, or nothing.
    ProjectCase(Rc<str>),
    /// Inject a payload as one case of a sum.
    InjectCase(Rc<str>),
}

#[derive(Debug)]
pub struct Callback {
    pub inner: Value,
    pub mode: externs::Callback,
}

impl Value {
    pub fn unit() -> Self {
        Self::Record(Rc::new(Record::new()))
    }
    pub fn string(text: &str) -> Self {
        Self::Str(Rc::from(text))
    }
    pub fn record(fields: Vec<(&str, Value)>) -> Self {
        Self::Record(Rc::new(
            fields
                .into_iter()
                .map(|(name, value)| (Key::Named(Rc::from(name)), value))
                .collect(),
        ))
    }
    pub fn array(items: Vec<Value>) -> Self {
        Self::Array(Rc::new(items))
    }
    pub fn sum(tag: &str, payload: Option<Value>) -> Self {
        Self::Sum(Rc::new(Sum {
            tag: Rc::from(tag),
            payload,
        }))
    }
    pub fn some(value: Value) -> Self {
        Self::sum("Some", Some(value))
    }
    pub fn none() -> Self {
        Self::sum("None", None)
    }
    /// A pair, which is a record with the fields `0` and `1`.
    pub fn pair(first: Value, second: Value) -> Self {
        Self::record(vec![("0", first), ("1", second)])
    }

    /// What kind of value this is, for a message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Absent => "nothing",
            Self::Nat(_) => "a natural number",
            Self::Int(_) => "an integer",
            Self::Fixed(kind, _) => kind.name(),
            Self::Real(_) => "a real number",
            Self::Str(_) => "a string",
            Self::Bool(_) => "a boolean",
            Self::Record(_) => "a record",
            Self::Array(_) => "an array",
            Self::Sum(_) => "a sum",
            Self::Closure(_) => "a function",
            Self::Cont(_) => "a continuation",
            Self::Cell(_) => "a cell",
            Self::Tag(_) => "a handler identity",
            Self::Type(_) => "a runtime type",
            Self::Primitive(_) => "a primitive",
            Self::Native(_) => "a native function",
            Self::Package(_) => "a sealed package",
            Self::Callback(_) => "a callback",
        }
    }

    /// Whether a call can be made on this value.
    pub fn is_callable(&self) -> bool {
        matches!(
            self,
            Self::Closure(_) | Self::Primitive(_) | Self::Native(_) | Self::Callback(_)
        )
    }

    pub fn as_nat(&self) -> Result<u64, String> {
        match self {
            Self::Nat(value) => Ok(*value),
            Self::Int(value) if *value >= 0 => Ok(*value as u64),
            other => Err(format!("expected a natural number, found {}", other.kind())),
        }
    }
    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Self::Int(value) => Ok(*value),
            Self::Nat(value) => {
                i64::try_from(*value).map_err(|_| "integer out of range".to_string())
            }
            other => Err(format!("expected an integer, found {}", other.kind())),
        }
    }
    /// Any integer value, whatever its width, as a wide integer.
    pub fn as_integer(&self) -> Result<i128, String> {
        match self {
            Self::Nat(value) => Ok(i128::from(*value)),
            Self::Int(value) => Ok(i128::from(*value)),
            Self::Fixed(_, value) => Ok(*value),
            other => Err(format!("expected an integer, found {}", other.kind())),
        }
    }
    /// A real number, or an integer read as one.
    pub fn as_real(&self) -> Result<f64, String> {
        match self {
            Self::Real(value) => Ok(*value),
            Self::Nat(value) => Ok(*value as f64),
            Self::Int(value) => Ok(*value as f64),
            other => Err(format!("expected a real number, found {}", other.kind())),
        }
    }
    pub fn as_str(&self) -> Result<&str, String> {
        match self {
            Self::Str(value) => Ok(value),
            other => Err(format!("expected a string, found {}", other.kind())),
        }
    }
    pub fn as_bool(&self) -> Result<bool, String> {
        match self {
            Self::Bool(value) => Ok(*value),
            other => Err(format!("expected a boolean, found {}", other.kind())),
        }
    }
    pub fn as_record(&self) -> Result<&Rc<Record>, String> {
        match self {
            Self::Record(value) => Ok(value),
            other => Err(format!("expected a record, found {}", other.kind())),
        }
    }
    pub fn as_array(&self) -> Result<&Rc<Vec<Value>>, String> {
        match self {
            Self::Array(value) => Ok(value),
            other => Err(format!("expected an array, found {}", other.kind())),
        }
    }
    pub fn as_sum(&self) -> Result<&Rc<Sum>, String> {
        match self {
            Self::Sum(value) => Ok(value),
            other => Err(format!("expected a sum, found {}", other.kind())),
        }
    }
    pub fn as_type(&self) -> Result<&Rc<Type>, String> {
        match self {
            Self::Type(value) => Ok(value),
            other => Err(format!("expected a runtime type, found {}", other.kind())),
        }
    }

    /// Read a named field of a record, or nothing when it has no such field.
    pub fn field(&self, name: &str) -> Value {
        match self {
            Self::Record(record) => record
                .get(&Key::Named(Rc::from(name)))
                .cloned()
                .unwrap_or(Value::Absent),
            _ => Value::Absent,
        }
    }
}

impl Key {
    pub fn named(name: &str) -> Self {
        Self::Named(Rc::from(name))
    }
}
