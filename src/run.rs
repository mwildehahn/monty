use std::borrow::Cow;
use crate::evaluate::Evaluator;

use crate::prepare::{RunExpr, RunNode};
use crate::types::{Expr, Node, Operator};
use crate::object::Object;

pub type RunResult<T> = Result<T, Cow<'static, str>>;

#[derive(Debug)]
pub(crate) struct Frame {
    namespace: Vec<Object>,
}

impl Frame {
    pub fn new(namespace: Vec<Object>) -> Self {
        Self {
            namespace,
        }
    }

    pub fn execute(&mut self, nodes: &[RunNode]) -> RunResult<()> {
        for node in nodes {
            self.execute_node(node)?;
        }
        Ok(())
    }

    fn execute_node(&mut self, node: &RunNode) -> RunResult<()> {
        match node {
            Node::Pass => return Err("Unexpected `pass` in execution".into()),
            Node::Expr(expr) => {
                self.execute_expr(expr)?;
            },
            Node::Assign { target, object } => {
                self.assign(*target, object)?;
            },
            Node::OpAssign { target, op, object } => {
                self.op_assign(*target, op, object)?;
            },
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => self.for_loop(target, iter, body, or_else)?,
            Node::If { test, body, or_else } => self.if_(test, body, or_else)?,
        };
        Ok(())
    }

    fn execute_expr<'a>(&'a self, expr: &'a RunExpr) -> RunResult<Cow<Object>> {
        // TODO, does creating this struct harm performance, or is it optimised out?
        Evaluator::new(&self.namespace).evaluate(expr)
    }

    fn execute_expr_bool(&self, expr: &RunExpr) -> RunResult<bool> {
        Evaluator::new(&self.namespace).evaluate_bool(expr)
    }

    fn assign(&mut self, target: usize, object: &RunExpr) -> RunResult<()> {
        self.namespace[target] = match self.execute_expr(object)? {
            Cow::Borrowed(object) => object.clone(),
            Cow::Owned(object) => object,
        };
        Ok(())
    }

    fn op_assign(&mut self, target: usize, op: &Operator, object: &RunExpr) -> RunResult<()> {
        let right_object = self.execute_expr(object)?.into_owned();
        if let Some(target_object) = self.namespace.get_mut(target) {
            let ok = match op {
                Operator::Add => target_object.add_mut(right_object),
                _ => return Err(format!("Assign operator {op:?} not yet implemented").into()),
            };
            match ok {
                true => Ok(()),
                false => Err(format!("Cannot apply assign operator {op:?} {object:?}").into()),
            }
        } else {
            Err(format!("name '{target}' is not defined").into())
        }
    }

    fn for_loop(
        &mut self,
        target: &RunExpr,
        iter: &RunExpr,
        body: &[RunNode],
        _or_else: &[RunNode],
    ) -> RunResult<()> {
        let target_id = match target {
            Expr::Name(id) => *id,
            _ => return Err("For target must be a name".into()),
        };
        let range_size = match self.execute_expr(iter)?.as_ref() {
            Object::Range(s) => *s,
            _ => return Err("For iter must be a range".into()),
        };

        for object in 0i64..range_size {
            self.namespace[target_id] = Object::Int(object);
            self.execute(body)?;
        }
        Ok(())
    }

    fn if_(&mut self, test: &RunExpr, body: &[RunNode], or_else: &[RunNode]) -> RunResult<()> {
        if self.execute_expr_bool(test)? {
            self.execute(body)?;
        } else {
            self.execute(or_else)?;
        }
        Ok(())
    }
}
