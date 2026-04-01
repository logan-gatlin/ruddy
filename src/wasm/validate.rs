use super::{
    ArrayType, BlockType, FieldType, Function, Global, HeapType, Instruction, Mutability, RefType,
    StorageType, StructType, ValueType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub instruction_index: usize,
    pub kind: ValidationErrorKind,
    pub value_origins: Option<Vec<usize>>,
}

impl ValidationError {
    fn new(instruction_index: usize, kind: ValidationErrorKind) -> Self {
        Self {
            instruction_index,
            kind,
            value_origins: None,
        }
    }

    fn with_value_origins(mut self, value_origins: &[usize]) -> Self {
        if !value_origins.is_empty() {
            self.value_origins = Some(value_origins.to_vec());
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationErrorKind {
    UnsupportedInstruction,
    StackUnderflow,
    TypeMismatch,
    ExpectedReference,
    InvalidLabelDepth,
    BranchTableTypeMismatch,
    InvalidElsePlacement,
    InvalidEndPlacement,
    IfWithoutElseTypeMismatch,
    BranchStackHeightMismatch,
    UnclosedControlFrames,
    TopLevelUnreachable,
    UnknownStackValueAtEnd,
    InvalidLocalIndex,
    InvalidGlobalIndex,
    InvalidFunctionIndex,
    InvalidStructFieldIndex,
    ImmutableGlobal,
    SelectTypedArityMismatch,
    SelectTypeMismatch,
    BrOnNullLabelMismatch,
    BrOnNonNullLabelMismatch,
    BrOnNonNullTypeMismatch,
    BrOnCastTypeMismatch,
    BrOnCastFailTypeMismatch,
    ExpectedPackedStructField,
    ImmutableStructField,
    InvalidArrayFixedLength,
    ExpectedPackedArrayElement,
    ImmutableArrayElement,
    ArrayLenRequiresArrayReference,
    ArrayCopyTypeMismatch,
}

type ValidationResult<T> = Result<T, ValidationError>;

fn invalid_at<T>(instruction_index: usize, kind: ValidationErrorKind) -> ValidationResult<T> {
    Err(ValidationError::new(instruction_index, kind))
}

fn invalid_at_with_origins<T>(
    instruction_index: usize,
    kind: ValidationErrorKind,
    value_origins: &[usize],
) -> ValidationResult<T> {
    Err(ValidationError::new(instruction_index, kind).with_value_origins(value_origins))
}

pub fn arity(
    instructions: &[Instruction],
    starting_state: &[ValueType],
    locals: &[ValueType],
    globals: &[Global],
    funcs: &[Function],
) -> Result<Vec<ValueType>, ValidationError> {
    let mut validator = Validator::new(starting_state);
    for (instruction_index, instruction) in instructions.iter().enumerate() {
        validator.set_instruction_index(instruction_index);
        validator.step(instruction, locals, globals, funcs)?;
    }
    validator.set_instruction_index(instructions.len());
    validator.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ValueOrigins {
    instruction_indices: Vec<usize>,
}

impl ValueOrigins {
    fn merge(&self, other: &Self) -> Self {
        if self.instruction_indices.is_empty() {
            return other.clone();
        }
        if other.instruction_indices.is_empty() {
            return self.clone();
        }

        let mut instruction_indices = self.instruction_indices.clone();
        instruction_indices.extend(other.instruction_indices.iter().copied());
        instruction_indices.sort_unstable();
        instruction_indices.dedup();
        Self {
            instruction_indices,
        }
    }

    fn as_slice(&self) -> &[usize] {
        &self.instruction_indices
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StackValue {
    Known {
        ty: ValueType,
        origins: ValueOrigins,
    },
    Unknown {
        origins: ValueOrigins,
    },
}

impl StackValue {
    fn produced_by(instruction_index: usize, ty: ValueType) -> Self {
        Self::Known {
            ty,
            origins: ValueOrigins {
                instruction_indices: vec![instruction_index],
            },
        }
    }

    fn with_type_and_origins(ty: ValueType, origins: ValueOrigins) -> Self {
        Self::Known { ty, origins }
    }

    fn unknown() -> Self {
        Self::Unknown {
            origins: ValueOrigins::default(),
        }
    }

    fn origins(&self) -> &ValueOrigins {
        match self {
            StackValue::Known { origins, .. } | StackValue::Unknown { origins } => origins,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Block,
    Loop,
    If,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlFrame {
    kind: FrameKind,
    base_height: usize,
    params: Vec<ValueType>,
    param_values: Vec<StackValue>,
    results: Vec<ValueType>,
    then_results: Option<Vec<StackValue>>,
    unreachable: bool,
    parent_unreachable: bool,
    saw_else: bool,
}

impl ControlFrame {
    fn label_types(&self) -> &[ValueType] {
        match self.kind {
            FrameKind::Loop => &self.params,
            FrameKind::Block | FrameKind::If => &self.results,
        }
    }
}

#[derive(Debug, Clone)]
struct Validator {
    stack: Vec<StackValue>,
    frames: Vec<ControlFrame>,
    top_unreachable: bool,
    current_instruction: usize,
}

impl Validator {
    fn new(starting_state: &[ValueType]) -> Self {
        Self {
            stack: starting_state
                .iter()
                .cloned()
                .map(|ty| StackValue::with_type_and_origins(ty, ValueOrigins::default()))
                .collect(),
            frames: Vec::new(),
            top_unreachable: false,
            current_instruction: 0,
        }
    }

    fn set_instruction_index(&mut self, instruction_index: usize) {
        self.current_instruction = instruction_index;
    }

    fn error(&self, kind: ValidationErrorKind) -> ValidationError {
        ValidationError::new(self.current_instruction, kind)
    }

    fn invalid<T>(&self, kind: ValidationErrorKind) -> ValidationResult<T> {
        Err(self.error(kind))
    }

    fn invalid_with_origins<T>(
        &self,
        kind: ValidationErrorKind,
        value_origins: &ValueOrigins,
    ) -> ValidationResult<T> {
        Err(self
            .error(kind)
            .with_value_origins(value_origins.as_slice()))
    }

    fn step(
        &mut self,
        instruction: &Instruction,
        locals: &[ValueType],
        globals: &[Global],
        funcs: &[Function],
    ) -> ValidationResult<()> {
        match instruction {
            Instruction::Unreachable => {
                self.mark_unreachable();
            }
            Instruction::Nop => {}
            Instruction::Block(block_type) => {
                let (params, results) = block_signature(block_type);
                self.enter_frame(FrameKind::Block, params, results)?;
            }
            Instruction::Loop(block_type) => {
                let (params, results) = block_signature(block_type);
                self.enter_frame(FrameKind::Loop, params, results)?;
            }
            Instruction::If(block_type) => {
                self.pop_i32()?;
                let (params, results) = block_signature(block_type);
                self.enter_frame(FrameKind::If, params, results)?;
            }
            Instruction::Else => {
                self.handle_else()?;
            }
            Instruction::End => {
                self.handle_end()?;
            }
            Instruction::Br(label_idx) => {
                self.branch(*label_idx)?;
            }
            Instruction::BrIf(label_idx) => {
                self.branch_if(*label_idx)?;
            }
            Instruction::BrTable(label_indices, default_label_idx) => {
                self.branch_table(label_indices, *default_label_idx)?;
            }
            Instruction::CallIndirect(function_type, _table_idx) => {
                self.pop_i32()?;
                self.pop_types(&function_type.params)?;
                self.push_types(&function_type.results);
            }

            Instruction::Call(function_idx) => {
                let function = resolve_function(funcs, *function_idx, self.current_instruction)?;
                self.pop_types(&function.type_.params)?;
                self.push_types(&function.type_.results);
            }

            Instruction::RefNull(heap_type) => {
                self.push(ValueType::Ref(RefType {
                    nullable: true,
                    heap_type: heap_type.clone(),
                }));
            }
            Instruction::RefIsNull => {
                self.pop_any_reference()?;
                self.push_i32();
            }
            Instruction::RefFunc(_function_idx) => {
                self.push(any_func_ref(false));
            }
            Instruction::RefEq => {
                let eq_ref = eq_ref(true);
                self.pop_expected(&eq_ref)?;
                self.pop_expected(&eq_ref)?;
                self.push_i32();
            }
            Instruction::RefAsNonNull => {
                let value = self.pop_any_reference()?;
                match value {
                    StackValue::Unknown { origins } => {
                        self.push_stack_value(StackValue::Unknown { origins });
                    }
                    StackValue::Known {
                        ty: ValueType::Ref(mut ref_type),
                        origins,
                    } => {
                        ref_type.nullable = false;
                        self.push_with_origins(ValueType::Ref(ref_type), origins);
                    }
                    StackValue::Known { origins, .. } => {
                        return self.invalid_with_origins(
                            ValidationErrorKind::ExpectedReference,
                            &origins,
                        );
                    }
                }
            }
            Instruction::BrOnNull(label_idx) => {
                let label_types = self.label_types(*label_idx)?.to_vec();
                if !label_types.is_empty() {
                    return self.invalid(ValidationErrorKind::BrOnNullLabelMismatch);
                }

                let value = self.pop_any_reference()?;
                let fallthrough = match value {
                    StackValue::Unknown { origins } => StackValue::Unknown { origins },
                    StackValue::Known {
                        ty: ValueType::Ref(mut ref_type),
                        origins,
                    } => {
                        ref_type.nullable = false;
                        StackValue::with_type_and_origins(ValueType::Ref(ref_type), origins)
                    }
                    StackValue::Known { origins, .. } => {
                        return self.invalid_with_origins(
                            ValidationErrorKind::ExpectedReference,
                            &origins,
                        );
                    }
                };

                self.push_stack_value(fallthrough);
            }
            Instruction::BrOnNonNull(label_idx) => {
                let label_types = self.label_types(*label_idx)?.to_vec();
                if label_types.len() != 1 {
                    return self.invalid(ValidationErrorKind::BrOnNonNullLabelMismatch);
                }

                let value = self.pop_any_reference()?;
                let branch_arg = match value {
                    StackValue::Unknown { origins } => StackValue::Unknown { origins },
                    StackValue::Known {
                        ty: ValueType::Ref(mut ref_type),
                        origins,
                    } => {
                        ref_type.nullable = false;
                        StackValue::with_type_and_origins(ValueType::Ref(ref_type), origins)
                    }
                    StackValue::Known { origins, .. } => {
                        return self.invalid_with_origins(
                            ValidationErrorKind::ExpectedReference,
                            &origins,
                        );
                    }
                };

                if !stack_value_matches_expected(&branch_arg, &label_types[0]) {
                    return self.invalid_with_origins(
                        ValidationErrorKind::BrOnNonNullTypeMismatch,
                        branch_arg.origins(),
                    );
                }
            }

            Instruction::Drop => {
                self.pop_raw()?;
            }
            Instruction::Select => {
                self.pop_i32()?;
                let rhs = self.pop_raw()?;
                let lhs = self.pop_raw()?;
                self.push_stack_value(merge_select_values(lhs, rhs, self.current_instruction)?);
            }
            Instruction::SelectTyped(result_types) => {
                if result_types.len() != 1 {
                    return self.invalid(ValidationErrorKind::SelectTypedArityMismatch);
                }
                let selected = &result_types[0];
                self.pop_i32()?;
                self.pop_expected(selected)?;
                self.pop_expected(selected)?;
                self.push(selected.clone());
            }

            Instruction::LocalGet(local_idx) => {
                let local_type = resolve_local_type(locals, *local_idx, self.current_instruction)?;
                self.push(local_type.clone());
            }
            Instruction::LocalSet(local_idx) => {
                let local_type = resolve_local_type(locals, *local_idx, self.current_instruction)?;
                self.pop_expected(local_type)?;
            }
            Instruction::LocalTee(local_idx) => {
                let local_type = resolve_local_type(locals, *local_idx, self.current_instruction)?;
                let value = self.pop_expected_value(local_type)?;
                self.push_stack_value(value);
            }
            Instruction::GlobalGet(global_idx) => {
                let global = resolve_global(globals, *global_idx, self.current_instruction)?;
                self.push(global.type_.type_.clone());
            }
            Instruction::GlobalSet(global_idx) => {
                let global = resolve_global(globals, *global_idx, self.current_instruction)?;
                if global.type_.mutable != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableGlobal);
                }
                self.pop_expected(&global.type_.type_)?;
            }

            Instruction::TableGet(_table_idx) => {
                self.pop_i32()?;
                self.push(any_ref(true));
            }
            Instruction::TableSet(_table_idx) => {
                self.pop_any_reference()?;
                self.pop_i32()?;
            }
            Instruction::TableSize(_table_idx) => {
                self.push_i32();
            }
            Instruction::TableGrow(_table_idx) => {
                self.pop_any_reference()?;
                self.pop_i32()?;
                self.push_i32();
            }
            Instruction::TableFill(_table_idx) => {
                self.pop_i32()?;
                self.pop_any_reference()?;
                self.pop_i32()?;
            }
            Instruction::TableCopy(_dst_table_idx, _src_table_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
            }
            Instruction::TableInit(_elem_idx, _table_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
            }
            Instruction::ElemDrop(_elem_idx) => {}

            Instruction::I32Load(_)
            | Instruction::I32Load8S(_)
            | Instruction::I32Load8U(_)
            | Instruction::I32Load16S(_)
            | Instruction::I32Load16U(_) => {
                self.pop_i32()?;
                self.push_i32();
            }
            Instruction::I64Load(_)
            | Instruction::I64Load8S(_)
            | Instruction::I64Load8U(_)
            | Instruction::I64Load16S(_)
            | Instruction::I64Load16U(_)
            | Instruction::I64Load32S(_)
            | Instruction::I64Load32U(_) => {
                self.pop_i32()?;
                self.push_i64();
            }
            Instruction::F32Load(_) => {
                self.pop_i32()?;
                self.push_f32();
            }
            Instruction::F64Load(_) => {
                self.pop_i32()?;
                self.push_f64();
            }

            Instruction::I32Store(_) | Instruction::I32Store8(_) | Instruction::I32Store16(_) => {
                self.pop_i32()?;
                self.pop_i32()?;
            }
            Instruction::I64Store(_)
            | Instruction::I64Store8(_)
            | Instruction::I64Store16(_)
            | Instruction::I64Store32(_) => {
                self.pop_i64()?;
                self.pop_i32()?;
            }
            Instruction::F32Store(_) => {
                self.pop_f32()?;
                self.pop_i32()?;
            }
            Instruction::F64Store(_) => {
                self.pop_f64()?;
                self.pop_i32()?;
            }

            Instruction::MemorySize(_memory_idx) => {
                self.push_i32();
            }
            Instruction::MemoryGrow(_memory_idx) => {
                self.pop_i32()?;
                self.push_i32();
            }
            Instruction::MemoryInit(_data_idx, _memory_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
            }
            Instruction::DataDrop(_data_idx) => {}
            Instruction::MemoryCopy(_dst_memory_idx, _src_memory_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
            }
            Instruction::MemoryFill(_memory_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
            }

            Instruction::I32Const(_) => self.push_i32(),
            Instruction::I64Const(_) => self.push_i64(),
            Instruction::F32Const(_) => self.push_f32(),
            Instruction::F64Const(_) => self.push_f64(),

            Instruction::I32Eqz
            | Instruction::I32Clz
            | Instruction::I32Ctz
            | Instruction::I32Popcnt
            | Instruction::I32Extend8S
            | Instruction::I32Extend16S => {
                self.pop_i32()?;
                self.push_i32();
            }
            Instruction::I32Eq
            | Instruction::I32Ne
            | Instruction::I32LtS
            | Instruction::I32LtU
            | Instruction::I32GtS
            | Instruction::I32GtU
            | Instruction::I32LeS
            | Instruction::I32LeU
            | Instruction::I32GeS
            | Instruction::I32GeU => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.push_i32();
            }
            Instruction::I32Add
            | Instruction::I32Sub
            | Instruction::I32Mul
            | Instruction::I32DivS
            | Instruction::I32DivU
            | Instruction::I32RemS
            | Instruction::I32RemU
            | Instruction::I32And
            | Instruction::I32Or
            | Instruction::I32Xor
            | Instruction::I32Shl
            | Instruction::I32ShrS
            | Instruction::I32ShrU
            | Instruction::I32Rotl
            | Instruction::I32Rotr => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.push_i32();
            }

            Instruction::I64Eqz => {
                self.pop_i64()?;
                self.push_i32();
            }
            Instruction::I64Eq
            | Instruction::I64Ne
            | Instruction::I64LtS
            | Instruction::I64LtU
            | Instruction::I64GtS
            | Instruction::I64GtU
            | Instruction::I64LeS
            | Instruction::I64LeU
            | Instruction::I64GeS
            | Instruction::I64GeU => {
                self.pop_i64()?;
                self.pop_i64()?;
                self.push_i32();
            }
            Instruction::I64Clz
            | Instruction::I64Ctz
            | Instruction::I64Popcnt
            | Instruction::I64Extend8S
            | Instruction::I64Extend16S
            | Instruction::I64Extend32S => {
                self.pop_i64()?;
                self.push_i64();
            }
            Instruction::I64Add
            | Instruction::I64Sub
            | Instruction::I64Mul
            | Instruction::I64DivS
            | Instruction::I64DivU
            | Instruction::I64RemS
            | Instruction::I64RemU
            | Instruction::I64And
            | Instruction::I64Or
            | Instruction::I64Xor
            | Instruction::I64Shl
            | Instruction::I64ShrS
            | Instruction::I64ShrU
            | Instruction::I64Rotl
            | Instruction::I64Rotr => {
                self.pop_i64()?;
                self.pop_i64()?;
                self.push_i64();
            }

            Instruction::F32Eq
            | Instruction::F32Ne
            | Instruction::F32Lt
            | Instruction::F32Gt
            | Instruction::F32Le
            | Instruction::F32Ge => {
                self.pop_f32()?;
                self.pop_f32()?;
                self.push_i32();
            }
            Instruction::F32Abs
            | Instruction::F32Neg
            | Instruction::F32Ceil
            | Instruction::F32Floor
            | Instruction::F32Trunc
            | Instruction::F32Nearest
            | Instruction::F32Sqrt => {
                self.pop_f32()?;
                self.push_f32();
            }
            Instruction::F32Add
            | Instruction::F32Sub
            | Instruction::F32Mul
            | Instruction::F32Div
            | Instruction::F32Min
            | Instruction::F32Max
            | Instruction::F32Copysign => {
                self.pop_f32()?;
                self.pop_f32()?;
                self.push_f32();
            }

            Instruction::F64Eq
            | Instruction::F64Ne
            | Instruction::F64Lt
            | Instruction::F64Gt
            | Instruction::F64Le
            | Instruction::F64Ge => {
                self.pop_f64()?;
                self.pop_f64()?;
                self.push_i32();
            }
            Instruction::F64Abs
            | Instruction::F64Neg
            | Instruction::F64Ceil
            | Instruction::F64Floor
            | Instruction::F64Trunc
            | Instruction::F64Nearest
            | Instruction::F64Sqrt => {
                self.pop_f64()?;
                self.push_f64();
            }
            Instruction::F64Add
            | Instruction::F64Sub
            | Instruction::F64Mul
            | Instruction::F64Div
            | Instruction::F64Min
            | Instruction::F64Max
            | Instruction::F64Copysign => {
                self.pop_f64()?;
                self.pop_f64()?;
                self.push_f64();
            }

            Instruction::I32WrapI64 => {
                self.pop_i64()?;
                self.push_i32();
            }
            Instruction::I32TruncF32S
            | Instruction::I32TruncF32U
            | Instruction::I32TruncSatF32S
            | Instruction::I32TruncSatF32U => {
                self.pop_f32()?;
                self.push_i32();
            }
            Instruction::I32TruncF64S
            | Instruction::I32TruncF64U
            | Instruction::I32TruncSatF64S
            | Instruction::I32TruncSatF64U => {
                self.pop_f64()?;
                self.push_i32();
            }

            Instruction::I64ExtendI32S | Instruction::I64ExtendI32U => {
                self.pop_i32()?;
                self.push_i64();
            }
            Instruction::I64TruncF32S
            | Instruction::I64TruncF32U
            | Instruction::I64TruncSatF32S
            | Instruction::I64TruncSatF32U => {
                self.pop_f32()?;
                self.push_i64();
            }
            Instruction::I64TruncF64S
            | Instruction::I64TruncF64U
            | Instruction::I64TruncSatF64S
            | Instruction::I64TruncSatF64U => {
                self.pop_f64()?;
                self.push_i64();
            }

            Instruction::F32ConvertI32S | Instruction::F32ConvertI32U => {
                self.pop_i32()?;
                self.push_f32();
            }
            Instruction::F32ConvertI64S | Instruction::F32ConvertI64U => {
                self.pop_i64()?;
                self.push_f32();
            }
            Instruction::F32DemoteF64 => {
                self.pop_f64()?;
                self.push_f32();
            }

            Instruction::F64ConvertI32S | Instruction::F64ConvertI32U => {
                self.pop_i32()?;
                self.push_f64();
            }
            Instruction::F64ConvertI64S | Instruction::F64ConvertI64U => {
                self.pop_i64()?;
                self.push_f64();
            }
            Instruction::F64PromoteF32 => {
                self.pop_f32()?;
                self.push_f64();
            }

            Instruction::I32ReinterpretF32 => {
                self.pop_f32()?;
                self.push_i32();
            }
            Instruction::I64ReinterpretF64 => {
                self.pop_f64()?;
                self.push_i64();
            }
            Instruction::F32ReinterpretI32 => {
                self.pop_i32()?;
                self.push_f32();
            }
            Instruction::F64ReinterpretI64 => {
                self.pop_i64()?;
                self.push_f64();
            }

            Instruction::RefTest(_ref_type) => {
                self.pop_any_reference()?;
                self.push_i32();
            }
            Instruction::RefCast(ref_type) => {
                self.pop_any_reference()?;
                self.push(ValueType::Ref(ref_type.clone()));
            }
            Instruction::BrOnCast(label_idx, from_ref_type, to_ref_type) => {
                let from = ValueType::Ref(from_ref_type.clone());
                let to = ValueType::Ref(to_ref_type.clone());

                self.pop_expected(&from)?;

                let label_types = self.label_types(*label_idx)?.to_vec();
                if label_types.len() != 1 || !value_type_matches(&to, &label_types[0]) {
                    return self.invalid(ValidationErrorKind::BrOnCastTypeMismatch);
                }

                self.push(from);
            }
            Instruction::BrOnCastFail(label_idx, from_ref_type, to_ref_type) => {
                let from = ValueType::Ref(from_ref_type.clone());
                let to = ValueType::Ref(to_ref_type.clone());

                self.pop_expected(&from)?;

                let label_types = self.label_types(*label_idx)?.to_vec();
                if label_types.len() != 1 || !value_type_matches(&from, &label_types[0]) {
                    return self.invalid(ValidationErrorKind::BrOnCastFailTypeMismatch);
                }

                self.push(to);
            }

            Instruction::StructNew(struct_type) => {
                for field in struct_type.fields.iter().rev() {
                    self.pop_expected(&field_input_type(field))?;
                }
                self.push(struct_ref(struct_type, false));
            }
            Instruction::StructNewDefault(struct_type) => {
                self.push(struct_ref(struct_type, false));
            }
            Instruction::StructGet(struct_type, field_idx) => {
                let field = struct_field(struct_type, *field_idx, self.current_instruction)?;
                self.pop_expected(&struct_ref(struct_type, true))?;
                self.push(field_output_type(field));
            }
            Instruction::StructGetS(struct_type, field_idx)
            | Instruction::StructGetU(struct_type, field_idx) => {
                let field = struct_field(struct_type, *field_idx, self.current_instruction)?;
                if !matches!(field.storage, StorageType::Packed(_)) {
                    return self.invalid(ValidationErrorKind::ExpectedPackedStructField);
                }

                self.pop_expected(&struct_ref(struct_type, true))?;
                self.push_i32();
            }
            Instruction::StructSet(struct_type, field_idx) => {
                let field = struct_field(struct_type, *field_idx, self.current_instruction)?;
                if field.mutability != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableStructField);
                }

                self.pop_expected(&field_input_type(field))?;
                self.pop_expected(&struct_ref(struct_type, true))?;
            }

            Instruction::ArrayNew(array_type) => {
                self.pop_i32()?;
                self.pop_expected(&array_element_input_type(array_type))?;
                self.push(array_ref(array_type, false));
            }
            Instruction::ArrayNewDefault(array_type) => {
                self.pop_i32()?;
                self.push(array_ref(array_type, false));
            }
            Instruction::ArrayNewFixed(array_type, length) => {
                let length = usize::try_from(*length)
                    .map_err(|_| self.error(ValidationErrorKind::InvalidArrayFixedLength))?;
                let element_type = array_element_input_type(array_type);
                for _ in 0..length {
                    self.pop_expected(&element_type)?;
                }
                self.push(array_ref(array_type, false));
            }
            Instruction::ArrayNewData(array_type, _data_idx)
            | Instruction::ArrayNewElem(array_type, _data_idx) => {
                self.pop_i32()?;
                self.pop_i32()?;
                self.push(array_ref(array_type, false));
            }
            Instruction::ArrayGet(array_type) => {
                self.pop_i32()?;
                self.pop_expected(&array_ref(array_type, true))?;
                self.push(array_element_output_type(array_type));
            }
            Instruction::ArrayGetS(array_type) | Instruction::ArrayGetU(array_type) => {
                if !matches!(array_type.element.storage, StorageType::Packed(_)) {
                    return self.invalid(ValidationErrorKind::ExpectedPackedArrayElement);
                }

                self.pop_i32()?;
                self.pop_expected(&array_ref(array_type, true))?;
                self.push_i32();
            }
            Instruction::ArraySet(array_type) => {
                if array_type.element.mutability != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableArrayElement);
                }

                self.pop_expected(&array_element_input_type(array_type))?;
                self.pop_i32()?;
                self.pop_expected(&array_ref(array_type, true))?;
            }
            Instruction::ArrayLen => {
                let value = self.pop_any_reference()?;
                match value {
                    StackValue::Unknown { .. } => {}
                    StackValue::Known {
                        ty: ref_type @ ValueType::Ref(_),
                        origins,
                    } => {
                        if !is_array_reference_type(&ref_type) {
                            return self.invalid_with_origins(
                                ValidationErrorKind::ArrayLenRequiresArrayReference,
                                &origins,
                            );
                        }
                    }
                    StackValue::Known { origins, .. } => {
                        return self.invalid_with_origins(
                            ValidationErrorKind::ExpectedReference,
                            &origins,
                        );
                    }
                }
                self.push_i32();
            }
            Instruction::ArrayFill(array_type) => {
                if array_type.element.mutability != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableArrayElement);
                }

                self.pop_i32()?;
                self.pop_expected(&array_element_input_type(array_type))?;
                self.pop_i32()?;
                self.pop_expected(&array_ref(array_type, true))?;
            }
            Instruction::ArrayCopy(dst_array_type, src_array_type) => {
                if dst_array_type.element.mutability != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableArrayElement);
                }

