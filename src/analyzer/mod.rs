use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::parser;
use crate::parser::ast::{self, Statement};
pub use crate::parser::ast::{BinaryOperator, ConstantNumberType, UnaryOperator};

pub fn analyze(path: &Path) -> Analyzed {
    let mut ctx = Context::new();
    ctx.process_file(path);
    ctx.into()
}

#[derive(Default)]
struct Context {
    namespace: String,
    polynomial_degree: ConstantNumberType,
    /// Constants are not namespaced!
    constants: HashMap<String, ConstantNumberType>,
    declarations: HashMap<String, Polynomial>,
    polynomial_identities: Vec<Expression>,
    plookup_identities: Vec<PlookupIdentity>,
    included_files: HashSet<PathBuf>,
    current_dir: PathBuf,
    commit_poly_counter: u64,
    constant_poly_counter: u64,
    intermediate_poly_counter: u64,
}

pub struct Analyzed {
    /// Constants are not namespaced!
    pub constants: HashMap<String, ConstantNumberType>,
    pub declarations: HashMap<String, Polynomial>,
    pub polynomial_identities: Vec<Expression>,
    pub plookup_identities: Vec<PlookupIdentity>,
}

impl Analyzed {
    /// @returns the number of committed polynomials
    pub fn commitment_count(&self) -> usize {
        self.declarations
            .iter()
            .filter(|(_name, poly)| poly.poly_type == PolynomialType::Committed)
            .count()
    }
    /// @returns the number of intermediate polynomials
    pub fn intermediate_count(&self) -> usize {
        self.declarations
            .iter()
            .filter(|(_name, poly)| poly.poly_type == PolynomialType::Intermediate)
            .count()
    }
    /// @returns the number of constant polynomials
    pub fn constant_count(&self) -> usize {
        self.declarations
            .iter()
            .filter(|(_name, poly)| poly.poly_type == PolynomialType::Constant)
            .count()
    }
}

impl From<Context> for Analyzed {
    fn from(
        Context {
            constants,
            declarations,
            polynomial_identities,
            plookup_identities,
            ..
        }: Context,
    ) -> Self {
        Self {
            constants,
            declarations,
            polynomial_identities,
            plookup_identities,
        }
    }
}

pub struct Polynomial {
    pub id: u64,
    pub absolute_name: String,
    pub poly_type: PolynomialType,
    pub degree: ConstantNumberType,
    pub length: Option<ConstantNumberType>,
}

impl Polynomial {
    pub fn is_array(&self) -> bool {
        self.length.is_some()
    }
}

pub struct PlookupIdentity {
    pub key: SelectedExpressions,
    pub haystack: SelectedExpressions,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SelectedExpressions {
    pub selector: Option<Expression>,
    pub expressions: Vec<Expression>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Expression {
    Constant(String),
    PolynomialReference(PolynomialReference),
    Number(ConstantNumberType),
    BinaryOperation(Box<Expression>, BinaryOperator, Box<Expression>),
    UnaryOperation(UnaryOperator, Box<Expression>),
}

#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct PolynomialReference {
    // TODO would be better to use numeric IDs instead of names,
    // but the IDs as they are overlap. Maybe we can change that.
    pub name: String,
    pub index: Option<u64>,
    pub next: bool,
}

#[derive(Copy, Clone, PartialEq)]
pub enum PolynomialType {
    Committed,
    Constant,
    Intermediate,
}

impl Context {
    pub fn new() -> Context {
        Context {
            namespace: "Global".to_string(),
            ..Default::default()
        }
    }

    pub fn process_file(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap();
        if self.included_files.contains(&path) {
            return;
        }
        let contents = fs::read_to_string(path.clone()).unwrap();
        let pil_file = parser::parse(&contents).unwrap();
        let old_current_dir = self.current_dir.clone();
        self.current_dir = path.parent().unwrap().to_path_buf();

        for statement in &pil_file.0 {
            match statement {
                Statement::Include(include) => self.handle_include(include),
                Statement::Namespace(name, degree) => self.handle_namespace(name, degree),
                Statement::PolynomialDefinition(_, _) => todo!(),
                Statement::PolynomialConstantDeclaration(polynomials) => {
                    self.handle_polynomial_declaration(polynomials, PolynomialType::Constant)
                }
                Statement::PolynomialCommitDeclaration(polynomials) => {
                    self.handle_polynomial_declaration(polynomials, PolynomialType::Committed)
                }
                Statement::PolynomialIdentity(expression) => {
                    self.handle_polynomial_identity(expression)
                }
                Statement::PlookupIdentity(key, haystack) => {
                    self.handle_plookup_identity(key, haystack)
                }
                Statement::ConstantDefinition(name, value) => {
                    self.handle_constant_definition(name, value)
                }
            }
        }

        self.current_dir = old_current_dir;
    }

