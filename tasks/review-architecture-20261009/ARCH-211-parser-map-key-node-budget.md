---
id: ARCH-211
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-184
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Charge map key expressions to the parser's existing AST budget

## Current ownership and evidence

The language leaf owns source admission in parser::parse_program. ParseCtx documents ast_node_count as the total AST nodes created and fixes max_ast_nodes at 2,000 (parser/mod.rs:21–46,80–88). The original DSL security model independently lists this parser limit alongside the 64 KiB source and 50,000 parse-step limits, explaining that these constants protect the loader/parser itself. The real mod_test.rs test_ast_node_limit accepts 1,000 nil expression statements and rejects the next created node with `too many AST nodes: 2001 (max 2000)`.

The map branch of parse_atom (parser/expr.rs:209–259) bypasses this account for its keys. Each key calls lit::parse_string_literal directly and returns an Expr::string, retained as the first Expr in each MapLiteral pair. lit.rs does not charge its unused ParseCtx argument. Values go through parse_expr/parse_atom and are charged. The map itself receives the existing parse_atom charge and another charge after the pair loop; that additional conservative charge covers only one extra node, irrespective of the number of keys.

A concrete static witness is 333 newline-separated copies of `{"a":1,"b":2,"c":3}`. This is 6,660 ASCII source bytes, below the source limit, uses no recursive nesting and has ample parse budget. Each statement charges one statement, two map charges and three values: six charges, or 1,998 total. Each retained statement actually contains one statement, one map expression, three key expressions and three value expressions: eight AST nodes, or 2,664 total. The first actual crossing occurs in statement 251 while the parser's account remains below 2,000. The entire program returns from parse_program without the promised node-limit failure. This count follows the actual AST types and parser branches; no program was executed.

The witness reaches a real configuration seam: Controller EdgionPluginsHandler::preparse calls schema validation; DslConfig::validation_error calls compile_and_validate; its selected source follows resolve_script → compile_dsl_source → parse_program. Gateway DslPlugin::resolve_parts independently uses the same schema entry. compile_dsl_source parses the whole program and then compiles it before running Validator. Compiler::compile_expr traverses both members of every map pair, emitting their constants, MakeMap and the expression-statement Pop.

The default 1,000-instruction validator rejects this particular witness later: compilation emits 2,665 instructions including the implicit ReturnNext. Consequently, this finding does not claim that the witness reaches VM execution or bypasses runtime map/stack/step limits. That later rejection occurs after the extra AST and compiler work; it cannot enforce the parser's independent admission bound retroactively.

## Root cause and bounded improvement

The map parser constructs one supported AST-node family outside the existing creation account. Charge each key through ParseCtx::add_node before calling parse_string_literal in this branch. Keep that policy at the map-key creation site: charging inside the shared literal helper would double-charge ordinary string expressions already admitted through parse_atom.

Preserve the existing conservative map charge and all other limits. Removing or recalibrating additional charges is unnecessary to close this omission and would separately change the admitted source domain. The narrow correction makes the current 2,000-node ceiling cover map keys before their decoded String and Expr are constructed. It requires no public AST factory, framework, runtime dependency, second validation pass or generated inventory.

An alternative is to redefine map keys as non-expression AST data. That would change compiler and AST consumer contracts to address a local accounting omission. A post-parse tree count is also insufficient: the parser would already have allocated the nodes it was supposed to reject. The existing local budget mechanism is the appropriate owner.

## Required invariants and future verification

Retain the source-byte, parse-step and nesting limits; UTF-8-safe slicing; actual quoted-newline and existing escape behavior; map key spelling/order and duplicate-key behavior; spans; operator precedence; compiler and validator checks; bytecode precedence/version handling; and independent Controller/Gateway validation. The correction intentionally makes the existing AST-limit error occur earlier for map-heavy sources that previously exceeded the documented ceiling. Do not echo source/key material in the new limit error or add logging or metrics.

Existing parser expr/mod/stmt test bodies were fully read. They cover maps, expression nesting/restoration, ordinary operators and the nil-statement node boundary, but no inspected test charges map keys at the AST boundary. Future focused parser tests should cover ordinary and empty maps, key-heavy maps, repeated small maps, mixed nested expressions and limits around the first crossing. Establish both node-accounting failure and unchanged AST values below the limit. A schema-level source-resolution check should prove that map-heavy input fails at the parser budget rather than only after bytecode generation, while preserving bytecode-first selection. Tests should use the existing private-access unit layout, without widening production visibility or constructing mock runtime state.

## Original decisions and scope limits

The original security model explicitly separates parser limits from compiled instruction/map limits. The custom-engine ADR requires engine-owned bounds and independent configured-source or bytecode validation; it does not accept excluding map key Expr nodes. The quoted-string newline guard preserves the existing string language and is unaffected. Current findings and pending DSL task paths were searched; no matching map-key AST-budget cause was found. ARCH-050's call-local plugin validation and the XML parser ownership findings concern different producers and accounts.

The 64 KiB source limit and other counters still bound this workload. This is a finite early-admission defect, not an unbounded allocation, denial-of-service incident, measured performance loss or admitted-runtime sandbox escape. No builds, tests, requests or application execution were performed. The benefit is enforcing the existing parser boundary before downstream work, without redesigning the language or its runtime.