                let src_out = array_element_output_type(src_array_type);
                let dst_in = array_element_input_type(dst_array_type);
                if !value_type_matches(&src_out, &dst_in) {
                    return self.invalid(ValidationErrorKind::ArrayCopyTypeMismatch);
                }

                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_expected(&array_ref(src_array_type, true))?;
                self.pop_i32()?;
                self.pop_expected(&array_ref(dst_array_type, true))?;
            }
            Instruction::ArrayInitData(array_type, _data_idx)
            | Instruction::ArrayInitElem(array_type, _data_idx) => {
                if array_type.element.mutability != Mutability::Mutable {
                    return self.invalid(ValidationErrorKind::ImmutableArrayElement);
                }

                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_i32()?;
                self.pop_expected(&array_ref(array_type, true))?;
            }

            Instruction::AnyConvertExtern => {
                self.pop_expected(&extern_ref(true))?;
                self.push(any_ref(true));
            }
            Instruction::ExternConvertAny => {
                self.pop_expected(&any_ref(true))?;
                self.push(extern_ref(true));
            }

            Instruction::RefI31 => {
                self.pop_i32()?;
                self.push(i31_ref(false));
            }
            Instruction::I31GetS | Instruction::I31GetU => {
                self.pop_expected(&i31_ref(false))?;
                self.push_i32();
            }

            Instruction::Return => {
                return self.invalid(ValidationErrorKind::UnsupportedInstruction);
            }
        }

        Ok(())
    }

    fn finish(self) -> ValidationResult<Vec<ValueType>> {
        let instruction_index = self.current_instruction;
        if !self.frames.is_empty() {
            return invalid_at(
                instruction_index,
                ValidationErrorKind::UnclosedControlFrames,
            );
        }
        if self.top_unreachable {
            return invalid_at(instruction_index, ValidationErrorKind::TopLevelUnreachable);
        }

        let mut out = Vec::with_capacity(self.stack.len());
        for value in self.stack {
            match value {
                StackValue::Known { ty, .. } => out.push(ty),
                StackValue::Unknown { origins } => {
                    return invalid_at_with_origins(
                        instruction_index,
                        ValidationErrorKind::UnknownStackValueAtEnd,
                        origins.as_slice(),
                    );
                }
            }
        }

        Ok(out)
    }

    fn push(&mut self, value: ValueType) {
        self.push_stack_value(StackValue::produced_by(self.current_instruction, value));
    }

    fn push_with_origins(&mut self, value: ValueType, origins: ValueOrigins) {
        self.push_stack_value(StackValue::with_type_and_origins(value, origins));
    }

    fn push_types(&mut self, types: &[ValueType]) {
        for ty in types {
            self.push(ty.clone());
        }
    }

    fn push_stack_value(&mut self, value: StackValue) {
        self.stack.push(value);
    }

    fn push_stack_values(&mut self, values: impl IntoIterator<Item = StackValue>) {
        self.stack.extend(values);
    }

    fn pop_raw(&mut self) -> ValidationResult<StackValue> {
        let base_height = self.current_base_height();
        if self.stack.len() == base_height {
            if self.current_unreachable() {
                return Ok(StackValue::unknown());
            }
            return self.invalid(ValidationErrorKind::StackUnderflow);
        }

        self.stack
            .pop()
            .ok_or_else(|| self.error(ValidationErrorKind::StackUnderflow))
    }

    fn pop_expected_value(&mut self, expected: &ValueType) -> ValidationResult<StackValue> {
        let value = self.pop_raw()?;
        match value {
            StackValue::Unknown { .. } => Ok(value),
            StackValue::Known { ref ty, .. } if value_type_matches(ty, expected) => Ok(value),
            StackValue::Known { origins, .. } => {
                self.invalid_with_origins(ValidationErrorKind::TypeMismatch, &origins)
            }
        }
    }

    fn pop_expected(&mut self, expected: &ValueType) -> ValidationResult<()> {
        self.pop_expected_value(expected).map(|_| ())
    }

    fn pop_types(&mut self, types: &[ValueType]) -> ValidationResult<()> {
        for ty in types.iter().rev() {
            self.pop_expected(ty)?;
        }
        Ok(())
    }

    fn pop_types_values(&mut self, types: &[ValueType]) -> ValidationResult<Vec<StackValue>> {
        let mut values = Vec::with_capacity(types.len());
        for ty in types.iter().rev() {
            values.push(self.pop_expected_value(ty)?);
        }
        values.reverse();
        Ok(values)
    }

    fn pop_any_reference(&mut self) -> ValidationResult<StackValue> {
        match self.pop_raw()? {
            StackValue::Unknown { origins } => Ok(StackValue::Unknown { origins }),
            StackValue::Known {
                ty: value @ ValueType::Ref(_),
                origins,
            } => Ok(StackValue::Known { ty: value, origins }),
            StackValue::Known { origins, .. } => {
                self.invalid_with_origins(ValidationErrorKind::ExpectedReference, &origins)
            }
        }
    }

    fn pop_i32(&mut self) -> ValidationResult<()> {
        self.pop_expected(&ValueType::I32)
    }

    fn pop_i64(&mut self) -> ValidationResult<()> {
        self.pop_expected(&ValueType::I64)
    }

    fn pop_f32(&mut self) -> ValidationResult<()> {
        self.pop_expected(&ValueType::F32)
    }

    fn pop_f64(&mut self) -> ValidationResult<()> {
        self.pop_expected(&ValueType::F64)
    }

    fn push_i32(&mut self) {
        self.push(ValueType::I32);
    }

    fn push_i64(&mut self) {
        self.push(ValueType::I64);
    }

    fn push_f32(&mut self) {
        self.push(ValueType::F32);
    }

    fn push_f64(&mut self) {
        self.push(ValueType::F64);
    }

    fn current_base_height(&self) -> usize {
        self.frames.last().map_or(0, |frame| frame.base_height)
    }

    fn current_unreachable(&self) -> bool {
        self.frames
            .last()
            .map_or(self.top_unreachable, |frame| frame.unreachable)
    }

    fn mark_unreachable(&mut self) {
        let base_height = self.current_base_height();
        self.stack.truncate(base_height);

        if let Some(frame) = self.frames.last_mut() {
            frame.unreachable = true;
        } else {
            self.top_unreachable = true;
        }
    }

    fn enter_frame(
        &mut self,
        kind: FrameKind,
        params: Vec<ValueType>,
        results: Vec<ValueType>,
    ) -> ValidationResult<()> {
        let parent_unreachable = self.current_unreachable();
        let param_values = self.pop_types_values(&params)?;

        let base_height = self.stack.len();
        self.push_stack_values(param_values.iter().cloned());

        self.frames.push(ControlFrame {
            kind,
            base_height,
            params,
            param_values,
            results,
            then_results: None,
            unreachable: parent_unreachable,
            parent_unreachable,
            saw_else: false,
        });

        Ok(())
    }

    fn label_types(&self, depth: u32) -> ValidationResult<&[ValueType]> {
        let depth = usize::try_from(depth)
            .map_err(|_| self.error(ValidationErrorKind::InvalidLabelDepth))?;
        if depth >= self.frames.len() {
            return self.invalid(ValidationErrorKind::InvalidLabelDepth);
        }

        let idx = self
            .frames
            .len()
            .checked_sub(depth + 1)
            .ok_or_else(|| self.error(ValidationErrorKind::InvalidLabelDepth))?;
        Ok(self
            .frames
            .get(idx)
            .ok_or_else(|| self.error(ValidationErrorKind::InvalidLabelDepth))?
            .label_types())
    }

    fn branch(&mut self, depth: u32) -> ValidationResult<()> {
        let label_types = self.label_types(depth)?.to_vec();
        self.pop_types(&label_types)?;
        self.mark_unreachable();
        Ok(())
    }

    fn branch_if(&mut self, depth: u32) -> ValidationResult<()> {
        self.pop_i32()?;

        let label_types = self.label_types(depth)?.to_vec();
        let values = self.pop_types_values(&label_types)?;
        self.push_stack_values(values);
        Ok(())
    }

    fn branch_table(
        &mut self,
        label_indices: &[u32],
        default_label_idx: u32,
    ) -> ValidationResult<()> {
        let default_types = self.label_types(default_label_idx)?.to_vec();
        for label_idx in label_indices {
            if self.label_types(*label_idx)? != default_types.as_slice() {
                return self.invalid(ValidationErrorKind::BranchTableTypeMismatch);
            }
        }

        self.pop_i32()?;
        self.pop_types(&default_types)?;
        self.mark_unreachable();
        Ok(())
    }

    fn collect_current_branch_results(
        &mut self,
        frame: &ControlFrame,
    ) -> ValidationResult<Vec<StackValue>> {
        let result_values = self.pop_types_values(&frame.results)?;

        if self.stack.len() != frame.base_height {
            return self.invalid(ValidationErrorKind::BranchStackHeightMismatch);
        }

        Ok(result_values)
    }

    fn handle_else(&mut self) -> ValidationResult<()> {
        let frame = self
            .frames
            .last()
            .cloned()
            .ok_or_else(|| self.error(ValidationErrorKind::InvalidElsePlacement))?;
        if frame.kind != FrameKind::If || frame.saw_else {
            return self.invalid(ValidationErrorKind::InvalidElsePlacement);
        }

        let then_results = self.collect_current_branch_results(&frame)?;

        self.stack.truncate(frame.base_height);
        self.push_stack_values(frame.param_values.iter().cloned());

        let Some(top) = self.frames.last_mut() else {
            return self.invalid(ValidationErrorKind::InvalidElsePlacement);
        };
        top.saw_else = true;
        top.unreachable = top.parent_unreachable;
        top.then_results = Some(then_results);
        Ok(())
    }

    fn handle_end(&mut self) -> ValidationResult<()> {
        let frame = self
            .frames
            .last()
            .cloned()
            .ok_or_else(|| self.error(ValidationErrorKind::InvalidEndPlacement))?;

        if frame.kind == FrameKind::If
            && !frame.saw_else
            && !frame.parent_unreachable
            && frame.params != frame.results
        {
            return self.invalid(ValidationErrorKind::IfWithoutElseTypeMismatch);
        }

        let mut result_values = self.collect_current_branch_results(&frame)?;

        if frame.kind == FrameKind::If && frame.saw_else {
            let then_results = frame
                .then_results
                .clone()
                .ok_or_else(|| self.error(ValidationErrorKind::InvalidEndPlacement))?;

            if then_results.len() != result_values.len()
                || result_values.len() != frame.results.len()
            {
                return self.invalid(ValidationErrorKind::BranchStackHeightMismatch);
            }

            result_values = merge_if_result_values(&frame.results, then_results, result_values);
        }

        self.frames.pop();

        self.stack.truncate(frame.base_height);
        self.push_stack_values(result_values);
        Ok(())
    }
}