    fn handle_include(&mut self, path: &str) {
        let mut dir = self.current_dir.clone();
        dir.push(path);
        self.process_file(&dir);
    }

    fn handle_namespace(&mut self, name: &str, degree: &ast::Expression) {
        self.polynomial_degree = self.evaluate_expression(degree).unwrap();
        self.namespace = name.to_owned();
    }

    fn handle_polynomial_declaration(
        &mut self,
        polynomials: &Vec<ast::PolynomialName>,
        polynomial_type: PolynomialType,
    ) {
        for ast::PolynomialName { name, array_size } in polynomials {
            let counter = match polynomial_type {
                PolynomialType::Committed => &mut self.commit_poly_counter,
                PolynomialType::Constant => &mut self.constant_poly_counter,
                PolynomialType::Intermediate => &mut self.intermediate_poly_counter,
            };
            let id = *counter;
            *counter += 1;
            let poly = Polynomial {
                id,
                absolute_name: self.namespaced(name),
                degree: self.polynomial_degree,
                poly_type: polynomial_type,
                length: array_size
                    .as_ref()
                    .map(|l| self.evaluate_expression(l).unwrap()),
            };
            let name = poly.absolute_name.clone();
            let is_new = self.declarations.insert(name, poly).is_none();
            assert!(is_new);
        }
    }

    fn handle_polynomial_identity(&mut self, expression: &ast::Expression) {
        let expr = self.process_expression(expression);
        self.polynomial_identities.push(expr);
    }

    fn handle_plookup_identity(
        &mut self,
        key: &ast::SelectedExpressions,
        haystack: &ast::SelectedExpressions,
    ) {
        let key = self.process_selected_expression(key);
        let haystack = self.process_selected_expression(haystack);
        self.plookup_identities
            .push(PlookupIdentity { key, haystack })
    }

    fn handle_constant_definition(&mut self, name: &str, value: &ast::Expression) {
        let is_new = self
            .constants
            .insert(name.to_string(), self.evaluate_expression(value).unwrap())
            .is_none();
        assert!(is_new);
    }

    fn namespaced(&self, name: &String) -> String {
        self.namespaced_ref(&None, name)
    }

    fn namespaced_ref(&self, namespace: &Option<String>, name: &String) -> String {
        format!("{}.{name}", namespace.as_ref().unwrap_or(&self.namespace))
    }

    fn process_selected_expression(&self, expr: &ast::SelectedExpressions) -> SelectedExpressions {
        SelectedExpressions {
            selector: expr.selector.as_ref().map(|e| self.process_expression(e)),
            expressions: expr
                .expressions
                .iter()
                .map(|e| self.process_expression(e))
                .collect(),
        }
    }

    fn process_expression(&self, expr: &ast::Expression) -> Expression {
        match expr {
            ast::Expression::Constant(name) => Expression::Constant(name.clone()),
            ast::Expression::PolynomialReference(poly) => {
                let index = poly
                    .index
                    .as_ref()
                    .map(|i| self.evaluate_expression(i).unwrap() as u64);
                Expression::PolynomialReference(PolynomialReference {
                    name: self.namespaced_ref(&poly.namespace, &poly.name),
                    index,
                    next: poly.next,
                })
            }
            ast::Expression::Number(n) => Expression::Number(*n),
            ast::Expression::BinaryOperation(left, op, right) => {
                if let Some(value) = self.evaluate_binary_operation(left, op, right) {
                    Expression::Number(value)
                } else {
                    Expression::BinaryOperation(
                        Box::new(self.process_expression(left)),
                        *op,
                        Box::new(self.process_expression(right)),
                    )
                }
            }
            ast::Expression::UnaryOperation(_, _) => todo!(),
        }
    }

    fn evaluate_expression(&self, expr: &ast::Expression) -> Option<ConstantNumberType> {
        match expr {
            ast::Expression::Constant(name) => Some(self.constants[name]),
            ast::Expression::PolynomialReference(_) => None,
            ast::Expression::Number(n) => Some(*n),
            ast::Expression::BinaryOperation(left, op, right) => {
                self.evaluate_binary_operation(left, op, right)
            }
            ast::Expression::UnaryOperation(_, _) => todo!(),
        }
    }

    fn evaluate_binary_operation(
        &self,
        left: &ast::Expression,
        op: &BinaryOperator,
        right: &ast::Expression,
    ) -> Option<ConstantNumberType> {
        // TODO handle owerflow and maybe use bigint instead.
        if let (Some(left), Some(right)) = (
            self.evaluate_expression(left),
            self.evaluate_expression(right),
        ) {
            Some(match op {
                BinaryOperator::Add => left + right,
                BinaryOperator::Sub => left - right,
                BinaryOperator::Mul => left * right,
                BinaryOperator::Div => left / right,
                BinaryOperator::Pow => {
                    assert!(right <= u32::MAX.into());
                    left.pow(right as u32)
                }
            })
        } else {
            None
        }
    }
}
