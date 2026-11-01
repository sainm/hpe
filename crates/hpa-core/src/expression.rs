use std::collections::BTreeSet;

use jaq_core::data;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars};
use jaq_json::Val;
use serde_json::Value;

use crate::error::{CoreError, CoreResult};

const FORBIDDEN_IDENTIFIERS: &[&str] = &[
    "input",
    "inputs",
    "env",
    "now",
    "debug",
    "stderr",
    "halt",
    "halt_error",
    "input_filename",
    "include",
    "import",
];

#[derive(Debug, Clone, Default)]
pub struct ExpressionScope {
    pub inputs: BTreeSet<String>,
    pub variables: BTreeSet<String>,
    pub nodes: BTreeSet<String>,
    pub allow_loop: bool,
    pub allow_parallel: bool,
}

impl ExpressionScope {
    pub fn new(
        inputs: impl IntoIterator<Item = String>,
        variables: impl IntoIterator<Item = String>,
        nodes: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            inputs: inputs.into_iter().collect(),
            variables: variables.into_iter().collect(),
            nodes: nodes.into_iter().collect(),
            allow_loop: false,
            allow_parallel: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    Literal(String),
    Expression(String),
}

type CompiledFilter = jaq_core::Filter<data::JustLut<Val>>;

fn defs() -> impl Iterator<Item = jaq_core::load::parse::Def<&'static str>> {
    jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs())
}

fn funs() -> impl Iterator<
    Item = (
        &'static str,
        Box<[jaq_core::Bind]>,
        jaq_core::Native<data::JustLut<Val>>,
    ),
> {
    jaq_core::funs::<data::JustLut<Val>>()
        .chain(jaq_std::funs::<data::JustLut<Val>>())
        .chain(jaq_json::funs::<data::JustLut<Val>>())
}

fn compile_filter(source: &str) -> Result<CompiledFilter, String> {
    let arena = Arena::default();
    let file = File {
        code: source,
        path: (),
    };
    let loader = Loader::new(defs());
    let modules = loader
        .load(&arena, file)
        .map_err(|errors| format!("{errors:?}"))?;
    let compiler = Compiler::<&str, data::JustLut<Val>>::default().with_funs(funs());
    compiler
        .compile(modules)
        .map_err(|errors| format!("{errors:?}"))
}

pub fn check_forbidden(source: &str) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
            index += 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                index += 1;
            }
            b'#' => {
                in_comment = true;
                index += 1;
            }
            b'$' => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_identifier_byte(bytes[index]) {
                    index += 1;
                }
                if &source[start..index] == "$ENV" {
                    return Err("builtin `$ENV` is forbidden".to_string());
                }
            }
            value if is_identifier_start(value) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_identifier_byte(bytes[index]) {
                    index += 1;
                }
                let token = &source[start..index];
                if FORBIDDEN_IDENTIFIERS.contains(&token) {
                    let previous = source[..start]
                        .bytes()
                        .rev()
                        .find(|byte| !byte.is_ascii_whitespace());
                    if !matches!(previous, Some(b'.')) {
                        return Err(format!("builtin `{token}` is forbidden"));
                    }
                }
            }
            _ => index += 1,
        }
    }

    Ok(())
}

pub fn check_expression(source: &str, scope: &ExpressionScope) -> Result<(), String> {
    check_forbidden(source)?;
    compile_filter(source)?;
    let references = scan_references(source, scope);
    if references.is_empty() {
        Ok(())
    } else {
        Err(references.join("; "))
    }
}