fn merge_if_result_values(
    result_types: &[ValueType],
    then_results: Vec<StackValue>,
    else_results: Vec<StackValue>,
) -> Vec<StackValue> {
    result_types
        .iter()
        .cloned()
        .zip(then_results)
        .zip(else_results)
        .map(|((result_type, then_value), else_value)| {
            let origins = then_value.origins().merge(else_value.origins());

            match (then_value, else_value) {
                (StackValue::Unknown { .. }, StackValue::Unknown { .. }) => {
                    StackValue::Unknown { origins }
                }
                _ => StackValue::with_type_and_origins(result_type, origins),
            }
        })
        .collect()
}

fn merge_select_values(
    lhs: StackValue,
    rhs: StackValue,
    instruction_index: usize,
) -> ValidationResult<StackValue> {
    match (lhs, rhs) {
        (
            StackValue::Known {
                ty: lhs_type,
                origins: lhs_origins,
            },
            StackValue::Known {
                ty: rhs_type,
                origins: rhs_origins,
            },
        ) => {
            let origins = lhs_origins.merge(&rhs_origins);
            if lhs_type == rhs_type {
                Ok(StackValue::with_type_and_origins(lhs_type, origins))
            } else {
                invalid_at_with_origins(
                    instruction_index,
                    ValidationErrorKind::SelectTypeMismatch,
                    origins.as_slice(),
                )
            }
        }
        (
            StackValue::Known { ty, origins },
            StackValue::Unknown {
                origins: unknown_origins,
            },
        )
        | (
            StackValue::Unknown {
                origins: unknown_origins,
            },
            StackValue::Known { ty, origins },
        ) => Ok(StackValue::with_type_and_origins(
            ty,
            origins.merge(&unknown_origins),
        )),
        (StackValue::Unknown { origins: lhs }, StackValue::Unknown { origins: rhs }) => {
            Ok(StackValue::Unknown {
                origins: lhs.merge(&rhs),
            })
        }
    }
}

