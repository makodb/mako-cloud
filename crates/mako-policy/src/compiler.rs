use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde_json::Value;

use crate::{
    DiagnosticSeverity, DocumentOperation, PolicyDiagnostic, PolicyEffect, PolicyModelError,
    PolicyRuleId, PolicySet, PolicyVersion, SourceSpan,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyCompilerLimits {
    pub maximum_source_bytes: usize,
    pub maximum_ast_nodes: usize,
}

impl Default for PolicyCompilerLimits {
    fn default() -> Self {
        Self {
            maximum_source_bytes: 16 * 1024,
            maximum_ast_nodes: 256,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PolicyCompiler {
    limits: PolicyCompilerLimits,
}

impl PolicyCompiler {
    #[must_use]
    pub fn new(limits: PolicyCompilerLimits) -> Self {
        Self { limits }
    }

    #[must_use]
    pub fn limits(&self) -> PolicyCompilerLimits {
        self.limits
    }

    pub fn compile(
        &self,
        policy: &PolicySet,
        collection_schema: &Value,
    ) -> Result<PolicyCompilation, PolicyCompileError> {
        let schema = PolicySchema::from_json_schema(collection_schema)?;
        let mut diagnostics = Vec::new();
        let mut compiled_rules = Vec::new();
        for rule in policy.rules() {
            if rule.expression().len() > self.limits.maximum_source_bytes {
                diagnostics.push(diagnostic(
                    "source_too_large",
                    "policy expression exceeds the configured source-size limit",
                    rule.expression(),
                    0,
                    rule.expression().len().max(1),
                )?);
                continue;
            }
            let tokens = match lex(rule.expression()) {
                Ok(tokens) => tokens,
                Err(error) => {
                    diagnostics.push(error.into_diagnostic(rule.expression())?);
                    continue;
                }
            };
            let expression = match Parser::new(tokens).parse() {
                Ok(expression) => expression,
                Err(error) => {
                    diagnostics.push(error.into_diagnostic(rule.expression())?);
                    continue;
                }
            };
            if expression.node_count() > self.limits.maximum_ast_nodes {
                diagnostics.push(diagnostic(
                    "cost_limit_exceeded",
                    "policy expression exceeds the configured AST cost limit",
                    rule.expression(),
                    0,
                    rule.expression().len().max(1),
                )?);
                continue;
            }
            let mut rule_diagnostics = Vec::new();
            let value_type = infer_type(
                &expression,
                &schema,
                rule.operations(),
                rule.expression(),
                &mut rule_diagnostics,
            )?;
            if !matches!(value_type, ValueType::Bool | ValueType::Dynamic) {
                rule_diagnostics.push(diagnostic(
                    "expression_not_boolean",
                    "policy expression must evaluate to a boolean",
                    rule.expression(),
                    expression.span().start,
                    expression.span().length,
                )?);
            }
            if rule_diagnostics.is_empty() {
                compiled_rules.push(CompiledPolicyRule {
                    id: rule.id().clone(),
                    effect: rule.effect(),
                    operations: rule.operations().iter().copied().collect(),
                    expression,
                });
            } else {
                diagnostics.extend(rule_diagnostics);
            }
        }
        let compiled = diagnostics.is_empty().then(|| CompiledPolicySet {
            scope: policy.scope().clone(),
            version: policy.version(),
            rules: compiled_rules,
            maximum_evaluation_nodes: self.limits.maximum_ast_nodes,
        });
        Ok(PolicyCompilation {
            compiled,
            diagnostics,
        })
    }
}

impl Default for PolicyCompiler {
    fn default() -> Self {
        Self::new(PolicyCompilerLimits::default())
    }
}

#[derive(Clone, Debug)]
pub struct PolicyCompilation {
    compiled: Option<CompiledPolicySet>,
    diagnostics: Vec<PolicyDiagnostic>,
}

impl PolicyCompilation {
    #[must_use]
    pub fn compiled(&self) -> Option<&CompiledPolicySet> {
        self.compiled.as_ref()
    }

    #[must_use]
    pub fn into_compiled(self) -> Option<CompiledPolicySet> {
        self.compiled
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[PolicyDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug)]
pub struct CompiledPolicySet {
    scope: mako_api::CollectionScope,
    version: PolicyVersion,
    rules: Vec<CompiledPolicyRule>,
    maximum_evaluation_nodes: usize,
}

impl CompiledPolicySet {
    #[must_use]
    pub fn scope(&self) -> &mako_api::CollectionScope {
        &self.scope
    }

    #[must_use]
    pub const fn version(&self) -> PolicyVersion {
        self.version
    }

    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub(crate) fn rules(&self) -> &[CompiledPolicyRule] {
        &self.rules
    }

    pub(crate) const fn maximum_evaluation_nodes(&self) -> usize {
        self.maximum_evaluation_nodes
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledPolicyRule {
    id: PolicyRuleId,
    effect: PolicyEffect,
    operations: Vec<DocumentOperation>,
    expression: Expression,
}

impl CompiledPolicyRule {
    pub(crate) fn id(&self) -> &PolicyRuleId {
        &self.id
    }

    pub(crate) const fn effect(&self) -> PolicyEffect {
        self.effect
    }

    pub(crate) fn applies_to(&self, operation: DocumentOperation) -> bool {
        self.operations.contains(&operation)
    }

    pub(crate) fn expression(&self) -> &Expression {
        &self.expression
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteSpan {
    start: usize,
    length: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expression {
    Literal(Value, ByteSpan),
    Path(Vec<String>, ByteSpan),
    Not(Box<Self>, ByteSpan),
    And(Box<Self>, Box<Self>, ByteSpan),
    Or(Box<Self>, Box<Self>, ByteSpan),
    Compare(Comparison, Box<Self>, Box<Self>, ByteSpan),
    /// `base[index]`: a trusted-claim value looked up by an evaluated key.
    Index(Box<Self>, Box<Self>, ByteSpan),
}

impl Expression {
    pub(crate) const fn span(&self) -> ByteSpan {
        match self {
            Self::Literal(_, span)
            | Self::Path(_, span)
            | Self::Not(_, span)
            | Self::And(_, _, span)
            | Self::Or(_, _, span)
            | Self::Compare(_, _, _, span)
            | Self::Index(_, _, span) => *span,
        }
    }

    fn node_count(&self) -> usize {
        match self {
            Self::Literal(..) | Self::Path(..) => 1,
            Self::Not(expression, _) => 1 + expression.node_count(),
            Self::And(left, right, _)
            | Self::Or(left, right, _)
            | Self::Compare(_, left, right, _)
            | Self::Index(left, right, _) => 1 + left.node_count() + right.node_count(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueType {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
    Dynamic,
}

#[derive(Debug)]
struct PolicySchema {
    fields: BTreeMap<String, ValueType>,
    /// Object fields whose members the schema does not name -- no
    /// `properties`, or `additionalProperties` left open. What lives under
    /// one is decided by the application, not the schema, so the compiler
    /// cannot call a path beneath it unknown; it types it dynamic, exactly as
    /// it types a trusted claim.
    open_objects: BTreeSet<String>,
}

impl PolicySchema {
    fn from_json_schema(schema: &Value) -> Result<Self, PolicyCompileError> {
        let object = schema.as_object().ok_or(PolicyCompileError::InvalidSchema(
            "collection schema must be an object",
        ))?;
        let properties = object.get("properties").and_then(Value::as_object).ok_or(
            PolicyCompileError::InvalidSchema("collection schema must declare object properties"),
        )?;
        let mut fields = BTreeMap::new();
        let mut open_objects = BTreeSet::new();
        collect_schema_fields("", properties, &mut fields, &mut open_objects)?;
        Ok(Self {
            fields,
            open_objects,
        })
    }

    /// The type of a document path: the declared field, or dynamic when it
    /// lives under an open object, or `None` when the schema says it does not
    /// exist.
    fn field_type(&self, field: &str) -> Option<ValueType> {
        if let Some(value_type) = self.fields.get(field).copied() {
            return Some(value_type);
        }
        let mut prefix = field;
        while let Some((parent, _)) = prefix.rsplit_once('.') {
            if self.open_objects.contains(parent) {
                return Some(ValueType::Dynamic);
            }
            prefix = parent;
        }
        None
    }
}

fn collect_schema_fields(
    prefix: &str,
    properties: &serde_json::Map<String, Value>,
    output: &mut BTreeMap<String, ValueType>,
    open_objects: &mut BTreeSet<String>,
) -> Result<(), PolicyCompileError> {
    for (name, schema) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let value_type = schema
            .as_object()
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str)
            .map_or(ValueType::Dynamic, |value_type| match value_type {
                "null" => ValueType::Null,
                "boolean" => ValueType::Bool,
                "integer" | "number" => ValueType::Number,
                "string" => ValueType::String,
                "array" => ValueType::Array,
                "object" => ValueType::Object,
                _ => ValueType::Dynamic,
            });
        output.insert(path.clone(), value_type);
        if value_type == ValueType::Object {
            let nested = schema
                .as_object()
                .and_then(|schema| schema.get("properties"))
                .and_then(Value::as_object);
            let additional_open = schema
                .as_object()
                .and_then(|schema| schema.get("additionalProperties"))
                .is_none_or(|value| value != &Value::Bool(false));
            match nested {
                Some(nested) => {
                    collect_schema_fields(&path, nested, output, open_objects)?;
                    if additional_open {
                        open_objects.insert(path.clone());
                    }
                }
                None => {
                    open_objects.insert(path.clone());
                }
            }
        }
    }
    Ok(())
}

fn infer_type(
    expression: &Expression,
    schema: &PolicySchema,
    operations: &std::collections::BTreeSet<DocumentOperation>,
    source: &str,
    diagnostics: &mut Vec<PolicyDiagnostic>,
) -> Result<ValueType, PolicyModelError> {
    match expression {
        Expression::Literal(value, _) => Ok(match value {
            Value::Null => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::Number(_) => ValueType::Number,
            Value::String(_) => ValueType::String,
            Value::Array(_) => ValueType::Array,
            Value::Object(_) => ValueType::Object,
        }),
        Expression::Path(path, span) => {
            resolve_path(path, *span, schema, operations, source, diagnostics)
        }
        Expression::Not(inner, span) => {
            let value_type = infer_type(inner, schema, operations, source, diagnostics)?;
            require_boolean(value_type, *span, source, diagnostics)?;
            Ok(ValueType::Bool)
        }
        Expression::And(left, right, span) | Expression::Or(left, right, span) => {
            let left_type = infer_type(left, schema, operations, source, diagnostics)?;
            let right_type = infer_type(right, schema, operations, source, diagnostics)?;
            require_boolean(left_type, *span, source, diagnostics)?;
            require_boolean(right_type, *span, source, diagnostics)?;
            Ok(ValueType::Bool)
        }
        Expression::Compare(comparison, left, right, span) => {
            let left_type = infer_type(left, schema, operations, source, diagnostics)?;
            let right_type = infer_type(right, schema, operations, source, diagnostics)?;
            let compatible = left_type == ValueType::Dynamic
                || right_type == ValueType::Dynamic
                || left_type == right_type
                || matches!(comparison, Comparison::Equal | Comparison::NotEqual)
                    && (left_type == ValueType::Null || right_type == ValueType::Null);
            let ordered = matches!(comparison, Comparison::Equal | Comparison::NotEqual)
                || matches!(
                    left_type,
                    ValueType::Number | ValueType::String | ValueType::Dynamic
                );
            if !compatible || !ordered {
                diagnostics.push(diagnostic(
                    "type_mismatch",
                    "comparison operands have incompatible policy types",
                    source,
                    span.start,
                    span.length,
                )?);
            }
            Ok(ValueType::Bool)
        }
        Expression::Index(base, index, _) => {
            infer_type(base, schema, operations, source, diagnostics)?;
            // Only trusted claims are indexable by a value: they are the one
            // environment root whose shape the schema does not describe, so a
            // missing member is an ordinary `null` rather than a policy bug.
            // A chained index (`claims.a[x][y]`) was checked at its innermost
            // base already.
            let indexable = match base.as_ref() {
                Expression::Path(path, _) => {
                    path.len() >= 2 && path.first().is_some_and(|root| root == "claims")
                }
                Expression::Index(..) => true,
                _ => false,
            };
            if !indexable {
                diagnostics.push(diagnostic(
                    "index_not_allowed",
                    "only trusted claims may be indexed by a value",
                    source,
                    base.span().start,
                    base.span().length,
                )?);
            }
            let index_type = infer_type(index, schema, operations, source, diagnostics)?;
            if !matches!(
                index_type,
                ValueType::String | ValueType::Number | ValueType::Dynamic
            ) {
                diagnostics.push(diagnostic(
                    "index_type_invalid",
                    "index expression must be a string or a number",
                    source,
                    index.span().start,
                    index.span().length,
                )?);
            }
            Ok(ValueType::Dynamic)
        }
    }
}

fn resolve_path(
    path: &[String],
    span: ByteSpan,
    schema: &PolicySchema,
    operations: &std::collections::BTreeSet<DocumentOperation>,
    source: &str,
    diagnostics: &mut Vec<PolicyDiagnostic>,
) -> Result<ValueType, PolicyModelError> {
    let root = path.first().map(String::as_str).unwrap_or_default();
    match root {
        "identity" if path.len() == 2 && matches!(path[1].as_str(), "user_id" | "role") => {
            Ok(ValueType::String)
        }
        // The address the caller's account authenticated as, and whether the
        // environment has confirmed the caller controls it. A rule that hands
        // a document to an address must test both: without the second, an
        // attacker registers the address they want to read.
        "identity" if path.len() == 2 && path[1] == "email" => Ok(ValueType::String),
        "identity" if path.len() == 2 && path[1] == "email_verified" => Ok(ValueType::Bool),
        "claims" if path.len() >= 2 => Ok(ValueType::Dynamic),
        "request" if path.len() == 2 => Ok(ValueType::String),
        "operation" | "project_id" | "environment_id" | "collection_id" if path.len() == 1 => {
            Ok(ValueType::String)
        }
        "old" | "new" if path.len() >= 2 => {
            let invalid_for_operation = operations.iter().any(|operation| {
                matches!(
                    (root, operation),
                    ("old", DocumentOperation::Create)
                        | ("new", DocumentOperation::Read | DocumentOperation::Delete)
                )
            });
            if invalid_for_operation {
                diagnostics.push(diagnostic(
                    "state_unavailable",
                    "document state is unavailable for one or more rule operations",
                    source,
                    span.start,
                    span.length,
                )?);
            }
            let field = path[1..].join(".");
            match schema.field_type(&field) {
                Some(value_type) => Ok(value_type),
                None => {
                    diagnostics.push(diagnostic(
                        "unknown_field",
                        "document field is absent from the collection schema",
                        source,
                        span.start,
                        span.length,
                    )?);
                    Ok(ValueType::Dynamic)
                }
            }
        }
        _ => {
            diagnostics.push(diagnostic(
                "unknown_identifier",
                "identifier is not available in the policy environment",
                source,
                span.start,
                span.length,
            )?);
            Ok(ValueType::Dynamic)
        }
    }
}

fn require_boolean(
    value_type: ValueType,
    span: ByteSpan,
    source: &str,
    diagnostics: &mut Vec<PolicyDiagnostic>,
) -> Result<(), PolicyModelError> {
    if !matches!(value_type, ValueType::Bool | ValueType::Dynamic) {
        diagnostics.push(diagnostic(
            "boolean_required",
            "boolean operator requires boolean operands",
            source,
            span.start,
            span.length,
        )?);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Identifier(String),
    Literal(Value),
    Dot,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Not,
    And,
    Or,
    Compare(Comparison),
    End,
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: TokenKind,
    span: ByteSpan,
}

fn lex(source: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        let start = offset;
        let kind = match bytes[offset] {
            b'.' => {
                offset += 1;
                TokenKind::Dot
            }
            b'(' => {
                offset += 1;
                TokenKind::LeftParen
            }
            b')' => {
                offset += 1;
                TokenKind::RightParen
            }
            b'[' => {
                offset += 1;
                TokenKind::LeftBracket
            }
            b']' => {
                offset += 1;
                TokenKind::RightBracket
            }
            b'&' if bytes.get(offset + 1) == Some(&b'&') => {
                offset += 2;
                TokenKind::And
            }
            b'|' if bytes.get(offset + 1) == Some(&b'|') => {
                offset += 2;
                TokenKind::Or
            }
            b'!' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                TokenKind::Compare(Comparison::NotEqual)
            }
            b'!' => {
                offset += 1;
                TokenKind::Not
            }
            b'=' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                TokenKind::Compare(Comparison::Equal)
            }
            b'<' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                TokenKind::Compare(Comparison::LessOrEqual)
            }
            b'>' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                TokenKind::Compare(Comparison::GreaterOrEqual)
            }
            b'<' => {
                offset += 1;
                TokenKind::Compare(Comparison::Less)
            }
            b'>' => {
                offset += 1;
                TokenKind::Compare(Comparison::Greater)
            }
            b'"' => {
                offset += 1;
                let mut escaped = false;
                while offset < bytes.len() {
                    match bytes[offset] {
                        b'"' if !escaped => {
                            offset += 1;
                            break;
                        }
                        b'\\' if !escaped => escaped = true,
                        _ => escaped = false,
                    }
                    offset += 1;
                }
                if bytes.get(offset.saturating_sub(1)) != Some(&b'"') {
                    return Err(ParseError::new("unterminated string literal", start, 1));
                }
                let value =
                    serde_json::from_str::<Value>(&source[start..offset]).map_err(|_| {
                        ParseError::new("invalid string literal", start, offset - start)
                    })?;
                TokenKind::Literal(value)
            }
            byte if byte.is_ascii_digit() || byte == b'-' => {
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset].is_ascii_digit()
                        || matches!(bytes[offset], b'.' | b'e' | b'E' | b'+' | b'-'))
                {
                    offset += 1;
                }
                let value =
                    serde_json::from_str::<Value>(&source[start..offset]).map_err(|_| {
                        ParseError::new("invalid number literal", start, offset - start)
                    })?;
                if !value.is_number() {
                    return Err(ParseError::new(
                        "invalid number literal",
                        start,
                        offset - start,
                    ));
                }
                TokenKind::Literal(value)
            }
            byte if byte == b'_' || byte.is_ascii_alphabetic() => {
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset] == b'_' || bytes[offset].is_ascii_alphanumeric())
                {
                    offset += 1;
                }
                match &source[start..offset] {
                    "true" => TokenKind::Literal(Value::Bool(true)),
                    "false" => TokenKind::Literal(Value::Bool(false)),
                    "null" => TokenKind::Literal(Value::Null),
                    identifier => TokenKind::Identifier(identifier.to_owned()),
                }
            }
            _ => return Err(ParseError::new("unsupported policy token", start, 1)),
        };
        tokens.push(Token {
            kind,
            span: ByteSpan {
                start,
                length: offset - start,
            },
        });
    }
    tokens.push(Token {
        kind: TokenKind::End,
        span: ByteSpan {
            start: source.len(),
            length: 1,
        },
    });
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    fn parse(mut self) -> Result<Expression, ParseError> {
        let expression = self.parse_or()?;
        if !matches!(self.peek().kind, TokenKind::End) {
            return Err(ParseError::at(
                "unexpected token after expression",
                self.peek(),
            ));
        }
        Ok(expression)
    }

    fn parse_or(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.parse_and()?;
        while matches!(self.peek().kind, TokenKind::Or) {
            self.advance();
            let right = self.parse_and()?;
            let span = joined_span(expression.span(), right.span());
            expression = Expression::Or(Box::new(expression), Box::new(right), span);
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.parse_comparison()?;
        while matches!(self.peek().kind, TokenKind::And) {
            self.advance();
            let right = self.parse_comparison()?;
            let span = joined_span(expression.span(), right.span());
            expression = Expression::And(Box::new(expression), Box::new(right), span);
        }
        Ok(expression)
    }

    fn parse_comparison(&mut self) -> Result<Expression, ParseError> {
        let left = self.parse_unary()?;
        let TokenKind::Compare(comparison) = self.peek().kind else {
            return Ok(left);
        };
        self.advance();
        let right = self.parse_unary()?;
        let span = joined_span(left.span(), right.span());
        Ok(Expression::Compare(
            comparison,
            Box::new(left),
            Box::new(right),
            span,
        ))
    }

    fn parse_unary(&mut self) -> Result<Expression, ParseError> {
        if matches!(self.peek().kind, TokenKind::Not) {
            let start = self.advance().span;
            let expression = self.parse_unary()?;
            let span = joined_span(start, expression.span());
            return Ok(Expression::Not(Box::new(expression), span));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expression, ParseError> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Literal(value) => Ok(Expression::Literal(value, token.span)),
            TokenKind::Identifier(identifier) => {
                let mut path = vec![identifier];
                let mut span = token.span;
                while matches!(self.peek().kind, TokenKind::Dot) {
                    self.advance();
                    let segment = self.advance().clone();
                    let TokenKind::Identifier(identifier) = segment.kind else {
                        return Err(ParseError::at("expected identifier after dot", &segment));
                    };
                    path.push(identifier);
                    span = joined_span(span, segment.span);
                }
                let mut expression = Expression::Path(path, span);
                while matches!(self.peek().kind, TokenKind::LeftBracket) {
                    self.advance();
                    let index = self.parse_or()?;
                    if !matches!(self.peek().kind, TokenKind::RightBracket) {
                        return Err(ParseError::at("expected closing bracket", self.peek()));
                    }
                    let closing = self.advance().span;
                    let span = joined_span(expression.span(), closing);
                    expression = Expression::Index(Box::new(expression), Box::new(index), span);
                }
                Ok(expression)
            }
            TokenKind::LeftParen => {
                let expression = self.parse_or()?;
                if !matches!(self.peek().kind, TokenKind::RightParen) {
                    return Err(ParseError::at("expected closing parenthesis", self.peek()));
                }
                self.advance();
                Ok(expression)
            }
            _ => Err(ParseError::at("expected policy expression", &token)),
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn advance(&mut self) -> &Token {
        let token = &self.tokens[self.current];
        if !matches!(token.kind, TokenKind::End) {
            self.current += 1;
        }
        token
    }
}

fn joined_span(left: ByteSpan, right: ByteSpan) -> ByteSpan {
    ByteSpan {
        start: left.start,
        length: right.start + right.length - left.start,
    }
}

#[derive(Debug)]
struct ParseError {
    message: &'static str,
    start: usize,
    length: usize,
}

impl ParseError {
    const fn new(message: &'static str, start: usize, length: usize) -> Self {
        Self {
            message,
            start,
            length,
        }
    }

    fn at(message: &'static str, token: &Token) -> Self {
        Self::new(message, token.span.start, token.span.length)
    }

    fn into_diagnostic(self, source: &str) -> Result<PolicyDiagnostic, PolicyModelError> {
        diagnostic(
            "syntax_error",
            self.message,
            source,
            self.start,
            self.length,
        )
    }
}

fn diagnostic(
    code: &str,
    message: &str,
    source: &str,
    start: usize,
    length: usize,
) -> Result<PolicyDiagnostic, PolicyModelError> {
    let before = &source[..start.min(source.len())];
    let line =
        u32::try_from(before.bytes().filter(|byte| *byte == b'\n').count() + 1).unwrap_or(u32::MAX);
    let column_start = before.rfind('\n').map_or(0, |offset| offset + 1);
    let column = u32::try_from(before[column_start..].chars().count() + 1).unwrap_or(u32::MAX);
    PolicyDiagnostic::new(
        DiagnosticSeverity::Error,
        code,
        message,
        Some(SourceSpan::new(
            line,
            column,
            u32::try_from(length.max(1)).unwrap_or(u32::MAX),
        )?),
    )
}

#[derive(Debug)]
pub enum PolicyCompileError {
    InvalidSchema(&'static str),
    Model(PolicyModelError),
}

impl fmt::Display for PolicyCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchema(reason) => write!(formatter, "invalid collection schema: {reason}"),
            Self::Model(error) => error.fmt(formatter),
        }
    }
}

impl Error for PolicyCompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Model(error) => Some(error),
            Self::InvalidSchema(_) => None,
        }
    }
}

impl From<PolicyModelError> for PolicyCompileError {
    fn from(error: PolicyModelError) -> Self {
        Self::Model(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use serde_json::json;

    use super::*;
    use crate::{PolicyEffect, PolicyRule, PolicyRuleId, PolicyState};

    #[test]
    fn compiles_typed_bounded_expressions_without_runtime_capabilities() {
        let policy = policy(
            "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
            [DocumentOperation::Update],
        );
        let compilation = PolicyCompiler::default()
            .compile(&policy, &schema())
            .expect("compile");
        assert!(compilation.diagnostics().is_empty());
        assert_eq!(compilation.compiled().expect("compiled").rule_count(), 1);
    }

    #[test]
    fn unknown_fields_calls_and_cost_excess_return_source_diagnostics() {
        let unknown = PolicyCompiler::default()
            .compile(
                &policy("new.secret == true", [DocumentOperation::Create]),
                &schema(),
            )
            .expect("compile");
        assert_eq!(unknown.diagnostics()[0].code(), "unknown_field");
        assert!(unknown.compiled().is_none());

        let call = PolicyCompiler::default()
            .compile(
                &policy(
                    "fetch(\"https://example.com\") == true",
                    [DocumentOperation::Read],
                ),
                &schema(),
            )
            .expect("compile");
        assert_eq!(call.diagnostics()[0].code(), "syntax_error");

        let limited = PolicyCompiler::new(PolicyCompilerLimits {
            maximum_source_bytes: 1_024,
            maximum_ast_nodes: 2,
        })
        .compile(
            &policy("true && true", [DocumentOperation::Read]),
            &schema(),
        )
        .expect("compile");
        assert_eq!(limited.diagnostics()[0].code(), "cost_limit_exceeded");
    }

    #[test]
    fn index_expressions_parse_as_postfix_lookups_with_precise_spans() {
        let source = "claims.a[new.slot][new.household_id]";
        let expression = Parser::new(lex(source).expect("lex"))
            .parse()
            .expect("parse");
        let path = |segments: &[&str], start, length| {
            Box::new(Expression::Path(
                segments
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect(),
                ByteSpan { start, length },
            ))
        };
        assert_eq!(
            expression,
            Expression::Index(
                Box::new(Expression::Index(
                    path(&["claims", "a"], 0, 8),
                    path(&["new", "slot"], 9, 8),
                    ByteSpan {
                        start: 0,
                        length: 18
                    },
                )),
                path(&["new", "household_id"], 19, 16),
                ByteSpan {
                    start: 0,
                    length: 36
                },
            )
        );
    }

    #[test]
    fn indexed_claims_compile_for_every_operation_form_and_count_toward_the_cost_limit() {
        let compiler = PolicyCompiler::default();
        for (expression, operations) in [
            (
                "claims.households[new.household_id] == \"editor\"",
                vec![DocumentOperation::Create, DocumentOperation::Update],
            ),
            (
                "claims.households[old.household_id] != null",
                vec![DocumentOperation::Read, DocumentOperation::Delete],
            ),
            (
                "(claims.households[new.household_id] == \"owner\" || claims.households[new.household_id] == \"editor\")",
                vec![DocumentOperation::Create],
            ),
            (
                "claims.a[new.household_id][new.slot] == 1",
                vec![DocumentOperation::Create],
            ),
            (
                "claims.team_ids[0] == \"blue\"",
                vec![DocumentOperation::Read],
            ),
            ("claims.team_ids[-1] == null", vec![DocumentOperation::Read]),
            (
                "claims.a[request.method] == true",
                vec![DocumentOperation::Read],
            ),
            (
                "claims.a[identity.user_id] == true",
                vec![DocumentOperation::Read],
            ),
            ("claims.a[claims.b] == true", vec![DocumentOperation::Read]),
            (
                "!(claims.a[new.slot] == null) && claims.a[new.slot] < 3",
                vec![DocumentOperation::Create],
            ),
        ] {
            let compilation = compiler
                .compile(&policy(expression, operations), &schema())
                .expect("compile");
            assert!(
                compilation.diagnostics().is_empty(),
                "{expression}: {:?}",
                compilation.diagnostics()
            );
            assert_eq!(compilation.compiled().expect("compiled").rule_count(), 1);
        }

        // Compare + Index + two paths + literal: the index node itself counts.
        let with_limit = |maximum_ast_nodes| {
            PolicyCompiler::new(PolicyCompilerLimits {
                maximum_source_bytes: 1_024,
                maximum_ast_nodes,
            })
            .compile(
                &policy(
                    "claims.households[new.household_id] == \"editor\"",
                    [DocumentOperation::Create],
                ),
                &schema(),
            )
            .expect("compile")
        };
        assert_eq!(with_limit(4).diagnostics()[0].code(), "cost_limit_exceeded");
        assert!(with_limit(4).compiled().is_none());
        assert!(with_limit(5).compiled().is_some());
    }

    #[test]
    fn index_diagnostics_are_addressed_to_the_base_the_index_or_the_missing_bracket() {
        let compiler = PolicyCompiler::default();
        let not_allowed = "only trusted claims may be indexed by a value";
        let invalid_index = "index expression must be a string or a number";
        let cases = [
            (
                "new.owner_id[new.household_id] == \"x\"",
                DocumentOperation::Create,
                "index_not_allowed",
                not_allowed,
                (1, 1, 12),
            ),
            (
                "identity.role[new.household_id] == \"x\"",
                DocumentOperation::Create,
                "index_not_allowed",
                not_allowed,
                (1, 1, 13),
            ),
            (
                "request.method[new.household_id] == \"x\"",
                DocumentOperation::Create,
                "index_not_allowed",
                not_allowed,
                (1, 1, 14),
            ),
            (
                "old.owner_id[\"k\"] == \"x\"",
                DocumentOperation::Read,
                "index_not_allowed",
                not_allowed,
                (1, 1, 12),
            ),
            (
                "new.owner_id[\"a\"][\"b\"] == \"x\"",
                DocumentOperation::Create,
                "index_not_allowed",
                not_allowed,
                (1, 1, 12),
            ),
            (
                "claims.households[true] == \"x\"",
                DocumentOperation::Create,
                "index_type_invalid",
                invalid_index,
                (1, 19, 4),
            ),
            (
                "claims.households[null] == \"x\"",
                DocumentOperation::Create,
                "index_type_invalid",
                invalid_index,
                (1, 19, 4),
            ),
            (
                "claims.households[new.blocked] == \"x\"",
                DocumentOperation::Create,
                "index_type_invalid",
                invalid_index,
                (1, 19, 11),
            ),
            (
                "claims.households[new.household_id == \"x\"] == \"x\"",
                DocumentOperation::Create,
                "index_type_invalid",
                invalid_index,
                (1, 19, 23),
            ),
            (
                "claims.households[new.household_id",
                DocumentOperation::Create,
                "syntax_error",
                "expected closing bracket",
                (1, 35, 1),
            ),
            (
                "claims.households[new.household_id == \"editor\"",
                DocumentOperation::Create,
                "syntax_error",
                "expected closing bracket",
                (1, 47, 1),
            ),
            (
                "claims.households[]",
                DocumentOperation::Create,
                "syntax_error",
                "expected policy expression",
                (1, 19, 1),
            ),
            (
                "true[0] == true",
                DocumentOperation::Create,
                "syntax_error",
                "unexpected token after expression",
                (1, 5, 1),
            ),
            (
                "(claims.a)[0] == true",
                DocumentOperation::Create,
                "syntax_error",
                "unexpected token after expression",
                (1, 11, 1),
            ),
            (
                "claims.a[0].b == true",
                DocumentOperation::Create,
                "syntax_error",
                "unexpected token after expression",
                (1, 12, 1),
            ),
        ];
        for (expression, operation, code, message, (line, column, length)) in cases {
            let compilation = compiler
                .compile(&policy(expression, [operation]), &schema())
                .expect("compile");
            assert!(compilation.compiled().is_none(), "{expression}");
            assert_eq!(compilation.diagnostics().len(), 1, "{expression}");
            let diagnostic = &compilation.diagnostics()[0];
            assert_eq!(diagnostic.code(), code, "{expression}");
            assert_eq!(diagnostic.message(), message, "{expression}");
            let span = diagnostic.span().expect("span");
            assert_eq!(
                (span.line(), span.column(), span.length()),
                (line, column, length),
                "{expression}"
            );
        }

        // A bare `claims` root is not a claim, so it is unknown and unindexable.
        let bare = compiler
            .compile(
                &policy(
                    "claims[new.household_id] == \"x\"",
                    [DocumentOperation::Create],
                ),
                &schema(),
            )
            .expect("compile");
        let codes: Vec<_> = bare
            .diagnostics()
            .iter()
            .map(PolicyDiagnostic::code)
            .collect();
        assert_eq!(codes, ["unknown_identifier", "index_not_allowed"]);
    }

    fn policy(
        expression: &str,
        operations: impl IntoIterator<Item = DocumentOperation>,
    ) -> PolicySet {
        PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Draft,
            [PolicyRule::new(
                PolicyRuleId::parse("rule").expect("id"),
                PolicyEffect::Allow,
                operations,
                expression,
            )
            .expect("rule")],
            [],
        )
        .expect("policy")
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "owner_id": {"type": "string"},
                "blocked": {"type": "boolean"},
                "household_id": {"type": "string"},
                "slot": {"type": "integer"}
            }
        })
    }

    fn scope() -> CollectionScope {
        CollectionScope::new(
            TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            CollectionId::parse("todos").expect("collection"),
        )
    }
}
