use super::{
    BlockType, Function, FunctionType, Global, HeapType, Instruction, ValueType,
    validate::{ValidationError, ValidationErrorKind, arity},
};

fn mk_global(value_type: ValueType, mutable: bool) -> Global {
    value_type.global(mutable).def()
}

fn mk_function(params: &[ValueType], results: &[ValueType]) -> Function {
    FunctionType {
        params: params.to_vec(),
        results: results.to_vec(),
    }
    .def()
}

fn run(
    instructions: &[Instruction],
    locals: &[ValueType],
    globals: &[Global],
    funcs: &[Function],
) -> Result<Vec<ValueType>, ValidationError> {
    arity(instructions, &[], locals, globals, funcs)
}

fn assert_validation_error(
    result: Result<Vec<ValueType>, ValidationError>,
    instruction_index: usize,
    kind: ValidationErrorKind,
    value_origins: Option<Vec<usize>>,
) {
    let error = result.expect_err("expected validation error");
    assert_eq!(error.instruction_index, instruction_index);
    assert_eq!(error.kind, kind);
    assert_eq!(error.value_origins, value_origins);
}

#[test]
fn validates_call_local_and_global_usage() {
    let locals = vec![ValueType::I32];
    let globals = vec![mk_global(ValueType::I32, true)];
    let funcs = vec![mk_function(&[ValueType::I32], &[ValueType::I64])];
    let instructions = vec![
        Instruction::LocalGet(0),
        Instruction::Call(0),
        Instruction::Drop,
        Instruction::I32Const(8),
        Instruction::GlobalSet(0),
        Instruction::GlobalGet(0),
    ];

    let out = run(&instructions, &locals, &globals, &funcs).expect("validation should succeed");
    assert_eq!(out, vec![ValueType::I32]);
}

#[test]
fn reports_invalid_local_index() {
    let instructions = vec![Instruction::LocalGet(0)];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::InvalidLocalIndex,
        None,
    );
}

#[test]
fn reports_invalid_global_index() {
    let instructions = vec![Instruction::GlobalGet(0)];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::InvalidGlobalIndex,
        None,
    );
}

#[test]
fn reports_invalid_function_index() {
    let instructions = vec![Instruction::I32Const(1), Instruction::Call(0)];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::InvalidFunctionIndex,
        None,
    );
}

#[test]
fn reports_immutable_global_set() {
    let globals = vec![mk_global(ValueType::I32, false)];
    let instructions = vec![Instruction::I32Const(1), Instruction::GlobalSet(0)];
    assert_validation_error(
        run(&instructions, &[], &globals, &[]),
        1,
        ValidationErrorKind::ImmutableGlobal,
        None,
    );
}

#[test]
fn reports_type_mismatch_with_value_origin() {
    let instructions = vec![Instruction::I32Const(1), Instruction::I64Eqz];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::TypeMismatch,
        Some(vec![0]),
    );
}

#[test]
fn reports_expected_reference_with_value_origin() {
    let instructions = vec![Instruction::I32Const(1), Instruction::RefIsNull];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::ExpectedReference,
        Some(vec![0]),
    );
}

#[test]
fn reports_select_type_mismatch_with_both_origins() {
    let instructions = vec![
        Instruction::I32Const(1),
        Instruction::I64Const(2),
        Instruction::I32Const(0),
        Instruction::Select,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        3,
        ValidationErrorKind::SelectTypeMismatch,
        Some(vec![0, 1]),
    );
}

#[test]
fn preserves_value_origins_through_br_if() {
    let instructions = vec![
        Instruction::Block(BlockType::Result(ValueType::I32)),
        Instruction::I32Const(7),
        Instruction::I32Const(1),
        Instruction::BrIf(0),
        Instruction::End,
        Instruction::I64Eqz,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        5,
        ValidationErrorKind::TypeMismatch,
        Some(vec![1]),
    );
}

#[test]
fn merges_if_else_origins_for_downstream_errors() {
    let instructions = vec![
        Instruction::I32Const(1),
        Instruction::If(BlockType::Result(ValueType::I32)),
        Instruction::I32Const(10),
        Instruction::Else,
        Instruction::I32Const(20),
        Instruction::End,
        Instruction::I64Eqz,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        6,
        ValidationErrorKind::TypeMismatch,
        Some(vec![2, 4]),
    );
}

#[test]
fn reports_branch_type_mismatch_with_branch_value_origin() {
    let instructions = vec![
        Instruction::Block(BlockType::Result(ValueType::I32)),
        Instruction::RefNull(HeapType::Extern),
        Instruction::BrOnNonNull(0),
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        2,
        ValidationErrorKind::BrOnNonNullTypeMismatch,
        Some(vec![1]),
    );
}

#[test]
fn reports_array_len_non_array_reference_with_origin() {
    let instructions = vec![
        Instruction::RefNull(HeapType::Extern),
        Instruction::ArrayLen,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::ArrayLenRequiresArrayReference,
        Some(vec![0]),
    );
}

#[test]
fn reports_invalid_label_depth() {
    let instructions = vec![Instruction::Br(0)];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::InvalidLabelDepth,
        None,
    );
}

#[test]
fn reports_branch_table_type_mismatch() {
    let instructions = vec![
        Instruction::Block(BlockType::Result(ValueType::I32)),
        Instruction::Block(BlockType::Empty),
        Instruction::BrTable(vec![1], 0),
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        2,
        ValidationErrorKind::BranchTableTypeMismatch,
        None,
    );
}

#[test]
fn reports_invalid_else_placement() {
    let instructions = vec![Instruction::Else];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::InvalidElsePlacement,
        None,
    );
}

#[test]
fn reports_invalid_end_placement() {
    let instructions = vec![Instruction::End];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::InvalidEndPlacement,
        None,
    );
}

#[test]
fn reports_if_without_else_type_mismatch() {
    let instructions = vec![
        Instruction::I32Const(1),
        Instruction::If(BlockType::Result(ValueType::I32)),
        Instruction::I32Const(2),
        Instruction::End,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        3,
        ValidationErrorKind::IfWithoutElseTypeMismatch,
        None,
    );
}

#[test]
fn reports_branch_stack_height_mismatch() {
    let instructions = vec![
        Instruction::Block(BlockType::Result(ValueType::I32)),
        Instruction::I32Const(1),
        Instruction::I32Const(2),
        Instruction::End,
    ];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        3,
        ValidationErrorKind::BranchStackHeightMismatch,
        None,
    );
}

#[test]
fn reports_unclosed_control_frames() {
    let instructions = vec![Instruction::Block(BlockType::Empty)];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::UnclosedControlFrames,
        None,
    );
}

#[test]
fn reports_top_level_unreachable() {
    let instructions = vec![Instruction::Unreachable];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        1,
        ValidationErrorKind::TopLevelUnreachable,
        None,
    );
}

#[test]
fn reports_unsupported_return_instruction() {
    let instructions = vec![Instruction::Return];
    assert_validation_error(
        run(&instructions, &[], &[], &[]),
        0,
        ValidationErrorKind::UnsupportedInstruction,
        None,
    );
}