fn stack_value_matches_expected(value: &StackValue, expected: &ValueType) -> bool {
    match value {
        StackValue::Unknown { .. } => true,
        StackValue::Known { ty: actual, .. } => value_type_matches(actual, expected),
    }
}

fn block_signature(block_type: &BlockType) -> (Vec<ValueType>, Vec<ValueType>) {
    match block_type {
        BlockType::Empty => (Vec::new(), Vec::new()),
        BlockType::Result(value_type) => (Vec::new(), vec![value_type.clone()]),
        BlockType::Function(function_type) => {
            (function_type.params.clone(), function_type.results.clone())
        }
    }
}

fn resolve_local_type(
    locals: &[ValueType],
    local_idx: u32,
    instruction_index: usize,
) -> ValidationResult<&ValueType> {
    let local_idx = usize::try_from(local_idx).map_err(|_| {
        ValidationError::new(instruction_index, ValidationErrorKind::InvalidLocalIndex)
    })?;
    locals.get(local_idx).ok_or(ValidationError::new(
        instruction_index,
        ValidationErrorKind::InvalidLocalIndex,
    ))
}

fn resolve_global(
    globals: &[Global],
    global_idx: u32,
    instruction_index: usize,
) -> ValidationResult<&Global> {
    let global_idx = usize::try_from(global_idx).map_err(|_| {
        ValidationError::new(instruction_index, ValidationErrorKind::InvalidGlobalIndex)
    })?;
    globals.get(global_idx).ok_or(ValidationError::new(
        instruction_index,
        ValidationErrorKind::InvalidGlobalIndex,
    ))
}

