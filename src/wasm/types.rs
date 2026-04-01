#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    I32,
    I64,
    F32,
    F64,
    V128,
    Ref(RefType),
}

impl ValueType {
    pub const fn storage(self) -> StorageType {
        StorageType::Value(self)
    }
    pub const fn block(self) -> BlockType {
        BlockType::Result(self)
    }
    pub fn global(self, mutable: bool) -> GlobalType {
        GlobalType {
            type_: self,
            mutable: mutable.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefType {
    pub nullable: bool,
    pub heap_type: HeapType,
}

impl RefType {
    pub const fn value(self) -> ValueType {
        ValueType::Ref(self)
    }
    pub const fn cast(self) -> super::Instruction {
        super::Instruction::RefCast(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HeapType {
    NoFunc,
    NoExtern,
    None,
    AnyFunc,
    Extern,
    #[default]
    Any,
    Eq,
    I31,
    AnyStruct,
    AnyArray,
    Func(Box<FunctionType>),
    Struct(Box<StructType>),
    Array(Box<ArrayType>),
}

impl HeapType {
    pub const fn ref_t(self, nullable: bool) -> RefType {
        RefType {
            nullable,
            heap_type: self,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedType {
    I8,
    I16,
}

impl PackedType {
    pub const fn storage(self) -> StorageType {
        StorageType::Packed(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageType {
    Value(ValueType),
    Packed(PackedType),
}

impl StorageType {
    pub fn field(self, mutable: bool) -> FieldType {
        FieldType {
            storage: self,
            mutability: mutable.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mutability {
    Immutable,
    #[default]
    Mutable,
}

impl From<bool> for Mutability {
    fn from(value: bool) -> Self {
        if value {
            Self::Mutable
        } else {
            Self::Immutable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldType {
    pub storage: StorageType,
    pub mutability: Mutability,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructType {
    pub fields: Vec<FieldType>,
}

impl StructType {
    pub fn heap(self) -> HeapType {
        HeapType::Struct(self.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayType {
    pub element: FieldType,
}

impl ArrayType {
    pub fn heap(self) -> HeapType {
        HeapType::Array(self.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunctionType {
    pub params: Vec<ValueType>,
    pub results: Vec<ValueType>,
}

impl FunctionType {
    pub fn heap(self) -> HeapType {
        HeapType::Func(self.into())
    }
    pub const fn block(self) -> BlockType {
        BlockType::Function(self)
    }
    pub const fn entity(self) -> EntityType {
        EntityType::Function(self)
    }
    pub fn def(self) -> super::Function {
        super::Function {
            type_: self,
            locals: vec![],
            instructions: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BlockType {
    #[default]
    Empty,
    Result(ValueType),
    Function(FunctionType),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalType {
    pub type_: ValueType,
    pub mutable: Mutability,
}

impl GlobalType {
    pub fn def(self) -> super::Global {
        super::Global {
            type_: self,
            init: vec![],
        }
    }
    pub const fn entity(self) -> EntityType {
        EntityType::Global(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityType {
    Function(FunctionType),
    Global(GlobalType),
    Memory(MemoryType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryType {
    pub min: u64,
    pub max: Option<u64>,
}

impl Default for MemoryType {
    fn default() -> Self {
        Self { min: 1, max: None }
    }
}

impl MemoryType {
    pub const PAGE_SIZE: u64 = 64000;

    pub fn fits(self, byte_size: u64) -> Self {
        Self {
            min: self.min.max(byte_size / Self::PAGE_SIZE),
            max: self.max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportType {
    Function,
    Memory,
    Global,
}
