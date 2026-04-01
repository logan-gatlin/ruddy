use super::{ArrayType, BlockType, FunctionType, HeapType, RefType, StructType, ValueType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemArg {
    pub align: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// Stack: [] -> [] (traps immediately).
    Unreachable,
    /// Stack: [] -> [].
    Nop,
    /// Stack: [params*] -> [results*] according to `block_type`.
    /// Args: (`block_type`) signature describing block parameter and result types.
    Block(BlockType),
    /// Stack: [params*] -> [results*] according to `block_type`.
    /// Args: (`block_type`) signature describing loop parameter and result types.
    Loop(BlockType),
    /// Stack: [params*, i32] -> [results*] according to `block_type`.
    /// Args: (`block_type`) signature describing if-branch parameter and result types.
    If(BlockType),
    /// Stack: [] -> [] (marks the alternate `if` branch).
    Else,
    /// Stack: [] -> [] (closes a structured control block).
    End,
    /// Stack: [label_args*] -> [] (branches to `label_idx`, does not continue in-place).
    /// Args: (`label_idx`) relative label depth to branch to.
    Br(u32),
    /// Stack: [label_args*, i32] -> [] (branches when condition is non-zero).
    /// Args: (`label_idx`) relative label depth to branch to when condition is non-zero.
    BrIf(u32),
    /// Stack: [label_args*, i32] -> [] (branches by selector or `default_label_idx`).
    /// Args: (`label_indices`) selector-indexed branch label depths; (`default_label_idx`) fallback label depth.
    BrTable(Vec<u32>, u32),
    /// Stack: [result*] -> [] (returns from the current function).
    Return,
    /// Stack: [params*] -> [results*] as defined by the callee type.
    /// Args: (`function_idx`) index of the direct callee function.
    Call(u32),
    /// Stack: [params*, i32] -> [results*] as defined by `function_type`.
    /// Args: (`function_type`) expected indirect callee signature; (`table_idx`) table index for lookup.
    CallIndirect(FunctionType, u32),

    /// Stack: [] -> [ref.null heap_type].
    /// Args: (`heap_type`) heap type of the null reference produced.
    RefNull(HeapType),
    /// Stack: [ref] -> [i32].
    RefIsNull,
    /// Stack: [] -> [funcref].
    /// Args: (`function_idx`) index of the function reference to produce.
    RefFunc(u32),
    /// Stack: [eqref, eqref] -> [i32].
    RefEq,
    /// Stack: [ref null ht] -> [ref ht] (traps on null).
    RefAsNonNull,
    /// Stack: [ref null ht] -> [ref ht] (branches to `label_idx` when input is null).
    /// Args: (`label_idx`) relative label depth to branch to when reference is null.
    BrOnNull(u32),
    /// Stack: [ref null ht] -> [] (branches to `label_idx` with non-null ref on success).
    /// Args: (`label_idx`) relative label depth to branch to when reference is non-null.
    BrOnNonNull(u32),

    /// Stack: [t] -> [].
    Drop,
    /// Stack: [t, t, i32] -> [t].
    Select,
    /// Stack: [t, t, i32] -> [t], where `t` is constrained by `result_types`.
    /// Args: (`result_types`) allowed result value types for selection.
    SelectTyped(Vec<ValueType>),

    /// Stack: [] -> [t].
    /// Args: (`local_idx`) index of the local to read.
    LocalGet(u32),
    /// Stack: [t] -> [].
    /// Args: (`local_idx`) index of the local to write.
    LocalSet(u32),
    /// Stack: [t] -> [t].
    /// Args: (`local_idx`) index of the local to write while preserving the value on stack.
    LocalTee(u32),
    /// Stack: [] -> [t].
    /// Args: (`global_idx`) index of the global to read.
    GlobalGet(u32),
    /// Stack: [t] -> [].
    /// Args: (`global_idx`) index of the global to write.
    GlobalSet(u32),

    /// Stack: [i32] -> [ref].
    /// Args: (`table_idx`) index of the table to read from.
    TableGet(u32),
    /// Stack: [i32, ref] -> [].
    /// Args: (`table_idx`) index of the table to write to.
    TableSet(u32),
    /// Stack: [] -> [i32].
    /// Args: (`table_idx`) index of the table whose size is queried.
    TableSize(u32),
    /// Stack: [ref, i32] -> [i32].
    /// Args: (`table_idx`) index of the table to grow.
    TableGrow(u32),
    /// Stack: [i32, ref, i32] -> [].
    /// Args: (`table_idx`) index of the table to fill.
    TableFill(u32),
    /// Stack: [i32, i32, i32] -> [] (`dst`, `src`, `len`).
    /// Args: (`dst_table_idx`) destination table index; (`src_table_idx`) source table index.
    TableCopy(u32, u32),
    /// Stack: [i32, i32, i32] -> [] (`dst`, `src`, `len`).
    /// Args: (`elem_idx`) element segment index; (`table_idx`) destination table index.
    TableInit(u32, u32),
    /// Stack: [] -> [] (invalidates passive element segment).
    /// Args: (`elem_idx`) passive element segment index to invalidate.
    ElemDrop(u32),

    /// Stack: [i32] -> [i32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Load(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load(MemArg),
    /// Stack: [i32] -> [f32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    F32Load(MemArg),
    /// Stack: [i32] -> [f64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    F64Load(MemArg),
    /// Stack: [i32] -> [i32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Load8S(MemArg),
    /// Stack: [i32] -> [i32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Load8U(MemArg),
    /// Stack: [i32] -> [i32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Load16S(MemArg),
    /// Stack: [i32] -> [i32].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Load16U(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load8S(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load8U(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load16S(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load16U(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load32S(MemArg),
    /// Stack: [i32] -> [i64].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Load32U(MemArg),
    /// Stack: [i32, i32] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Store(MemArg),
    /// Stack: [i32, i64] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Store(MemArg),
    /// Stack: [i32, f32] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    F32Store(MemArg),
    /// Stack: [i32, f64] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    F64Store(MemArg),
    /// Stack: [i32, i32] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Store8(MemArg),
    /// Stack: [i32, i32] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I32Store16(MemArg),
    /// Stack: [i32, i64] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Store8(MemArg),
    /// Stack: [i32, i64] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Store16(MemArg),
    /// Stack: [i32, i64] -> [].
    /// Args: (`memarg`) memory immediate containing alignment and offset.
    I64Store32(MemArg),
    /// Stack: [] -> [i32].
    /// Args: (`memory_idx`) index of the memory whose current size is queried.
    MemorySize(u32),
    /// Stack: [i32] -> [i32].
    /// Args: (`memory_idx`) index of the memory to grow.
    MemoryGrow(u32),
    /// Stack: [i32, i32, i32] -> [] (`dst`, `src`, `len`).
    /// Args: (`data_idx`) data segment index; (`memory_idx`) destination memory index.
    MemoryInit(u32, u32),
    /// Stack: [] -> [] (invalidates passive data segment).
    /// Args: (`data_idx`) passive data segment index to invalidate.
    DataDrop(u32),
    /// Stack: [i32, i32, i32] -> [] (`dst`, `src`, `len`).
    /// Args: (`dst_memory_idx`) destination memory index; (`src_memory_idx`) source memory index.
    MemoryCopy(u32, u32),
    /// Stack: [i32, i32, i32] -> [] (`dst`, `value`, `len`).
    /// Args: (`memory_idx`) destination memory index to fill.
    MemoryFill(u32),

    /// Stack: [] -> [i32].
    /// Args: (`value`) i32 literal value.
    I32Const(i32),
    /// Stack: [] -> [i64].
    /// Args: (`value`) i64 literal value.
    I64Const(i64),
    /// Stack: [] -> [f32].
    /// Args: (`value`) f32 literal value.
    F32Const(f32),
    /// Stack: [] -> [f64].
    /// Args: (`value`) f64 literal value.
    F64Const(f64),

    /// Stack: [i32] -> [i32].
    I32Eqz,
    /// Stack: [i32, i32] -> [i32].
    I32Eq,
    /// Stack: [i32, i32] -> [i32].
    I32Ne,
    /// Stack: [i32, i32] -> [i32].
    I32LtS,
    /// Stack: [i32, i32] -> [i32].
    I32LtU,
    /// Stack: [i32, i32] -> [i32].
    I32GtS,
    /// Stack: [i32, i32] -> [i32].
    I32GtU,
    /// Stack: [i32, i32] -> [i32].
    I32LeS,
    /// Stack: [i32, i32] -> [i32].
    I32LeU,
    /// Stack: [i32, i32] -> [i32].
    I32GeS,
    /// Stack: [i32, i32] -> [i32].
    I32GeU,

    /// Stack: [i64] -> [i32].
    I64Eqz,
    /// Stack: [i64, i64] -> [i32].
    I64Eq,
    /// Stack: [i64, i64] -> [i32].
    I64Ne,
    /// Stack: [i64, i64] -> [i32].
    I64LtS,
    /// Stack: [i64, i64] -> [i32].
    I64LtU,
    /// Stack: [i64, i64] -> [i32].
    I64GtS,
    /// Stack: [i64, i64] -> [i32].
    I64GtU,
    /// Stack: [i64, i64] -> [i32].
    I64LeS,
    /// Stack: [i64, i64] -> [i32].
    I64LeU,
    /// Stack: [i64, i64] -> [i32].
    I64GeS,
    /// Stack: [i64, i64] -> [i32].
    I64GeU,

    /// Stack: [f32, f32] -> [i32].
    F32Eq,
    /// Stack: [f32, f32] -> [i32].
    F32Ne,
    /// Stack: [f32, f32] -> [i32].
    F32Lt,
    /// Stack: [f32, f32] -> [i32].
    F32Gt,
    /// Stack: [f32, f32] -> [i32].
    F32Le,
    /// Stack: [f32, f32] -> [i32].
    F32Ge,

    /// Stack: [f64, f64] -> [i32].
    F64Eq,
    /// Stack: [f64, f64] -> [i32].
    F64Ne,
    /// Stack: [f64, f64] -> [i32].
    F64Lt,
    /// Stack: [f64, f64] -> [i32].
    F64Gt,
    /// Stack: [f64, f64] -> [i32].
    F64Le,
    /// Stack: [f64, f64] -> [i32].
    F64Ge,

    /// Stack: [i32] -> [i32].
    I32Clz,
    /// Stack: [i32] -> [i32].
    I32Ctz,
    /// Stack: [i32] -> [i32].
    I32Popcnt,
    /// Stack: [i32, i32] -> [i32].
    I32Add,
    /// Stack: [i32, i32] -> [i32].
    I32Sub,
    /// Stack: [i32, i32] -> [i32].
    I32Mul,
    /// Stack: [i32, i32] -> [i32].
    I32DivS,
    /// Stack: [i32, i32] -> [i32].
    I32DivU,
    /// Stack: [i32, i32] -> [i32].
    I32RemS,
    /// Stack: [i32, i32] -> [i32].
    I32RemU,
    /// Stack: [i32, i32] -> [i32].
    I32And,
    /// Stack: [i32, i32] -> [i32].
    I32Or,
    /// Stack: [i32, i32] -> [i32].
    I32Xor,
    /// Stack: [i32, i32] -> [i32].
    I32Shl,
    /// Stack: [i32, i32] -> [i32].
    I32ShrS,
    /// Stack: [i32, i32] -> [i32].
    I32ShrU,
    /// Stack: [i32, i32] -> [i32].
    I32Rotl,
    /// Stack: [i32, i32] -> [i32].
    I32Rotr,

    /// Stack: [i64] -> [i64].
    I64Clz,
    /// Stack: [i64] -> [i64].
    I64Ctz,
    /// Stack: [i64] -> [i64].
    I64Popcnt,
    /// Stack: [i64, i64] -> [i64].
    I64Add,
    /// Stack: [i64, i64] -> [i64].
    I64Sub,
    /// Stack: [i64, i64] -> [i64].
    I64Mul,
    /// Stack: [i64, i64] -> [i64].
    I64DivS,
    /// Stack: [i64, i64] -> [i64].
    I64DivU,
    /// Stack: [i64, i64] -> [i64].
    I64RemS,
    /// Stack: [i64, i64] -> [i64].
    I64RemU,
    /// Stack: [i64, i64] -> [i64].
    I64And,
    /// Stack: [i64, i64] -> [i64].
    I64Or,
    /// Stack: [i64, i64] -> [i64].
    I64Xor,
    /// Stack: [i64, i64] -> [i64].
    I64Shl,
    /// Stack: [i64, i64] -> [i64].
    I64ShrS,
    /// Stack: [i64, i64] -> [i64].
    I64ShrU,
    /// Stack: [i64, i64] -> [i64].
    I64Rotl,
    /// Stack: [i64, i64] -> [i64].
    I64Rotr,

    /// Stack: [f32] -> [f32].
    F32Abs,
    /// Stack: [f32] -> [f32].
    F32Neg,
    /// Stack: [f32] -> [f32].
    F32Ceil,
    /// Stack: [f32] -> [f32].
    F32Floor,
    /// Stack: [f32] -> [f32].
    F32Trunc,
    /// Stack: [f32] -> [f32].
    F32Nearest,
    /// Stack: [f32] -> [f32].
    F32Sqrt,
    /// Stack: [f32, f32] -> [f32].
    F32Add,
    /// Stack: [f32, f32] -> [f32].
    F32Sub,
    /// Stack: [f32, f32] -> [f32].
    F32Mul,
    /// Stack: [f32, f32] -> [f32].
    F32Div,
    /// Stack: [f32, f32] -> [f32].
    F32Min,
    /// Stack: [f32, f32] -> [f32].
    F32Max,
    /// Stack: [f32, f32] -> [f32].
    F32Copysign,

    /// Stack: [f64] -> [f64].
    F64Abs,
    /// Stack: [f64] -> [f64].
    F64Neg,
    /// Stack: [f64] -> [f64].
    F64Ceil,
    /// Stack: [f64] -> [f64].
    F64Floor,
    /// Stack: [f64] -> [f64].
    F64Trunc,
    /// Stack: [f64] -> [f64].
    F64Nearest,
    /// Stack: [f64] -> [f64].
    F64Sqrt,
    /// Stack: [f64, f64] -> [f64].
    F64Add,
    /// Stack: [f64, f64] -> [f64].
    F64Sub,
    /// Stack: [f64, f64] -> [f64].
    F64Mul,
    /// Stack: [f64, f64] -> [f64].
    F64Div,
    /// Stack: [f64, f64] -> [f64].
    F64Min,
    /// Stack: [f64, f64] -> [f64].
    F64Max,
    /// Stack: [f64, f64] -> [f64].
    F64Copysign,

    /// Stack: [i64] -> [i32].
    I32WrapI64,
    /// Stack: [f32] -> [i32].
    I32TruncF32S,
    /// Stack: [f32] -> [i32].
    I32TruncF32U,
    /// Stack: [f64] -> [i32].
    I32TruncF64S,
    /// Stack: [f64] -> [i32].
    I32TruncF64U,
    /// Stack: [i32] -> [i64].
    I64ExtendI32S,
    /// Stack: [i32] -> [i64].
    I64ExtendI32U,
    /// Stack: [f32] -> [i64].
    I64TruncF32S,
    /// Stack: [f32] -> [i64].
    I64TruncF32U,
    /// Stack: [f64] -> [i64].
    I64TruncF64S,
    /// Stack: [f64] -> [i64].
    I64TruncF64U,
    /// Stack: [i32] -> [f32].
    F32ConvertI32S,
    /// Stack: [i32] -> [f32].
    F32ConvertI32U,
    /// Stack: [i64] -> [f32].
    F32ConvertI64S,
    /// Stack: [i64] -> [f32].
    F32ConvertI64U,
    /// Stack: [f64] -> [f32].
    F32DemoteF64,
    /// Stack: [i32] -> [f64].
    F64ConvertI32S,
    /// Stack: [i32] -> [f64].
    F64ConvertI32U,
    /// Stack: [i64] -> [f64].
    F64ConvertI64S,
    /// Stack: [i64] -> [f64].
    F64ConvertI64U,
    /// Stack: [f32] -> [f64].
    F64PromoteF32,
    /// Stack: [f32] -> [i32] (bit reinterpretation).
    I32ReinterpretF32,
    /// Stack: [f64] -> [i64] (bit reinterpretation).
    I64ReinterpretF64,
    /// Stack: [i32] -> [f32] (bit reinterpretation).
    F32ReinterpretI32,
    /// Stack: [i64] -> [f64] (bit reinterpretation).
    F64ReinterpretI64,
    /// Stack: [i32] -> [i32].
    I32Extend8S,
    /// Stack: [i32] -> [i32].
    I32Extend16S,
    /// Stack: [i64] -> [i64].
    I64Extend8S,
    /// Stack: [i64] -> [i64].
    I64Extend16S,
    /// Stack: [i64] -> [i64].
    I64Extend32S,
    /// Stack: [f32] -> [i32] (saturating conversion).
    I32TruncSatF32S,
    /// Stack: [f32] -> [i32] (saturating conversion).
    I32TruncSatF32U,
    /// Stack: [f64] -> [i32] (saturating conversion).
    I32TruncSatF64S,
    /// Stack: [f64] -> [i32] (saturating conversion).
    I32TruncSatF64U,
    /// Stack: [f32] -> [i64] (saturating conversion).
    I64TruncSatF32S,
    /// Stack: [f32] -> [i64] (saturating conversion).
    I64TruncSatF32U,
    /// Stack: [f64] -> [i64] (saturating conversion).
    I64TruncSatF64S,
    /// Stack: [f64] -> [i64] (saturating conversion).
    I64TruncSatF64U,

    /// Stack: [ref_value] -> [i32] (1 if `ref_value` matches `ref_type`, else 0).
    /// Args: (`ref_type`) reference type to test against.
    RefTest(RefType),
    /// Stack: [ref_value] -> [ref_type] (traps if `ref_value` does not match `ref_type`).
    /// Args: (`ref_type`) reference type to cast to.
    RefCast(RefType),
    /// Stack: [from_ref_type] -> [from_ref_type] on fallthrough; branches to `label_idx` with [to_ref_type] when cast succeeds.
    /// Args: (`label_idx`) relative label depth for successful cast branch; (`from_ref_type`) input reference type; (`to_ref_type`) target reference type.
    BrOnCast(u32, RefType, RefType),
    /// Stack: [from_ref_type] -> [to_ref_type] on fallthrough; branches to `label_idx` with [from_ref_type] when cast fails.
    /// Args: (`label_idx`) relative label depth for failed cast branch; (`from_ref_type`) input reference type; (`to_ref_type`) target reference type.
    BrOnCastFail(u32, RefType, RefType),

    /// Stack: [field_0, ..., field_n] -> [ref struct_type].
    /// Args: (`struct_type`) struct type describing field layout and result reference type.
    StructNew(StructType),
    /// Stack: [] -> [ref struct_type] (fields initialized to default values).
    /// Args: (`struct_type`) struct type describing default-initialized fields.
    StructNewDefault(StructType),
    /// Stack: [ref null struct_type] -> [field_value] (traps on null).
    /// Args: (`struct_type`) struct type to read from; (`field_idx`) index of the field to load.
    StructGet(StructType, u32),
    /// Stack: [ref null struct_type] -> [i32] (signed unpacked load, traps on null).
    /// Args: (`struct_type`) struct type to read from; (`field_idx`) index of the packed field to load.
    StructGetS(StructType, u32),
    /// Stack: [ref null struct_type] -> [i32] (unsigned unpacked load, traps on null).
    /// Args: (`struct_type`) struct type to read from; (`field_idx`) index of the packed field to load.
    StructGetU(StructType, u32),
    /// Stack: [ref null struct_type, field_value] -> [] (traps on null).
    /// Args: (`struct_type`) struct type to write to; (`field_idx`) index of the field to store.
    StructSet(StructType, u32),

    /// Stack: [element_value, i32] -> [ref array_type].
    /// Args: (`array_type`) array type describing element storage and result reference type.
    ArrayNew(ArrayType),
    /// Stack: [i32] -> [ref array_type] (elements initialized to default values).
    /// Args: (`array_type`) array type describing default element values.
    ArrayNewDefault(ArrayType),
    /// Stack: [element_value x `length`] -> [ref array_type].
    /// Args: (`array_type`) array type describing element storage; (`length`) number of stack-provided elements.
    ArrayNewFixed(ArrayType, u32),
    /// Stack: [i32, i32] -> [ref array_type] (`offset`, `length`).
    /// Args: (`array_type`) array type describing element storage; (`data_idx`) data segment index to copy from.
    ArrayNewData(ArrayType, u32),
    /// Stack: [i32, i32] -> [ref array_type] (`offset`, `length`).
    /// Args: (`array_type`) array type describing element storage; (`elem_idx`) element segment index to copy from.
    ArrayNewElem(ArrayType, u32),
    /// Stack: [ref null array_type, i32] -> [element_value] (traps on null or OOB).
    /// Args: (`array_type`) array type to read from.
    ArrayGet(ArrayType),
    /// Stack: [ref null array_type, i32] -> [i32] (signed unpacked load, traps on null or OOB).
    /// Args: (`array_type`) array type to read packed elements from.
    ArrayGetS(ArrayType),
    /// Stack: [ref null array_type, i32] -> [i32] (unsigned unpacked load, traps on null or OOB).
    /// Args: (`array_type`) array type to read packed elements from.
    ArrayGetU(ArrayType),
    /// Stack: [ref null array_type, i32, element_value] -> [] (traps on null or OOB).
    /// Args: (`array_type`) array type to write to.
    ArraySet(ArrayType),
    /// Stack: [ref null array] -> [i32] (traps on null).
    ArrayLen,
    /// Stack: [ref null array_type, i32, element_value, i32] -> [] (`start`, `value`, `length`).
    /// Args: (`array_type`) array type to fill.
    ArrayFill(ArrayType),
    /// Stack: [ref null dst_array_type, i32, ref null src_array_type, i32, i32] -> [] (`dst`, `src`, `length`).
    /// Args: (`dst_array_type`) destination array type; (`src_array_type`) source array type.
    ArrayCopy(ArrayType, ArrayType),
    /// Stack: [ref null array_type, i32, i32, i32] -> [] (`dst`, `src`, `length`).
    /// Args: (`array_type`) destination array type; (`data_idx`) data segment index to initialize from.
    ArrayInitData(ArrayType, u32),
    /// Stack: [ref null array_type, i32, i32, i32] -> [] (`dst`, `src`, `length`).
    /// Args: (`array_type`) destination array type; (`elem_idx`) element segment index to initialize from.
    ArrayInitElem(ArrayType, u32),

    /// Stack: [ref null extern] -> [ref null any].
    AnyConvertExtern,
    /// Stack: [ref null any] -> [ref null extern].
    ExternConvertAny,

    /// Stack: [i32] -> [ref i31].
    RefI31,
    /// Stack: [ref i31] -> [i32] (sign-extended from 31 bits).
    I31GetS,
    /// Stack: [ref i31] -> [i32] (zero-extended from 31 bits).
    I31GetU,
}