fn resolve_function(
    funcs: &[Function],
    function_idx: u32,
    instruction_index: usize,
) -> ValidationResult<&Function> {
    let function_idx = usize::try_from(function_idx).map_err(|_| {
        ValidationError::new(instruction_index, ValidationErrorKind::InvalidFunctionIndex)
    })?;
    funcs.get(function_idx).ok_or(ValidationError::new(
        instruction_index,
        ValidationErrorKind::InvalidFunctionIndex,
    ))
}

fn value_type_matches(actual: &ValueType, expected: &ValueType) -> bool {
    match (actual, expected) {
        (ValueType::Ref(actual_ref), ValueType::Ref(expected_ref)) => {
            if expected_ref.nullable || !actual_ref.nullable {
                heap_type_matches(&actual_ref.heap_type, &expected_ref.heap_type)
            } else {
                false
            }
        }
        _ => actual == expected,
    }
}

fn heap_type_matches(actual: &HeapType, expected: &HeapType) -> bool {
    if actual == expected {
        return true;
    }

    match expected {
        HeapType::Any => true,
        HeapType::Eq => matches!(
            actual,
            HeapType::Eq
                | HeapType::I31
                | HeapType::AnyStruct
                | HeapType::Struct(_)
                | HeapType::AnyArray
                | HeapType::Array(_)
        ),
        HeapType::AnyStruct => matches!(actual, HeapType::AnyStruct | HeapType::Struct(_)),
        HeapType::AnyArray => matches!(actual, HeapType::AnyArray | HeapType::Array(_)),
        HeapType::AnyFunc => matches!(actual, HeapType::AnyFunc | HeapType::Func(_)),
        HeapType::Func(expected_func) => {
            matches!(actual, HeapType::Func(actual_func) if actual_func == expected_func)
        }
        HeapType::Struct(expected_struct) => {
            matches!(actual, HeapType::Struct(actual_struct) if actual_struct == expected_struct)
        }
        HeapType::Array(expected_array) => {
            matches!(actual, HeapType::Array(actual_array) if actual_array == expected_array)
        }
        HeapType::NoFunc
        | HeapType::NoExtern
        | HeapType::None
        | HeapType::Extern
        | HeapType::I31 => false,
    }
}

