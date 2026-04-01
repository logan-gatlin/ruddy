use std::collections::hash_map::Entry;

use crate::parser::ast;
use crate::reporting::TextRange;
use crate::wasm;
use crate::wasm::types::{
    BlockType, FunctionType, GlobalType, HeapType, Mutability, RefType, StorageType, ValueType,
};

use super::*;

impl<'db> ModuleLowerer<'db> {
    pub(super) fn collect_wasm_module_scope(
        &mut self,
        statements: &[ast::Statement],
    ) -> WasmModuleScope {
        let mut scope = WasmModuleScope::default();
        let mut next_function = 0u32;
        let mut next_global = 0u32;

        for statement in statements {
            let ast::Statement::Wasm { declarations, .. } = statement else {
                continue;
            };

            for declaration in declarations {
                let Some(items) = self.sexpr_list_items(declaration) else {
                    continue;
                };
                let Some(head) = items.first() else {
                    continue;
                };
                let Some(head_text) = self.sexpr_atom_text(head) else {
                    continue;
                };

                if head_text == "func" {
                    let index = next_function;
                    next_function = next_function.saturating_add(1);
                    scope
                        .function_decl_indices
                        .insert(declaration.range(), index);

                    let name = self.parse_optional_wasm_binding_name(items, "function");
                    if let Some(name) = name {
                        match scope.functions.entry(name) {
                            Entry::Occupied(entry) => {
                                self.error(
                                    declaration.range(),
                                    format!("duplicate wasm function binding `{}`", entry.key()),
                                );
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(index);
                            }
                        }
                    }
                } else if head_text == "global" {
                    let index = next_global;
                    next_global = next_global.saturating_add(1);
                    scope.global_decl_indices.insert(declaration.range(), index);

                    let name = self.parse_optional_wasm_binding_name(items, "global");
                    if let Some(name) = name {
                        match scope.globals.entry(name) {
                            Entry::Occupied(entry) => {
                                self.error(
                                    declaration.range(),
                                    format!("duplicate wasm global binding `{}`", entry.key()),
                                );
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(index);
                            }
                        }
                    }
                }
            }
        }

        scope
    }

    pub(super) fn lower_wasm_statement(
        &mut self,
        declarations: &[ast::SExpr],
        range: TextRange,
    ) -> ir::Statement {
        let mut lowered = Vec::with_capacity(declarations.len());

        for declaration in declarations {
            let Some(items) = self.sexpr_list_items(declaration) else {
                self.error(
                    declaration.range(),
                    "expected wasm top-level declaration S-expression list",
                );
                lowered.push(ir::WasmTopLevelDeclaration::Error(ir::ErrorNode {
                    range: declaration.range(),
                }));
                continue;
            };

            let Some(head) = items.first() else {
                self.error(declaration.range(), "empty wasm top-level declaration");
                lowered.push(ir::WasmTopLevelDeclaration::Error(ir::ErrorNode {
                    range: declaration.range(),
                }));
                continue;
            };

            let Some(head_text) = self.sexpr_atom_text(head) else {
                self.error(declaration.range(), "expected wasm declaration kind atom");
                lowered.push(ir::WasmTopLevelDeclaration::Error(ir::ErrorNode {
                    range: declaration.range(),
                }));
                continue;
            };

            match head_text.as_str() {
                "func" => lowered.push(self.lower_wasm_function_declaration(declaration, items)),
                "global" => lowered.push(self.lower_wasm_global_declaration(declaration, items)),
                _ => {
                    self.error(
                        declaration.range(),
                        format!("unsupported top-level wasm declaration `{head_text}`"),
                    );
                    lowered.push(ir::WasmTopLevelDeclaration::Error(ir::ErrorNode {
                        range: declaration.range(),
                    }));
                }
            }
        }

        ir::Statement::Wasm {
            declarations: lowered,
            range,
        }
    }

    fn lower_wasm_function_declaration(
        &mut self,
        declaration: &ast::SExpr,
        items: &[ast::SExpr],
    ) -> ir::WasmTopLevelDeclaration {
        let function_index = self
            .wasm_scope
            .function_decl_indices
            .get(&declaration.range())
            .copied()
            .unwrap_or_default();

        let (binding_name, sections_start) = self.parse_optional_wasm_binding(items, "function");
        let binding = binding_name.map(|(name, range)| ir::WasmBinding {
            name,
            index: function_index,
            range,
        });

        let mut cursor = WasmStreamCursor::new(&items[sections_start..]);
        let mut local_scope = WasmLocalScope::default();
        let mut params = Vec::new();
        let mut locals = Vec::new();
        let mut function_type = FunctionType::default();

        while let Some(section) = cursor.peek() {
            let Some(section_items) = self.sexpr_list_items(section) else {
                break;
            };
            let Some(section_head) = section_items.first() else {
                break;
            };
            let Some(section_head_text) = self.sexpr_atom_text(section_head) else {
                break;
            };

            if section_head_text == "param" {
                let _ = cursor.next();
                let mut section_pos = 1;
                while section_pos + 1 < section_items.len() {
                    let name_node = &section_items[section_pos];
                    let type_node = &section_items[section_pos + 1];
                    section_pos += 2;

                    let name = self
                        .parse_wasm_binding_name(name_node, "local parameter")
                        .unwrap_or_else(|| format!("$invalid_param_{}", local_scope.next_index));
                    if local_scope.lookup(&name).is_some() {
                        self.error(
                            name_node.range(),
                            format!("duplicate wasm local binding `{name}`"),
                        );
                    }

                    let Some(ty) = self.parse_wasm_value_type(type_node, "function parameter")
                    else {
                        continue;
                    };

                    function_type.params.push(ty.clone());
                    let binding = local_scope.bind(name, name_node.range());
                    local_scope.insert_name_if_missing(binding.name.clone(), binding.index);
                    params.push(ir::WasmTypedBinding { binding, ty });
                }

                if section_pos != section_items.len() {
                    self.error(
                        section.range(),
                        "`param` section expects pairs of `$name <type>`",
                    );
                }
            } else if section_head_text == "result" {
                let _ = cursor.next();
                for type_node in &section_items[1..] {
                    if let Some(ty) = self.parse_wasm_value_type(type_node, "function result") {
                        function_type.results.push(ty);
                    }
                }
            } else if section_head_text == "local" {
                let _ = cursor.next();
                let mut section_pos = 1;
                while section_pos + 1 < section_items.len() {
                    let name_node = &section_items[section_pos];
                    let type_node = &section_items[section_pos + 1];
                    section_pos += 2;

                    let name = self
                        .parse_wasm_binding_name(name_node, "local")
                        .unwrap_or_else(|| format!("$invalid_local_{}", local_scope.next_index));
                    if local_scope.lookup(&name).is_some() {
                        self.error(
                            name_node.range(),
                            format!("duplicate wasm local binding `{name}`"),
                        );
                    }

                    let Some(ty) = self.parse_wasm_value_type(type_node, "local declaration")
                    else {
                        continue;
                    };

                    let binding = local_scope.bind(name, name_node.range());
                    local_scope.insert_name_if_missing(binding.name.clone(), binding.index);
                    locals.push(ir::WasmTypedBinding {
                        binding,
                        ty: ty.clone(),
                    });
                }

                if section_pos != section_items.len() {
                    self.error(
                        section.range(),
                        "`local` section expects pairs of `$name <type>`",
                    );
                }
            } else {
                break;
            }
        }

        let body_items = &cursor.items[cursor.pos..];
        let instruction_nodes =
            self.parse_wasm_instruction_stream(body_items, &mut local_scope, true, false);
        let function = wasm::Function {
            type_: function_type,
            locals: locals.iter().map(|local| local.ty.clone()).collect(),
            instructions: instruction_nodes
                .iter()
                .map(|node| node.instruction.clone())
                .collect(),
        };

        ir::WasmTopLevelDeclaration::Function(ir::WasmFunctionDecl {
            binding,
            index: function_index,
            function,
            params,
            locals,
            instructions: instruction_nodes,
            range: declaration.range(),
        })
    }

