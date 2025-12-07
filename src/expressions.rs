use std::fmt::{self, Write};

use crate::args::ArgExprs;
use crate::callable::Callable;
use crate::function::Function;
use crate::operators::{CmpOperator, Operator};
use crate::parse::CodeRange;
use crate::value::{Attr, Value};
use crate::values::bytes::bytes_repr;
use crate::values::str::string_repr;

/// Indicates which namespace a variable reference belongs to.
///
/// This is determined at prepare time based on Python's scoping rules:
/// - Variables assigned in a function are Local (unless declared `global`)
/// - Variables only read (not assigned) that exist at module level are Global
/// - The `global` keyword explicitly marks a variable as Global
///
/// # Future: `nonlocal` Support
///
/// To support `nonlocal`, add an `Enclosing(usize)` variant where the usize is the
/// namespace index of the enclosing function. The prepare phase would need to track
/// enclosing function namespace indices when resolving nonlocal declarations.
///
/// TODO: Add Enclosing(usize) variant for nonlocal support
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum NameScope {
    /// Variable is in the current frame's local namespace
    #[default]
    Local,
    /// Variable is in the module-level global namespace
    Global,
    // TODO: Enclosing(usize) for nonlocal support - index into Namespaces
}

#[derive(Debug, Clone)]
pub(crate) struct Identifier<'c> {
    pub position: CodeRange<'c>,
    pub name: &'c str,
    opt_heap_id: Option<usize>,
    /// Which namespace this identifier refers to (determined at prepare time)
    pub scope: NameScope,
}

impl<'c> Identifier<'c> {
    /// Creates a new identifier with unknown scope (to be resolved during prepare phase).
    pub fn new(name: &'c str, position: CodeRange<'c>) -> Self {
        Self {
            name,
            position,
            opt_heap_id: None,
            scope: NameScope::Local,
        }
    }

    /// Creates a new identifier with resolved namespace index and explicit scope.
    pub fn new_with_scope(name: &'c str, position: CodeRange<'c>, heap_id: usize, scope: NameScope) -> Self {
        Self {
            name,
            position,
            opt_heap_id: Some(heap_id),
            scope,
        }
    }

