//! The game's script commands (natives), as game.wasm registers them: for
//! each, its name, hash, handler (a function table index) and what the
//! module says about its types. Written as a table for crates that call
//! natives (glue5-mods), which add their own data (names of parameters,
//! documentation) on top.
//!
//! Every `<group>_commands::SetupScriptCommands()` (and the built-in ones)
//! calls `rage::scrThread::Register[Unused]Command(hash, handler, name, file,
//! signature)` with constants: five `i64.const` and the call. A handler,
//! `...::scrWrapped_<NAME>::Call(rage::scrThread::Info&)`, usually calls a
//! typed `<group>_commands::Command<Name>(...)`, whose demangled name gives
//! the C++ parameter types.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, ensure};
use wasmparser::{DataKind, ElementItems, ElementKind, KnownCustom, Name, Operator, Parser, Payload, TypeRef, ValType};

const REGISTER: &str = "rage::scrThread::RegisterCommand(unsigned long, void (*)(rage::scrThread::Info&), char const*, char const*, rage::scrSignature)";
const REGISTER_UNUSED: &str = "rage::scrThread::RegisterUnusedCommand(unsigned long, void (*)(rage::scrThread::Info&), char const*, char const*, rage::scrSignature)";
/// Where the first data segment (read-only data, strings) is loaded.
const RODATA: u64 = 1024;

struct Registration {
    hash: u64,
    handler: u64,
    name: u64,
    signature: u64,
    unused: bool,
    /// The function that registers it.
    setup: u32,
}