pub fn check_template(template: &str, scope: &ExpressionScope) -> Result<(), String> {
    let parts = parse_template(template)?;
    let mut errors = Vec::new();
    for part in parts {
        if let TemplatePart::Expression(source) = part
            && let Err(error) = check_expression(&source, scope)
        {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn parse_template(template: &str) -> Result<Vec<TemplatePart>, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '\\'
            && index + 2 < chars.len()
            && chars[index + 1] == '$'
            && chars[index + 2] == '{'
        {
            literal.push('$');
            literal.push('{');
            index += 3;
            continue;
        }
        if chars[index] == '$' && index + 1 < chars.len() && chars[index + 1] == '{' {
            if !literal.is_empty() {
                parts.push(TemplatePart::Literal(std::mem::take(&mut literal)));
            }
            let end = find_expression_end(&chars, index + 2)
                .ok_or_else(|| "unclosed `${` in template".to_string())?;
            let source: String = chars[index + 2..end].iter().collect();
            if source.trim().is_empty() {
                return Err("empty expression in template".to_string());
            }
            parts.push(TemplatePart::Expression(source));
            index = end + 1;
            continue;
        }
        literal.push(chars[index]);
        index += 1;
    }

    if !literal.is_empty() {
        parts.push(TemplatePart::Literal(literal));
    }
    Ok(parts)
}

fn find_expression_end(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut index = start;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    while index < chars.len() {
        let current = chars[index];
        if in_comment {
            if current == '\n' {
                in_comment = false;
            }
            index += 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match current {
            '"' => in_string = true,
            '#' => in_comment = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn scan_references(source: &str, scope: &ExpressionScope) -> Vec<String> {
    const KEYWORDS: &[&str] = &[
        "input",
        "vars",
        "nodes",
        "env",
        "execution",
        "loop",
        "parallel",
    ];
    let bytes = source.as_bytes();
    let mut errors = Vec::new();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
            index += 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                index += 1;
            }
            b'#' => {
                in_comment = true;
                index += 1;
            }
            b'.' => {
                let mut matched = false;
                for keyword in KEYWORDS {
                    let start = index + 1;
                    if !source[start..].starts_with(keyword) {
                        continue;
                    }
                    let end = start + keyword.len();
                    if end < bytes.len() && is_identifier_byte(bytes[end]) {
                        continue;
                    }
                    matched = true;
                    match *keyword {
                        "loop" => {
                            if !scope.allow_loop {
                                errors.push(
                                    "`.loop` is only available inside a loop body".to_string(),
                                );
                            }
                        }
                        "parallel" => {
                            if !scope.allow_parallel {
                                errors.push(
                                    "`.parallel` is only available inside a parallel branch"
                                        .to_string(),
                                );
                            }
                        }
                        "input" | "vars" | "nodes" if end < bytes.len() && bytes[end] == b'.' => {
                            let name_start = end + 1;
                            let mut name_end = name_start;
                            while name_end < bytes.len() && is_property_byte(bytes[name_end]) {
                                name_end += 1;
                            }
                            let name = &source[name_start..name_end];
                            if !name.is_empty() {
                                check_reference(keyword, name, scope, &mut errors);
                                index = name_end;
                                break;
                            }
                        }
                        _ => {}
                    }
                    index = end;
                    break;
                }
                if !matched {
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }

    errors
}

fn check_reference(keyword: &str, name: &str, scope: &ExpressionScope, errors: &mut Vec<String>) {
    match keyword {
        "nodes" => {
            if !scope.nodes.contains(name) {
                errors.push(format!("unknown node reference `.nodes.{name}`"));
            }
        }
        "input" => {
            if !scope.inputs.contains(name) {
                errors.push(format!("undeclared input reference `.input.{name}`"));
            }
        }
        "vars" if !scope.variables.contains(name) => {
            errors.push(format!("undeclared variable reference `.vars.{name}`"));
        }
        _ => {}
    }
}

pub fn evaluate(source: &str, context: &Value) -> CoreResult<Value> {
    check_forbidden(source).map_err(|message| CoreError::ExpressionForbidden {
        expression: source.to_string(),
        message,
    })?;
    let filter = compile_filter(source).map_err(|message| CoreError::Expression {
        expression: source.to_string(),
        message,
    })?;
    let input: Val =
        serde_json::from_value(context.clone()).map_err(|error| CoreError::Expression {
            expression: source.to_string(),
            message: error.to_string(),
        })?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let mut outputs = filter.id.run((ctx, input));
    let first = outputs.next().ok_or_else(|| CoreError::ExpressionArity {
        expression: source.to_string(),
        count: 0,
    })?;
    let value = first.map_err(|error| CoreError::Expression {
        expression: source.to_string(),
        message: exn_message(error),
    })?;
    if outputs.next().is_some() {
        return Err(CoreError::ExpressionArity {
            expression: source.to_string(),
            count: 2,
        });
    }
    val_to_json(value, source)
}

pub fn evaluate_optional(source: &str, context: &Value) -> CoreResult<Option<Value>> {
    check_forbidden(source).map_err(|message| CoreError::ExpressionForbidden {
        expression: source.to_string(),
        message,
    })?;
    let filter = compile_filter(source).map_err(|message| CoreError::Expression {
        expression: source.to_string(),
        message,
    })?;
    let input: Val =
        serde_json::from_value(context.clone()).map_err(|error| CoreError::Expression {
            expression: source.to_string(),
            message: error.to_string(),
        })?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let mut outputs = filter.id.run((ctx, input));
    let Some(first) = outputs.next() else {
        return Ok(None);
    };
    let value = first.map_err(|error| CoreError::Expression {
        expression: source.to_string(),
        message: exn_message(error),
    })?;
    if outputs.next().is_some() {
        return Err(CoreError::ExpressionArity {
            expression: source.to_string(),
            count: 2,
        });
    }
    Ok(Some(val_to_json(value, source)?))
}

pub fn evaluate_bool(source: &str, context: &Value) -> CoreResult<bool> {
    match evaluate(source, context)? {
        Value::Bool(value) => Ok(value),
        Value::Null => Ok(false),
        other => Err(CoreError::Expression {
            expression: source.to_string(),
            message: format!("condition must produce a boolean, got {other}"),
        }),
    }
}

pub fn render_template(template: &str, context: &Value) -> CoreResult<String> {
    let parts = parse_template(template).map_err(|message| CoreError::Expression {
        expression: template.to_string(),
        message,
    })?;
    let mut out = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(text) => out.push_str(&text),
            TemplatePart::Expression(source) => {
                let value = evaluate_optional(&source, context)?;
                match value {
                    None | Some(Value::Null) => {}
                    Some(Value::String(text)) => out.push_str(&text),
                    Some(other) => {
                        out.push_str(&serde_json::to_string(&other).map_err(|error| {
                            CoreError::Expression {
                                expression: source.clone(),
                                message: error.to_string(),
                            }
                        })?);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn exn_message(error: jaq_core::Exn<'_, Val>) -> String {
    match error.get_err() {
        Ok(error) => error.to_string(),
        Err(exn) => format!("{exn:?}"),
    }
}

fn val_to_json(value: Val, source: &str) -> CoreResult<Value> {
    let text = value.to_string();
    serde_json::from_str(&text).map_err(|error| CoreError::ExpressionValue {
        expression: source.to_string(),
        message: error.to_string(),
    })
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_property_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