    pub fn heap_id(&self) -> usize {
        self.opt_heap_id.expect("Identifier not prepared with heap_id")
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Expr<'c> {
    Literal(Literal),
    Callable(Callable<'c>),
    Name(Identifier<'c>),
    /// Function call expression.
    ///
    /// The `callable` can be a Builtin, ExcType (resolved at parse time), or a Name
    /// that will be looked up in the namespace at runtime.
    Call {
        callable: Callable<'c>,
        args: ArgExprs<'c>,
    },
    AttrCall {
        object: Identifier<'c>,
        attr: Attr,
        args: ArgExprs<'c>,
    },
    Op {
        left: Box<ExprLoc<'c>>,
        op: Operator,
        right: Box<ExprLoc<'c>>,
    },
    CmpOp {
        left: Box<ExprLoc<'c>>,
        op: CmpOperator,
        right: Box<ExprLoc<'c>>,
    },
    List(Vec<ExprLoc<'c>>),
    Tuple(Vec<ExprLoc<'c>>),
    Subscript {
        object: Box<ExprLoc<'c>>,
        index: Box<ExprLoc<'c>>,
    },
    Dict(Vec<(ExprLoc<'c>, ExprLoc<'c>)>),
    /// Unary `not` expression - evaluates to the boolean negation of the operand's truthiness.
    Not(Box<ExprLoc<'c>>),
}

impl fmt::Display for Expr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(literal) => write!(f, "{literal}"),
            Self::Callable(callable) => write!(f, "{callable}"),
            Self::Name(identifier) => f.write_str(identifier.name),
            Self::Call { callable, args } => write!(f, "{callable}{args}"),
            Self::AttrCall { object, attr, args } => write!(f, "{}.{}{}", object.name, attr, args),
            Self::Op { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::CmpOp { left, op, right } => write!(f, "{left} {op} {right}"),
            Self::List(itms) => {
                write!(
                    f,
                    "[{}]",
                    itms.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                )
            }
            Self::Tuple(itms) => {
                write!(
                    f,
                    "({})",
                    itms.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                )
            }
            Self::Subscript { object, index } => write!(f, "{object}[{index}]"),
            Self::Dict(pairs) => {
                if pairs.is_empty() {
                    f.write_str("{}")
                } else {
                    f.write_char('{')?;
                    let mut iter = pairs.iter();
                    if let Some((k, v)) = iter.next() {
                        write!(f, "{k}: {v}")?;
                    }
                    for (k, v) in iter {
                        write!(f, ", {k}: {v}")?;
                    }
                    f.write_char('}')
                }
            }
            Self::Not(operand) => write!(f, "not {operand}"),
        }
    }
}

impl Expr<'_> {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::Literal(Literal::None))
    }
}

/// Represents values that can be produced purely from the parser/prepare pipeline.
///
/// Const values are intentionally detached from the runtime heap so we can keep
/// parse-time transformations (constant folding, namespace seeding, etc.) free from
/// reference-count semantics. Only once execution begins are these literals turned
/// into real `Value`s that participate in the interpreter's runtime rules.
///
/// Note: unlike the AST `Constant` type, we store tuples only as expressions since they
/// can't always be recorded as constants.
#[derive(Debug, Clone)]
pub enum Literal {
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
}

impl Literal {
    /// Converts the literal into its runtime `Value` counterpart.
    ///
    /// This is the only place parse-time data crosses the boundary into runtime
    /// semantics, ensuring every literal follows the same conversion path.
    pub fn to_value<'c>(&self) -> Value<'c, '_> {
        match self {
            Self::Ellipsis => Value::Ellipsis,
            Self::None => Value::None,
            Self::Bool(b) => Value::Bool(*b),
            Self::Int(v) => Value::Int(*v),
            Self::Float(v) => Value::Float(*v),
            Self::Str(s) => Value::InternString(s),
            Self::Bytes(b) => Value::InternBytes(b),
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ellipsis => f.write_str("..."),
            Self::None => f.write_str("None"),
            Self::Bool(true) => f.write_str("True"),
            Self::Bool(false) => f.write_str("False"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => f.write_str(&string_repr(v)),
            Self::Bytes(v) => f.write_str(&bytes_repr(v)),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExprLoc<'c> {
    pub position: CodeRange<'c>,
    pub expr: Expr<'c>,
}

impl fmt::Display for ExprLoc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // don't show position as that should be displayed separately
        write!(f, "{}", self.expr)
    }
}

impl<'c> ExprLoc<'c> {
    pub fn new(position: CodeRange<'c>, expr: Expr<'c>) -> Self {
        Self { position, expr }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Node<'c> {
    Expr(ExprLoc<'c>),
    Return(ExprLoc<'c>),
    ReturnNone,
    Raise(Option<ExprLoc<'c>>),
    Assert {
        test: ExprLoc<'c>,
        msg: Option<ExprLoc<'c>>,
    },
    Assign {
        target: Identifier<'c>,
        object: ExprLoc<'c>,
    },
    OpAssign {
        target: Identifier<'c>,
        op: Operator,
        object: ExprLoc<'c>,
    },
    SubscriptAssign {
        target: Identifier<'c>,
        index: ExprLoc<'c>,
        value: ExprLoc<'c>,
    },
    For {
        target: Identifier<'c>,
        iter: ExprLoc<'c>,
        body: Vec<Node<'c>>,
        or_else: Vec<Node<'c>>,
    },
    If {
        test: ExprLoc<'c>,
        body: Vec<Node<'c>>,
        or_else: Vec<Node<'c>>,
    },
    FunctionDef(Function<'c>),
}

// TODO move to run
#[derive(Debug)]
pub enum FrameExit<'c, 'e> {
    Return(Value<'c, 'e>),
    // Yield(Value),
}