/// The table, one line per native, tab-separated:
/// `name hash handler unused group command-params command-wasm-type signature`
/// (`-` for no typed command function).
pub fn table(wasm: &[u8]) -> Result<String> {
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut types: Vec<(Vec<ValType>, Vec<ValType>)> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new();
    let mut imported = 0u32;
    let mut table: HashMap<u64, u32> = HashMap::new();
    let mut rodata: &[u8] = &[];
    let mut bodies: Vec<wasmparser::FunctionBody> = Vec::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    let ty = ty?;
                    types.push((ty.params().to_vec(), ty.results().to_vec()));
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    if let TypeRef::Func(ty) = import?.ty {
                        func_types.push(ty);
                        imported += 1;
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader {
                    func_types.push(ty?);
                }
            }
            Payload::ElementSection(reader) => {
                for element in reader {
                    let element = element?;
                    let ElementKind::Active { offset_expr, .. } = element.kind else { continue };
                    let base = match offset_expr.get_operators_reader().read()? {
                        Operator::I32Const { value } => value as u32 as u64,
                        Operator::I64Const { value } => value as u64,
                        other => anyhow::bail!("element offset {other:?}"),
                    };
                    match element.items {
                        ElementItems::Functions(funcs) => {
                            for (i, f) in funcs.into_iter().enumerate() {
                                table.insert(base + i as u64, f?);
                            }
                        }
                        ElementItems::Expressions(_, exprs) => {
                            for (i, e) in exprs.into_iter().enumerate() {
                                if let Operator::RefFunc { function_index } = e?.get_operators_reader().read()? {
                                    table.insert(base + i as u64, function_index);
                                }
                            }
                        }
                    }
                }
            }
            Payload::DataSection(reader) => {
                if let Some(first) = reader.into_iter().next() {
                    let first = first?;
                    ensure!(matches!(first.kind, DataKind::Passive), "the first data segment is not passive");
                    rodata = first.data;
                }
            }
            Payload::CodeSectionEntry(body) => bodies.push(body),
            Payload::CustomSection(section) => {
                if let KnownCustom::Name(reader) = section.as_known() {
                    for name in reader {
                        if let Name::Function(map) = name? {
                            for naming in map {
                                let naming = naming?;
                                names.insert(naming.index, naming.name.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let index_of = |wanted: &str| names.iter().find(|(_, n)| n.as_str() == wanted).map(|(&i, _)| i);
    let register = index_of(REGISTER).context("RegisterCommand not in the name section")?;
    let register_unused = index_of(REGISTER_UNUSED).context("RegisterUnusedCommand not in the name section")?;
    let body = |func: u32| bodies.get(func.checked_sub(imported)? as usize);

    // The registrations: five constants, then the call.
    let mut found = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let mut consts: Vec<u64> = Vec::new();
        for op in body.get_operators_reader()? {
            match op? {
                Operator::I64Const { value } => consts.push(value as u64),
                Operator::Call { function_index }
                    if function_index == register || function_index == register_unused =>
                {
                    if let [.., hash, handler, name, _file, signature] = consts[..] {
                        found.push(Registration {
                            hash,
                            handler,
                            name,
                            signature,
                            unused: function_index == register_unused,
                            setup: imported + i as u32,
                        });
                    }
                    consts.clear();
                }
                _ => consts.clear(),
            }
        }
    }
    ensure!(found.len() > 5000, "only {} natives found", found.len());

    let cstr = |addr: u64| -> Result<String> {
        let at = addr.checked_sub(RODATA).context("string below read-only data")? as usize;
        let len = rodata.get(at..).and_then(|s| s.iter().position(|&b| b == 0)).context("string out of range")?;
        Ok(String::from_utf8_lossy(&rodata[at..at + len]).into_owned())
    };
    let wasm_type = |func: u32| -> String {
        let Some((params, results)) = func_types.get(func as usize).and_then(|&t| types.get(t as usize)) else {
            return "-".into();
        };
        let list = |v: &[ValType]| v.iter().map(|t| format!("{t}")).collect::<Vec<_>>().join(",");
        format!("{}->{}", list(params), list(results))
    };

    // Typed commands by group and name without underscores (CREATE_VEHICLE
    // -> vehicle, CREATEVEHICLE), for handlers that inlined theirs.
    let mut commands: HashMap<(String, String), u32> = HashMap::new();
    for (&index, name) in &names {
        if let Some((group, rest)) = name.split_once("_commands::Command")
            && let Some(open) = rest.find('(')
        {
            let key = (group.to_string(), rest[..open].to_ascii_uppercase());
            commands.entry(key).or_insert(index);
        }
    }
    let typed = |function_index: u32| -> Option<(String, String)> {
        let callee = names.get(&function_index)?;
        let (_, rest) = callee.split_once("_commands::Command")?;
        let (open, close) = (rest.find('(')?, rest.rfind(')')?);
        let params = &rest[open + 1..close];
        Some((if params.is_empty() { "void".into() } else { params.to_string() }, wasm_type(function_index)))
    };

    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    for r in &found {
        let name = cstr(r.name)?;
        ensure!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
            "unexpected native name {name:?}"
        );
        if !seen.insert(name.clone()) {
            continue;
        }
        let setup = names.get(&r.setup).map(String::as_str).unwrap_or("");
        let group = setup.split_once("_commands::").map_or("builtin", |(g, _)| g);
        // The typed command the handler calls, else the one named like it.
        let mut command = None;
        if let Some(body) = table.get(&r.handler).copied().and_then(body) {
            for op in body.get_operators_reader()? {
                if let Operator::Call { function_index } = op?
                    && names.get(&function_index).is_some_and(|n| n.contains("_commands::Command"))
                {
                    command = typed(function_index);
                    break;
                }
            }
        }
        if command.is_none() {
            command = commands.get(&(group.to_string(), name.replace('_', ""))).copied().and_then(typed);
        }
        let (params, command_type) = command.unwrap_or_else(|| ("-".into(), "-".into()));
        writeln!(
            out,
            "{name}\t{:#018x}\t{}\t{}\t{group}\t{params}\t{command_type}\t{:#x}",
            r.hash, r.handler, r.unused as u8, r.signature
        )?;
    }
    Ok(out)
}