    fn lower_wasm_global_declaration(
        &mut self,
        declaration: &ast::SExpr,
        items: &[ast::SExpr],
    ) -> ir::WasmTopLevelDeclaration {
        let global_index = self
            .wasm_scope
            .global_decl_indices
            .get(&declaration.range())
            .copied()
            .unwrap_or_default();

        let (binding_name, mut pos) = self.parse_optional_wasm_binding(items, "global");
        let binding = binding_name.map(|(name, range)| ir::WasmBinding {
            name,
            index: global_index,
            range,
        });

        let mut mutable = Mutability::Immutable;
        let mut value_type = ValueType::I32;
        if let Some(type_node) = items.get(pos) {
            if let Some((ty, is_mutable)) = self.parse_wasm_global_type(type_node) {
                value_type = ty;
                mutable = if is_mutable {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                };
            }
        } else {
            self.error(
                declaration.range(),
                "`global` declaration requires a value type",
            );
        }
        pos += 1;

        let mut instruction_nodes = Vec::new();
        if let Some(init_node) = items.get(pos) {
            if let Some(init_items) = self.sexpr_list_items(init_node) {
                let mut local_scope = WasmLocalScope::default();
                instruction_nodes =
                    self.parse_wasm_instruction_stream(init_items, &mut local_scope, false, false);
            } else {
                self.error(
                    init_node.range(),
                    "global initializer must be a parenthesized linear instruction stream",
                );
                instruction_nodes.push(ir::WasmInstructionNode {
                    instruction: wasm::Instruction::Unreachable,
                    range: init_node.range(),
                    symbols: Vec::new(),
                });
            }
        }
        pos += 1;

        if items.len() > pos {
            self.error(
                declaration.range(),
                "unexpected extra items in `global` declaration",
            );
        }

        let global = wasm::Global {
            type_: GlobalType {
                type_: value_type,
                mutable,
            },
            init: instruction_nodes
                .iter()
                .map(|node| node.instruction.clone())
                .collect(),
        };

        ir::WasmTopLevelDeclaration::Global(ir::WasmGlobalDecl {
            binding,
            index: global_index,
            global,
            instructions: instruction_nodes,
            range: declaration.range(),
        })
    }

    pub(super) fn lower_inline_wasm_expression(
        &mut self,
        body: &Option<ast::SExpr>,
        range: TextRange,
    ) -> (Vec<ir::WasmTypedBinding>, Vec<ir::WasmInstructionNode>) {
        let Some(body) = body else {
            self.error(range, "inline wasm body is missing");
            return (
                Vec::new(),
                vec![ir::WasmInstructionNode {
                    instruction: wasm::Instruction::Unreachable,
                    range,
                    symbols: Vec::new(),
                }],
            );
        };

        let Some(items) = self.sexpr_list_items(body) else {
            self.error(
                body.range(),
                "inline wasm body must be a parenthesized linear instruction stream",
            );
            return (
                Vec::new(),
                vec![ir::WasmInstructionNode {
                    instruction: wasm::Instruction::Unreachable,
                    range: body.range(),
                    symbols: Vec::new(),
                }],
            );
        };

        let mut locals = Vec::new();
        let mut local_scope = WasmLocalScope::default();
        let instruction_nodes = self.parse_wasm_instruction_stream_with_inline_locals(
            items,
            &mut local_scope,
            &mut locals,
        );
        (locals, instruction_nodes)
    }

    fn parse_wasm_instruction_stream_with_inline_locals(
        &mut self,
        items: &[ast::SExpr],
        local_scope: &mut WasmLocalScope,
        locals: &mut Vec<ir::WasmTypedBinding>,
    ) -> Vec<ir::WasmInstructionNode> {
        let mut cursor = WasmStreamCursor::new(items);
        let mut instructions = Vec::new();

        while !cursor.is_eof() {
            if self.try_parse_inline_local_decl(&mut cursor, local_scope, locals) {
                continue;
            }

            instructions.push(self.parse_wasm_instruction(&mut cursor, local_scope));
        }

        instructions
    }

    fn parse_wasm_instruction_stream(
        &mut self,
        items: &[ast::SExpr],
        local_scope: &mut WasmLocalScope,
        _allow_locals: bool,
        _order_sensitive_locals: bool,
    ) -> Vec<ir::WasmInstructionNode> {
        let mut cursor = WasmStreamCursor::new(items);
        let mut instructions = Vec::new();

        while !cursor.is_eof() {
            instructions.push(self.parse_wasm_instruction(&mut cursor, local_scope));
        }

        instructions
    }

    fn try_parse_inline_local_decl(
        &mut self,
        cursor: &mut WasmStreamCursor<'_>,
        local_scope: &mut WasmLocalScope,
        locals: &mut Vec<ir::WasmTypedBinding>,
    ) -> bool {
        let Some(item) = cursor.peek() else {
            return false;
        };
        let Some(items) = self.sexpr_list_items(item) else {
            return false;
        };
        let Some(head) = items.first() else {
            return false;
        };
        let Some(head_text) = self.sexpr_atom_text(head) else {
            return false;
        };
        if head_text != "local" {
            return false;
        }

        let _ = cursor.next();
        let mut pos = 1;
        while pos + 1 < items.len() {
            let name_node = &items[pos];
            let type_node = &items[pos + 1];
            pos += 2;

            let name = self
                .parse_wasm_binding_name(name_node, "local")
                .unwrap_or_else(|| format!("$invalid_local_{}", local_scope.next_index));
            if local_scope.lookup(&name).is_some() {
                self.error(
                    name_node.range(),
                    format!("duplicate wasm local binding `{name}`"),
                );
            }

            let Some(ty) = self.parse_wasm_value_type(type_node, "local declaration") else {
                continue;
            };
            let binding = local_scope.bind(name, name_node.range());
            local_scope.insert_name_if_missing(binding.name.clone(), binding.index);
            locals.push(ir::WasmTypedBinding {
                binding,
                ty: ty.clone(),
            });
        }

        if pos != items.len() {
            self.error(
                item.range(),
                "`local` declaration expects pairs of `$name <type>`",
            );
        }

        true
    }

