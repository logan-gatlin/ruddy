#![allow(dead_code)]
pub mod instruction;
pub mod types;
pub mod validate;

#[cfg(test)]
mod tests;

pub use instruction::*;
pub use types::*;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub imports: Vec<Import>,
    pub globals: Vec<Global>,
    pub memory: Vec<MemoryType>,
    pub functions: Vec<Function>,
    pub start: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub type_: EntityType,
}

pub struct Export {
    name: String,
    type_: ExportType,
    index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Global {
    pub type_: GlobalType,
    pub init: Vec<Instruction>,
}

impl Global {
    pub fn init(mut self, i: impl IntoIterator<Item = Instruction>) -> Self {
        self.init = i.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Function {
    pub type_: FunctionType,
    pub locals: Vec<ValueType>,
    pub instructions: Vec<Instruction>,
}

impl Function {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn param(mut self, valtype: ValueType) -> Self {
        self.type_.params.push(valtype);
        self
    }
    pub fn result(mut self, valtype: ValueType) -> Self {
        self.type_.results.push(valtype);
        self
    }
    pub fn local(mut self, valtype: ValueType) -> Self {
        self.locals.push(valtype);
        self
    }
    pub fn instr(mut self, instr: Instruction) -> Self {
        self.instructions.push(instr);
        self
    }
    pub fn instrs(mut self, instrs: impl IntoIterator<Item = Instruction>) -> Self {
        self.instructions.extend(instrs);
        self
    }
    pub fn stub(mut self) -> Self {
        self.instructions = vec![Instruction::Unreachable];
        self
    }
}