fn field_input_type(field: &FieldType) -> ValueType {
    match &field.storage {
        StorageType::Value(value_type) => value_type.clone(),
        StorageType::Packed(_) => ValueType::I32,
    }
}

fn field_output_type(field: &FieldType) -> ValueType {
    match &field.storage {
        StorageType::Value(value_type) => value_type.clone(),
        StorageType::Packed(_) => ValueType::I32,
    }
}

fn array_element_input_type(array_type: &ArrayType) -> ValueType {
    field_input_type(&array_type.element)
}

fn array_element_output_type(array_type: &ArrayType) -> ValueType {
    field_output_type(&array_type.element)
}

fn struct_field(
    struct_type: &StructType,
    field_idx: u32,
    instruction_index: usize,
) -> ValidationResult<&FieldType> {
    let field_idx = usize::try_from(field_idx).map_err(|_| {
        ValidationError::new(
            instruction_index,
            ValidationErrorKind::InvalidStructFieldIndex,
        )
    })?;
    struct_type
        .fields
        .get(field_idx)
        .ok_or(ValidationError::new(
            instruction_index,
            ValidationErrorKind::InvalidStructFieldIndex,
        ))
}

fn is_array_reference_type(value_type: &ValueType) -> bool {
    match value_type {
        ValueType::Ref(ref_type) => is_array_heap_type(&ref_type.heap_type),
        _ => false,
    }
}

const fn is_array_heap_type(heap_type: &HeapType) -> bool {
    matches!(heap_type, HeapType::AnyArray | HeapType::Array(_))
}

const fn ref_value(heap_type: HeapType, nullable: bool) -> ValueType {
    ValueType::Ref(RefType {
        nullable,
        heap_type,
    })
}

const fn any_ref(nullable: bool) -> ValueType {
    ref_value(HeapType::Any, nullable)
}

const fn eq_ref(nullable: bool) -> ValueType {
    ref_value(HeapType::Eq, nullable)
}

const fn extern_ref(nullable: bool) -> ValueType {
    ref_value(HeapType::Extern, nullable)
}

const fn any_func_ref(nullable: bool) -> ValueType {
    ref_value(HeapType::AnyFunc, nullable)
}

const fn i31_ref(nullable: bool) -> ValueType {
    ref_value(HeapType::I31, nullable)
}

fn struct_ref(struct_type: &StructType, nullable: bool) -> ValueType {
    ref_value(HeapType::Struct(Box::new(struct_type.clone())), nullable)
}

fn array_ref(array_type: &ArrayType, nullable: bool) -> ValueType {
    ref_value(HeapType::Array(Box::new(array_type.clone())), nullable)
}