    fn parse_wasm_instruction(
        &mut self,
        cursor: &mut WasmStreamCursor<'_>,
        local_scope: &mut WasmLocalScope,
    ) -> ir::WasmInstructionNode {
        let Some(opcode_node) = cursor.next() else {
            return ir::WasmInstructionNode {
                instruction: wasm::Instruction::Unreachable,
                range: TextRange::generated(),
                symbols: Vec::new(),
            };
        };

        let opcode_range = opcode_node.range();
        let Some(opcode_text) = self.sexpr_atom_text(opcode_node) else {
            self.error(
                opcode_range,
                "wasm instruction opcode must be an atom in linear form",
            );
            return self.unreachable_wasm_instruction(opcode_range);
        };

        let mut range = opcode_range;
        let mut symbols = Vec::new();

        let instruction = match opcode_text.as_str() {
            "block" => {
                let block_type = self.parse_optional_wasm_block_type(cursor);
                wasm::Instruction::Block(block_type)
            }
            "loop" => {
                let block_type = self.parse_optional_wasm_block_type(cursor);
                wasm::Instruction::Loop(block_type)
            }
            "if" => {
                let block_type = self.parse_optional_wasm_block_type(cursor);
                wasm::Instruction::If(block_type)
            }
            "br" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`br` expects a label depth immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(label) = self.parse_wasm_u32_immediate(item, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::Br(label)
            }
            "br_if" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`br_if` expects a label depth immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(label) = self.parse_wasm_u32_immediate(item, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrIf(label)
            }
            "br_table" => {
                let Some(labels_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_table` expects a parenthesized label list",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(default_node) = cursor.next() else {
                    self.error(opcode_range, "`br_table` expects a default label depth");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, default_node.range());
                let Some(label_items) = self.sexpr_list_items(labels_node) else {
                    self.error(
                        labels_node.range(),
                        "`br_table` label list must be parenthesized",
                    );
                    return self.unreachable_wasm_instruction(range);
                };
                let mut labels = Vec::with_capacity(label_items.len());
                for label_node in label_items {
                    let Some(label) = self.parse_wasm_u32_immediate(label_node, "label depth")
                    else {
                        return self.unreachable_wasm_instruction(range);
                    };
                    labels.push(label);
                }
                let Some(default_label) =
                    self.parse_wasm_u32_immediate(default_node, "default label depth")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrTable(labels, default_label)
            }
            "call" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`call` expects a function index or `$name`");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Function,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::Call(resolved.index)
            }
            "call_indirect" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`call_indirect` expects a function type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(table_idx_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`call_indirect` expects a table index immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, table_idx_node.range());
                let Some(function_type) = self.parse_wasm_function_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(table_index) =
                    self.parse_wasm_u32_immediate(table_idx_node, "table index")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::CallIndirect(function_type, table_index)
            }
            "local.get" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`local.get` expects a local index or `$name`");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Local,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::LocalGet(resolved.index)
            }
            "local.set" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`local.set` expects a local index or `$name`");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Local,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::LocalSet(resolved.index)
            }
            "local.tee" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`local.tee` expects a local index or `$name`");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Local,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::LocalTee(resolved.index)
            }
            "global.get" => {
                let Some(item) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`global.get` expects a global index or `$name`",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Global,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::GlobalGet(resolved.index)
            }
            "global.set" => {
                let Some(item) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`global.set` expects a global index or `$name`",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Global,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::GlobalSet(resolved.index)
            }
            "ref.func" => {
                let Some(item) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`ref.func` expects a function index or `$name`",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(resolved) = self.resolve_wasm_index_operand(
                    item,
                    ir::WasmSymbolNamespace::Function,
                    local_scope,
                ) else {
                    return self.unreachable_wasm_instruction(range);
                };
                if let Some(symbol) = resolved.symbol {
                    symbols.push(symbol);
                }
                wasm::Instruction::RefFunc(resolved.index)
            }
            "i32.const" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`i32.const` expects an integer immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(value) = self.parse_wasm_i32_immediate(item) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::I32Const(value)
            }
            "i64.const" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`i64.const` expects an integer immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(value) = self.parse_wasm_i64_immediate(item) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::I64Const(value)
            }
            "f32.const" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`f32.const` expects a real immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(value) = self.parse_wasm_f32_immediate(item) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::F32Const(value)
            }
            "f64.const" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`f64.const` expects a real immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(value) = self.parse_wasm_f64_immediate(item) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::F64Const(value)
            }
            "ref.null" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`ref.null` expects a heap type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(heap_type) = self.parse_wasm_heap_type(item, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::RefNull(heap_type)
            }
            "ref.test" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`ref.test` expects a ref type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(ref_type) = self.parse_wasm_ref_type(item, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::RefTest(ref_type)
            }
            "ref.cast" => {
                let Some(item) = cursor.next() else {
                    self.error(opcode_range, "`ref.cast` expects a ref type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, item.range());
                let Some(ref_type) = self.parse_wasm_ref_type(item, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::RefCast(ref_type)
            }
            "br_on_null" => {
                let Some(label_node) = cursor.next() else {
                    self.error(opcode_range, "`br_on_null` expects a label depth immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, label_node.range());
                let Some(label) = self.parse_wasm_u32_immediate(label_node, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrOnNull(label)
            }
            "br_on_non_null" => {
                let Some(label_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_non_null` expects a label depth immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, label_node.range());
                let Some(label) = self.parse_wasm_u32_immediate(label_node, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrOnNonNull(label)
            }
            "br_on_cast" => {
                let Some(label_node) = cursor.next() else {
                    self.error(opcode_range, "`br_on_cast` expects a label depth immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(from_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_cast` expects a from ref type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(to_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_cast` expects a target ref type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, to_node.range());
                let Some(label) = self.parse_wasm_u32_immediate(label_node, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(from_ref) = self.parse_wasm_ref_type(from_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(to_ref) = self.parse_wasm_ref_type(to_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrOnCast(label, from_ref, to_ref)
            }
            "br_on_cast_fail" => {
                let Some(label_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_cast_fail` expects a label depth immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(from_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_cast_fail` expects a from ref type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(to_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`br_on_cast_fail` expects a target ref type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, to_node.range());
                let Some(label) = self.parse_wasm_u32_immediate(label_node, "label depth") else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(from_ref) = self.parse_wasm_ref_type(from_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(to_ref) = self.parse_wasm_ref_type(to_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::BrOnCastFail(label, from_ref, to_ref)
            }
            "struct.new" => {
                let Some(type_node) = cursor.next() else {
                    self.error(opcode_range, "`struct.new` expects a struct type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(struct_type) = self.parse_wasm_struct_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::StructNew(struct_type)
            }
            "struct.new_default" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`struct.new_default` expects a struct type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(struct_type) = self.parse_wasm_struct_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::StructNewDefault(struct_type)
            }
            "struct.get" | "struct.get_s" | "struct.get_u" | "struct.set" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects a struct type immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(field_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects a field index immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, field_node.range());
                let Some(struct_type) = self.parse_wasm_struct_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(field_idx) = self.parse_wasm_u32_immediate(field_node, "field index")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                match opcode_text.as_str() {
                    "struct.get" => wasm::Instruction::StructGet(struct_type, field_idx),
                    "struct.get_s" => wasm::Instruction::StructGetS(struct_type, field_idx),
                    "struct.get_u" => wasm::Instruction::StructGetU(struct_type, field_idx),
                    _ => wasm::Instruction::StructSet(struct_type, field_idx),
                }
            }
            "array.new" => {
                let Some(type_node) = cursor.next() else {
                    self.error(opcode_range, "`array.new` expects an array type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayNew(array_type)
            }
            "array.new_default" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.new_default` expects an array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayNewDefault(array_type)
            }
            "array.new_fixed" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.new_fixed` expects an array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(length_node) = cursor.next() else {
                    self.error(opcode_range, "`array.new_fixed` expects a length immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, length_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(length) = self.parse_wasm_u32_immediate(length_node, "array length")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayNewFixed(array_type, length)
            }
            "array.new_data" | "array.new_elem" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects an array type immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(index_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects a segment index immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, index_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(segment) = self.parse_wasm_u32_immediate(index_node, "segment index")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                if opcode_text == "array.new_data" {
                    wasm::Instruction::ArrayNewData(array_type, segment)
                } else {
                    wasm::Instruction::ArrayNewElem(array_type, segment)
                }
            }
            "array.get" => {
                let Some(type_node) = cursor.next() else {
                    self.error(opcode_range, "`array.get` expects an array type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayGet(array_type)
            }
            "array.get_s" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.get_s` expects an array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayGetS(array_type)
            }
            "array.get_u" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.get_u` expects an array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayGetU(array_type)
            }
            "array.set" => {
                let Some(type_node) = cursor.next() else {
                    self.error(opcode_range, "`array.set` expects an array type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArraySet(array_type)
            }
            "array.fill" => {
                let Some(type_node) = cursor.next() else {
                    self.error(opcode_range, "`array.fill` expects an array type immediate");
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, type_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayFill(array_type)
            }
            "array.copy" => {
                let Some(dst_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.copy` expects destination array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(src_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        "`array.copy` expects source array type immediate",
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, src_node.range());
                let Some(dst_type) = self.parse_wasm_array_type(dst_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(src_type) = self.parse_wasm_array_type(src_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                wasm::Instruction::ArrayCopy(dst_type, src_type)
            }
            "array.init_data" | "array.init_elem" => {
                let Some(type_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects an array type immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                let Some(index_node) = cursor.next() else {
                    self.error(
                        opcode_range,
                        format!("`{}` expects a segment index immediate", opcode_text),
                    );
                    return self.unreachable_wasm_instruction(opcode_range);
                };
                range = merge_text_ranges(range, index_node.range());
                let Some(array_type) = self.parse_wasm_array_type(type_node, true) else {
                    return self.unreachable_wasm_instruction(range);
                };
                let Some(segment) = self.parse_wasm_u32_immediate(index_node, "segment index")
                else {
                    return self.unreachable_wasm_instruction(range);
                };
                if opcode_text == "array.init_data" {
                    wasm::Instruction::ArrayInitData(array_type, segment)
                } else {
                    wasm::Instruction::ArrayInitElem(array_type, segment)
                }
            }
            _ => {
                if let Some(instruction) = parse_wasm_simple_instruction(&opcode_text) {
                    instruction
                } else {
                    self.error(
                        opcode_range,
                        format!("unsupported wasm instruction `{opcode_text}` in lowering"),
                    );
                    wasm::Instruction::Unreachable
                }
            }
        };

        ir::WasmInstructionNode {
            instruction,
            range,
            symbols,
        }
    }

    fn parse_optional_wasm_block_type(&mut self, cursor: &mut WasmStreamCursor<'_>) -> BlockType {
        let Some(next) = cursor.peek() else {
            return BlockType::Empty;
        };

        if self
            .sexpr_atom_text(next)
            .is_some_and(|text| is_wasm_opcode_name(&text))
        {
            return BlockType::Empty;
        }

        let mark = cursor.mark();
        let Some(item) = cursor.next() else {
            return BlockType::Empty;
        };
        if let Some(block_type) = self.parse_wasm_block_type(item, false) {
            block_type
        } else {
            cursor.reset(mark);
            BlockType::Empty
        }
    }

    fn resolve_wasm_index_operand(
        &mut self,
        item: &ast::SExpr,
        namespace: ir::WasmSymbolNamespace,
        local_scope: &WasmLocalScope,
    ) -> Option<WasmIndexResolution> {
        let text = self.sexpr_atom_text(item)?;

        if text.starts_with('$') {
            let symbol = self.parse_wasm_binding_name(item, namespace_name_wasm(namespace))?;
            let resolved_index = match namespace {
                ir::WasmSymbolNamespace::Local => local_scope.lookup(&symbol),
                ir::WasmSymbolNamespace::Function => {
                    self.wasm_scope.functions.get(&symbol).copied()
                }
                ir::WasmSymbolNamespace::Global => self.wasm_scope.globals.get(&symbol).copied(),
            };

            let Some(resolved_index) = resolved_index else {
                self.error(
                    item.range(),
                    format!(
                        "unresolved wasm {} binding `{symbol}`",
                        namespace_name_wasm(namespace)
                    ),
                );
                return None;
            };

            return Some(WasmIndexResolution {
                index: resolved_index,
                symbol: Some(ir::WasmResolvedSymbol {
                    namespace,
                    symbol,
                    resolved_index,
                    range: item.range(),
                }),
            });
        }

        if parse_wasm_integer_text(&text).is_none() {
            self.error(
                item.range(),
                format!(
                    "wasm {} references must be numeric indices or bare `$ident` symbols",
                    namespace_name_wasm(namespace)
                ),
            );
            return None;
        }

        let index = self.parse_wasm_u32_immediate(item, "index")?;
        Some(WasmIndexResolution {
            index,
            symbol: None,
        })
    }

    fn parse_wasm_global_type(&mut self, node: &ast::SExpr) -> Option<(ValueType, bool)> {
        if let Some(items) = self.sexpr_list_items(node)
            && let Some(head) = items.first()
            && self.sexpr_atom_text(head).as_deref() == Some("mut")
        {
            if items.len() != 2 {
                self.error(
                    node.range(),
                    "`(mut ...)` expects exactly one type argument",
                );
                return None;
            }
            let ty = self.parse_wasm_value_type(&items[1], "global")?;
            return Some((ty, true));
        }

        let ty = self.parse_wasm_value_type(node, "global")?;
        Some((ty, false))
    }

    fn parse_wasm_value_type(&mut self, node: &ast::SExpr, context: &str) -> Option<ValueType> {
        if let Some(text) = self.sexpr_atom_text(node) {
            return match text.as_str() {
                "i32" => Some(ValueType::I32),
                "i64" => Some(ValueType::I64),
                "f32" => Some(ValueType::F32),
                "f64" => Some(ValueType::F64),
                "v128" => Some(ValueType::V128),
                "i8" | "i16" => {
                    self.error(
                        node.range(),
                        format!(
                            "`{}` is storage-only and is not a valid {context} value type",
                            text
                        ),
                    );
                    None
                }
                _ => {
                    if let Some(heap_type) = parse_wasm_heap_type_atom(&text) {
                        Some(ValueType::Ref(RefType {
                            nullable: true,
                            heap_type,
                        }))
                    } else {
                        self.error(
                            node.range(),
                            format!("unsupported wasm value type `{text}`"),
                        );
                        None
                    }
                }
            };
        }

        if let Some(items) = self.sexpr_list_items(node)
            && let Some(head) = items.first()
            && let Some(head_text) = self.sexpr_atom_text(head)
        {
            if head_text == "ref" || head_text == "ref.null" {
                if items.len() != 2 {
                    self.error(
                        node.range(),
                        format!("`{head_text}` expects exactly one heap type argument"),
                    );
                    return None;
                }
                let heap_type = self.parse_wasm_heap_type(&items[1], true)?;
                return Some(ValueType::Ref(RefType {
                    nullable: head_text == "ref.null",
                    heap_type,
                }));
            }

            if matches!(head_text.as_str(), "func" | "struct" | "array") {
                let heap_type = self.parse_wasm_heap_type(node, true)?;
                return Some(ValueType::Ref(RefType {
                    nullable: true,
                    heap_type,
                }));
            }
        }

        self.error(node.range(), "invalid wasm value type syntax");
        None
    }

    fn parse_wasm_storage_type(
        &mut self,
        node: &ast::SExpr,
        emit_diag: bool,
    ) -> Option<StorageType> {
        if let Some(text) = self.sexpr_atom_text(node) {
            return match text.as_str() {
                "i8" => Some(StorageType::Packed(wasm::PackedType::I8)),
                "i16" => Some(StorageType::Packed(wasm::PackedType::I16)),
                _ => self
                    .parse_wasm_value_type(node, "storage")
                    .map(StorageType::Value),
            };
        }

        if let Some(value_type) = self.parse_wasm_value_type(node, "storage") {
            return Some(StorageType::Value(value_type));
        }

        if emit_diag {
            self.error(node.range(), "invalid wasm storage type");
        }
        None
    }

    fn parse_wasm_field_type(
        &mut self,
        node: &ast::SExpr,
        emit_diag: bool,
    ) -> Option<wasm::FieldType> {
        if let Some(items) = self.sexpr_list_items(node)
            && let Some(head) = items.first()
            && self.sexpr_atom_text(head).as_deref() == Some("mut")
        {
            if items.len() != 2 {
                if emit_diag {
                    self.error(node.range(), "`(mut ...)` expects exactly one storage type");
                }
                return None;
            }
            let storage = self.parse_wasm_storage_type(&items[1], emit_diag)?;
            return Some(wasm::FieldType {
                storage,
                mutability: Mutability::Mutable,
            });
        }

        let storage = self.parse_wasm_storage_type(node, emit_diag)?;
        Some(wasm::FieldType {
            storage,
            mutability: Mutability::Immutable,
        })
    }

    fn parse_wasm_struct_type(
        &mut self,
        node: &ast::SExpr,
        emit_diag: bool,
    ) -> Option<wasm::StructType> {
        let items = self.sexpr_list_items(node)?;
        let head = items.first()?;
        if self.sexpr_atom_text(head).as_deref() != Some("struct") {
            if emit_diag {
                self.error(node.range(), "expected `(struct ...)` wasm type");
            }
            return None;
        }

        let mut fields = Vec::with_capacity(items.len().saturating_sub(1));
        for field_node in &items[1..] {
            if let Some(field) = self.parse_wasm_field_type(field_node, emit_diag) {
                fields.push(field);
            }
        }

        Some(wasm::StructType { fields })
    }

    fn parse_wasm_array_type(
        &mut self,
        node: &ast::SExpr,
        emit_diag: bool,
    ) -> Option<wasm::ArrayType> {
        let items = self.sexpr_list_items(node)?;
        let head = items.first()?;
        if self.sexpr_atom_text(head).as_deref() != Some("array") {
            if emit_diag {
                self.error(node.range(), "expected `(array ...)` wasm type");
            }
            return None;
        }
        if items.len() != 2 {
            if emit_diag {
                self.error(
                    node.range(),
                    "`(array ...)` expects exactly one element type",
                );
            }
            return None;
        }

        let element = self.parse_wasm_field_type(&items[1], emit_diag)?;
        Some(wasm::ArrayType { element })
    }

    fn parse_wasm_function_type(
        &mut self,
        node: &ast::SExpr,
        emit_diag: bool,
    ) -> Option<FunctionType> {
        let items = self.sexpr_list_items(node)?;
        let head = items.first()?;
        if self.sexpr_atom_text(head).as_deref() != Some("func") {
            if emit_diag {
                self.error(node.range(), "expected `(func ...)` wasm function type");
            }
            return None;
        }

        let mut function_type = FunctionType::default();
        for section in &items[1..] {
            let Some(section_items) = self.sexpr_list_items(section) else {
                if emit_diag {
                    self.error(
                        section.range(),
                        "function type sections must be parenthesized",
                    );
                }
                continue;
            };
            let Some(section_head) = section_items.first() else {
                continue;
            };
            let Some(section_head_text) = self.sexpr_atom_text(section_head) else {
                continue;
            };

            if section_head_text == "param" {
                for type_node in &section_items[1..] {
                    if let Some(value_type) =
                        self.parse_wasm_value_type(type_node, "function type parameter")
                    {
                        function_type.params.push(value_type);
                    }
                }
            } else if section_head_text == "result" {
                for type_node in &section_items[1..] {
                    if let Some(value_type) =
                        self.parse_wasm_value_type(type_node, "function type result")
                    {
                        function_type.results.push(value_type);
                    }
                }
            } else if emit_diag {
                self.error(
                    section.range(),
                    format!("unsupported function type section `{section_head_text}`"),
                );
            }
        }

        Some(function_type)
    }

    fn parse_wasm_heap_type(&mut self, node: &ast::SExpr, emit_diag: bool) -> Option<HeapType> {
        if let Some(text) = self.sexpr_atom_text(node) {
            if let Some(heap_type) = parse_wasm_heap_type_atom(&text) {
                return Some(heap_type);
            }
            if emit_diag {
                self.error(node.range(), format!("unsupported wasm heap type `{text}`"));
            }
            return None;
        }

        let items = self.sexpr_list_items(node)?;
        let head = items.first()?;
        let Some(head_text) = self.sexpr_atom_text(head) else {
            if emit_diag {
                self.error(node.range(), "invalid wasm heap type syntax");
            }
            return None;
        };

        match head_text.as_str() {
            "func" => self
                .parse_wasm_function_type(node, emit_diag)
                .map(|ty| HeapType::Func(Box::new(ty))),
            "struct" => self
                .parse_wasm_struct_type(node, emit_diag)
                .map(|ty| HeapType::Struct(Box::new(ty))),
            "array" => self
                .parse_wasm_array_type(node, emit_diag)
                .map(|ty| HeapType::Array(Box::new(ty))),
            _ => {
                if emit_diag {
                    self.error(
                        node.range(),
                        format!("unsupported wasm heap type `{head_text}`"),
                    );
                }
                None
            }
        }
    }

    fn parse_wasm_ref_type(&mut self, node: &ast::SExpr, emit_diag: bool) -> Option<RefType> {
        if let Some(items) = self.sexpr_list_items(node)
            && let Some(head) = items.first()
            && let Some(head_text) = self.sexpr_atom_text(head)
            && (head_text == "ref" || head_text == "ref.null")
        {
            if items.len() != 2 {
                if emit_diag {
                    self.error(
                        node.range(),
                        format!("`{head_text}` expects exactly one heap type argument"),
                    );
                }
                return None;
            }
            let heap_type = self.parse_wasm_heap_type(&items[1], emit_diag)?;
            return Some(RefType {
                nullable: head_text == "ref.null",
                heap_type,
            });
        }

        let heap_type = self.parse_wasm_heap_type(node, emit_diag)?;
        Some(RefType {
            nullable: true,
            heap_type,
        })
    }

    fn parse_wasm_block_type(&mut self, node: &ast::SExpr, emit_diag: bool) -> Option<BlockType> {
        if let Some(text) = self.sexpr_atom_text(node)
            && text == "empty"
        {
            return Some(BlockType::Empty);
        }

        if let Some(value_type) = self.parse_wasm_value_type(node, "block") {
            return Some(BlockType::Result(value_type));
        }

        if let Some(function_type) = self.parse_wasm_function_type(node, emit_diag) {
            return Some(BlockType::Function(function_type));
        }

        if emit_diag {
            self.error(node.range(), "invalid wasm block type");
        }
        None
    }

    fn parse_wasm_binding_name(&mut self, node: &ast::SExpr, kind: &str) -> Option<String> {
        let text = self.sexpr_atom_text(node)?;
        if !text.starts_with('$') {
            self.error(
                node.range(),
                format!("wasm {kind} bindings must be `$` prefixed"),
            );
            return None;
        }

        if text.contains("::") || text.contains('.') {
            self.error(
                node.range(),
                format!("wasm {kind} binding `{text}` must be a bare `$ident`"),
            );
            return None;
        }

        let bare = &text[1..];
        if !is_bare_identifier_text(bare) {
            self.error(
                node.range(),
                format!("wasm {kind} binding `{text}` must be a bare `$ident`"),
            );
            return None;
        }

        Some(text)
    }

    fn parse_optional_wasm_binding_name(
        &mut self,
        items: &[ast::SExpr],
        kind: &str,
    ) -> Option<String> {
        let candidate = items.get(1)?;
        let text = self.sexpr_atom_text(candidate)?;
        if !text.starts_with('$') {
            return None;
        }
        self.parse_wasm_binding_name(candidate, kind)
    }

    fn parse_optional_wasm_binding(
        &mut self,
        items: &[ast::SExpr],
        kind: &str,
    ) -> (Option<(String, TextRange)>, usize) {
        let Some(candidate) = items.get(1) else {
            return (None, 1);
        };
        let Some(text) = self.sexpr_atom_text(candidate) else {
            return (None, 1);
        };
        if !text.starts_with('$') {
            return (None, 1);
        }

        let name = self.parse_wasm_binding_name(candidate, kind);
        (name.map(|name| (name, candidate.range())), 2)
    }

    fn sexpr_list_items<'a>(&self, sexpr: &'a ast::SExpr) -> Option<&'a [ast::SExpr]> {
        match sexpr {
            ast::SExpr::List { items, .. } => Some(items),
            _ => None,
        }
    }

    fn sexpr_atom_text(&self, sexpr: &ast::SExpr) -> Option<String> {
        match sexpr {
            ast::SExpr::Atom { .. } => sexpr.range().text(&self.source_contents),
            _ => None,
        }
    }

    fn parse_wasm_u32_immediate(&self, node: &ast::SExpr, context: &str) -> Option<u32> {
        let text = self.sexpr_atom_text(node)?;
        let Some(value) = parse_wasm_integer_text(&text) else {
            self.error(node.range(), format!("expected unsigned integer {context}"));
            return None;
        };

        if value < 0 {
            self.error(node.range(), format!("expected unsigned integer {context}"));
            return None;
        }

        u32::try_from(value).ok().or_else(|| {
            self.error(
                node.range(),
                format!("unsigned integer {context} is out of range"),
            );
            None
        })
    }

    fn parse_wasm_i32_immediate(&self, node: &ast::SExpr) -> Option<i32> {
        let text = self.sexpr_atom_text(node)?;
        let Some(value) = parse_wasm_integer_text(&text) else {
            self.error(node.range(), "expected i32 integer literal");
            return None;
        };
        i32::try_from(value).ok().or_else(|| {
            self.error(node.range(), "i32 literal is out of range");
            None
        })
    }

    fn parse_wasm_i64_immediate(&self, node: &ast::SExpr) -> Option<i64> {
        let text = self.sexpr_atom_text(node)?;
        let Some(value) = parse_wasm_integer_text(&text) else {
            self.error(node.range(), "expected i64 integer literal");
            return None;
        };
        i64::try_from(value).ok().or_else(|| {
            self.error(node.range(), "i64 literal is out of range");
            None
        })
    }

    fn parse_wasm_f32_immediate(&self, node: &ast::SExpr) -> Option<f32> {
        let text = self.sexpr_atom_text(node)?;
        parse_wasm_float_text(&text, 32)
            .map(|value| value as f32)
            .or_else(|| {
                self.error(node.range(), "expected f32 literal");
                None
            })
    }

    fn parse_wasm_f64_immediate(&self, node: &ast::SExpr) -> Option<f64> {
        let text = self.sexpr_atom_text(node)?;
        parse_wasm_float_text(&text, 64).or_else(|| {
            self.error(node.range(), "expected f64 literal");
            None
        })
    }

    fn unreachable_wasm_instruction(&self, range: TextRange) -> ir::WasmInstructionNode {
        ir::WasmInstructionNode {
            instruction: wasm::Instruction::Unreachable,
            range,
            symbols: Vec::new(),
        }
    }
}

fn namespace_name_wasm(namespace: ir::WasmSymbolNamespace) -> &'static str {
    match namespace {
        ir::WasmSymbolNamespace::Local => "local",
        ir::WasmSymbolNamespace::Function => "function",
        ir::WasmSymbolNamespace::Global => "global",
    }
}

fn merge_text_ranges(lhs: TextRange, rhs: TextRange) -> TextRange {
    match (
        lhs.source(),
        lhs.start(),
        lhs.end(),
        rhs.source(),
        rhs.start(),
        rhs.end(),
    ) {
        (Some(ls), Some(lb), Some(le), Some(rs), Some(rb), Some(re)) if ls == rs => {
            let start = if lb.as_u32() <= rb.as_u32() { lb } else { rb };
            let end = if le.as_u32() >= re.as_u32() { le } else { re };
            TextRange::from_bounds(ls, start, end)
        }
        _ => lhs,
    }
}

fn is_bare_identifier_text(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || unicode_ident::is_xid_start(first)) {
        return false;
    }
    chars.all(unicode_ident::is_xid_continue)
}

fn parse_wasm_heap_type_atom(text: &str) -> Option<HeapType> {
    Some(match text {
        "nofunc" => HeapType::NoFunc,
        "noextern" => HeapType::NoExtern,
        "none" => HeapType::None,
        "anyfunc" | "func" => HeapType::AnyFunc,
        "extern" => HeapType::Extern,
        "any" => HeapType::Any,
        "eq" => HeapType::Eq,
        "i31" => HeapType::I31,
        "struct" => HeapType::AnyStruct,
        "array" => HeapType::AnyArray,
        _ => return None,
    })
}

fn parse_wasm_integer_text(text: &str) -> Option<i128> {
    let mut cleaned = text.replace('_', "");
    if let Some(stripped) = cleaned.strip_suffix('n') {
        cleaned = stripped.to_owned();
    }

    let (negative, body) = if let Some(rest) = cleaned.strip_prefix('-') {
        (true, rest)
    } else {
        (false, cleaned.as_str())
    };

    let (radix, digits) = if let Some(rest) = body.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = body.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = body.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, body)
    };

    if digits.is_empty() {
        return None;
    }

    let value = i128::from_str_radix(digits, radix).ok()?;
    Some(if negative { -value } else { value })
}

fn parse_wasm_float_text(text: &str, _bits: u8) -> Option<f64> {
    let cleaned = text.replace('_', "");
    cleaned.parse::<f64>().ok()
}

fn is_wasm_opcode_name(text: &str) -> bool {
    parse_wasm_simple_instruction(text).is_some()
        || matches!(
            text,
            "block"
                | "loop"
                | "if"
                | "br"
                | "br_if"
                | "br_table"
                | "call"
                | "call_indirect"
                | "local.get"
                | "local.set"
                | "local.tee"
                | "global.get"
                | "global.set"
                | "i32.const"
                | "i64.const"
                | "f32.const"
                | "f64.const"
                | "ref.null"
                | "ref.func"
                | "ref.test"
                | "ref.cast"
                | "br_on_null"
                | "br_on_non_null"
                | "br_on_cast"
                | "br_on_cast_fail"
                | "struct.new"
                | "struct.new_default"
                | "struct.get"
                | "struct.get_s"
                | "struct.get_u"
                | "struct.set"
                | "array.new"
                | "array.new_default"
                | "array.new_fixed"
                | "array.new_data"
                | "array.new_elem"
                | "array.get"
                | "array.get_s"
                | "array.get_u"
                | "array.set"
                | "array.fill"
                | "array.copy"
                | "array.init_data"
                | "array.init_elem"
        )
}

fn parse_wasm_simple_instruction(text: &str) -> Option<wasm::Instruction> {
    Some(match text {
        "unreachable" => wasm::Instruction::Unreachable,
        "nop" => wasm::Instruction::Nop,
        "else" => wasm::Instruction::Else,
        "end" => wasm::Instruction::End,
        "return" => wasm::Instruction::Return,
        "drop" => wasm::Instruction::Drop,
        "select" => wasm::Instruction::Select,
        "ref.is_null" => wasm::Instruction::RefIsNull,
        "ref.eq" => wasm::Instruction::RefEq,
        "ref.as_non_null" => wasm::Instruction::RefAsNonNull,
        "array.len" => wasm::Instruction::ArrayLen,
        "any.convert_extern" => wasm::Instruction::AnyConvertExtern,
        "extern.convert_any" => wasm::Instruction::ExternConvertAny,
        "ref.i31" => wasm::Instruction::RefI31,
        "i31.get_s" => wasm::Instruction::I31GetS,
        "i31.get_u" => wasm::Instruction::I31GetU,

        "i32.eqz" => wasm::Instruction::I32Eqz,
        "i32.eq" => wasm::Instruction::I32Eq,
        "i32.ne" => wasm::Instruction::I32Ne,
        "i32.lt_s" => wasm::Instruction::I32LtS,
        "i32.lt_u" => wasm::Instruction::I32LtU,
        "i32.gt_s" => wasm::Instruction::I32GtS,
        "i32.gt_u" => wasm::Instruction::I32GtU,
        "i32.le_s" => wasm::Instruction::I32LeS,
        "i32.le_u" => wasm::Instruction::I32LeU,
        "i32.ge_s" => wasm::Instruction::I32GeS,
        "i32.ge_u" => wasm::Instruction::I32GeU,
        "i32.clz" => wasm::Instruction::I32Clz,
        "i32.ctz" => wasm::Instruction::I32Ctz,
        "i32.popcnt" => wasm::Instruction::I32Popcnt,
        "i32.add" => wasm::Instruction::I32Add,
        "i32.sub" => wasm::Instruction::I32Sub,
        "i32.mul" => wasm::Instruction::I32Mul,
        "i32.div_s" => wasm::Instruction::I32DivS,
        "i32.div_u" => wasm::Instruction::I32DivU,
        "i32.rem_s" => wasm::Instruction::I32RemS,
        "i32.rem_u" => wasm::Instruction::I32RemU,
        "i32.and" => wasm::Instruction::I32And,
        "i32.or" => wasm::Instruction::I32Or,
        "i32.xor" => wasm::Instruction::I32Xor,
        "i32.shl" => wasm::Instruction::I32Shl,
        "i32.shr_s" => wasm::Instruction::I32ShrS,
        "i32.shr_u" => wasm::Instruction::I32ShrU,
        "i32.rotl" => wasm::Instruction::I32Rotl,
        "i32.rotr" => wasm::Instruction::I32Rotr,

        "i64.eqz" => wasm::Instruction::I64Eqz,
        "i64.eq" => wasm::Instruction::I64Eq,
        "i64.ne" => wasm::Instruction::I64Ne,
        "i64.lt_s" => wasm::Instruction::I64LtS,
        "i64.lt_u" => wasm::Instruction::I64LtU,
        "i64.gt_s" => wasm::Instruction::I64GtS,
        "i64.gt_u" => wasm::Instruction::I64GtU,
        "i64.le_s" => wasm::Instruction::I64LeS,
        "i64.le_u" => wasm::Instruction::I64LeU,
        "i64.ge_s" => wasm::Instruction::I64GeS,
        "i64.ge_u" => wasm::Instruction::I64GeU,
        "i64.clz" => wasm::Instruction::I64Clz,
        "i64.ctz" => wasm::Instruction::I64Ctz,
        "i64.popcnt" => wasm::Instruction::I64Popcnt,
        "i64.add" => wasm::Instruction::I64Add,
        "i64.sub" => wasm::Instruction::I64Sub,
        "i64.mul" => wasm::Instruction::I64Mul,
        "i64.div_s" => wasm::Instruction::I64DivS,
        "i64.div_u" => wasm::Instruction::I64DivU,
        "i64.rem_s" => wasm::Instruction::I64RemS,
        "i64.rem_u" => wasm::Instruction::I64RemU,
        "i64.and" => wasm::Instruction::I64And,
        "i64.or" => wasm::Instruction::I64Or,
        "i64.xor" => wasm::Instruction::I64Xor,
        "i64.shl" => wasm::Instruction::I64Shl,
        "i64.shr_s" => wasm::Instruction::I64ShrS,
        "i64.shr_u" => wasm::Instruction::I64ShrU,
        "i64.rotl" => wasm::Instruction::I64Rotl,
        "i64.rotr" => wasm::Instruction::I64Rotr,

        "f32.eq" => wasm::Instruction::F32Eq,
        "f32.ne" => wasm::Instruction::F32Ne,
        "f32.lt" => wasm::Instruction::F32Lt,
        "f32.gt" => wasm::Instruction::F32Gt,
        "f32.le" => wasm::Instruction::F32Le,
        "f32.ge" => wasm::Instruction::F32Ge,
        "f32.abs" => wasm::Instruction::F32Abs,
        "f32.neg" => wasm::Instruction::F32Neg,
        "f32.ceil" => wasm::Instruction::F32Ceil,
        "f32.floor" => wasm::Instruction::F32Floor,
        "f32.trunc" => wasm::Instruction::F32Trunc,
        "f32.nearest" => wasm::Instruction::F32Nearest,
        "f32.sqrt" => wasm::Instruction::F32Sqrt,
        "f32.add" => wasm::Instruction::F32Add,
        "f32.sub" => wasm::Instruction::F32Sub,
        "f32.mul" => wasm::Instruction::F32Mul,
        "f32.div" => wasm::Instruction::F32Div,
        "f32.min" => wasm::Instruction::F32Min,
        "f32.max" => wasm::Instruction::F32Max,
        "f32.copysign" => wasm::Instruction::F32Copysign,

        "f64.eq" => wasm::Instruction::F64Eq,
        "f64.ne" => wasm::Instruction::F64Ne,
        "f64.lt" => wasm::Instruction::F64Lt,
        "f64.gt" => wasm::Instruction::F64Gt,
        "f64.le" => wasm::Instruction::F64Le,
        "f64.ge" => wasm::Instruction::F64Ge,
        "f64.abs" => wasm::Instruction::F64Abs,
        "f64.neg" => wasm::Instruction::F64Neg,
        "f64.ceil" => wasm::Instruction::F64Ceil,
        "f64.floor" => wasm::Instruction::F64Floor,
        "f64.trunc" => wasm::Instruction::F64Trunc,
        "f64.nearest" => wasm::Instruction::F64Nearest,
        "f64.sqrt" => wasm::Instruction::F64Sqrt,
        "f64.add" => wasm::Instruction::F64Add,
        "f64.sub" => wasm::Instruction::F64Sub,
        "f64.mul" => wasm::Instruction::F64Mul,
        "f64.div" => wasm::Instruction::F64Div,
        "f64.min" => wasm::Instruction::F64Min,
        "f64.max" => wasm::Instruction::F64Max,
        "f64.copysign" => wasm::Instruction::F64Copysign,

        "i32.wrap_i64" => wasm::Instruction::I32WrapI64,
        "i32.trunc_f32_s" => wasm::Instruction::I32TruncF32S,
        "i32.trunc_f32_u" => wasm::Instruction::I32TruncF32U,
        "i32.trunc_f64_s" => wasm::Instruction::I32TruncF64S,
        "i32.trunc_f64_u" => wasm::Instruction::I32TruncF64U,
        "i64.extend_i32_s" => wasm::Instruction::I64ExtendI32S,
        "i64.extend_i32_u" => wasm::Instruction::I64ExtendI32U,
        "i64.trunc_f32_s" => wasm::Instruction::I64TruncF32S,
        "i64.trunc_f32_u" => wasm::Instruction::I64TruncF32U,
        "i64.trunc_f64_s" => wasm::Instruction::I64TruncF64S,
        "i64.trunc_f64_u" => wasm::Instruction::I64TruncF64U,
        "f32.convert_i32_s" => wasm::Instruction::F32ConvertI32S,
        "f32.convert_i32_u" => wasm::Instruction::F32ConvertI32U,
        "f32.convert_i64_s" => wasm::Instruction::F32ConvertI64S,
        "f32.convert_i64_u" => wasm::Instruction::F32ConvertI64U,
        "f32.demote_f64" => wasm::Instruction::F32DemoteF64,
        "f64.convert_i32_s" => wasm::Instruction::F64ConvertI32S,
        "f64.convert_i32_u" => wasm::Instruction::F64ConvertI32U,
        "f64.convert_i64_s" => wasm::Instruction::F64ConvertI64S,
        "f64.convert_i64_u" => wasm::Instruction::F64ConvertI64U,
        "f64.promote_f32" => wasm::Instruction::F64PromoteF32,
        "i32.reinterpret_f32" => wasm::Instruction::I32ReinterpretF32,
        "i64.reinterpret_f64" => wasm::Instruction::I64ReinterpretF64,
        "f32.reinterpret_i32" => wasm::Instruction::F32ReinterpretI32,
        "f64.reinterpret_i64" => wasm::Instruction::F64ReinterpretI64,
        "i32.extend8_s" => wasm::Instruction::I32Extend8S,
        "i32.extend16_s" => wasm::Instruction::I32Extend16S,
        "i64.extend8_s" => wasm::Instruction::I64Extend8S,
        "i64.extend16_s" => wasm::Instruction::I64Extend16S,
        "i64.extend32_s" => wasm::Instruction::I64Extend32S,
        "i32.trunc_sat_f32_s" => wasm::Instruction::I32TruncSatF32S,
        "i32.trunc_sat_f32_u" => wasm::Instruction::I32TruncSatF32U,
        "i32.trunc_sat_f64_s" => wasm::Instruction::I32TruncSatF64S,
        "i32.trunc_sat_f64_u" => wasm::Instruction::I32TruncSatF64U,
        "i64.trunc_sat_f32_s" => wasm::Instruction::I64TruncSatF32S,
        "i64.trunc_sat_f32_u" => wasm::Instruction::I64TruncSatF32U,
        "i64.trunc_sat_f64_s" => wasm::Instruction::I64TruncSatF64S,
        "i64.trunc_sat_f64_u" => wasm::Instruction::I64TruncSatF64U,

        _ => return None,
    })
}
