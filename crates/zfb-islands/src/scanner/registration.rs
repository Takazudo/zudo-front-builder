//! Binding-aware discovery of concrete SDK island boundary children.
//!
//! This pass is intentionally separate from the module walk in `scanner.rs`:
//! imports and client-module resource facts remain rooted at every reachable
//! directive-bearing module, while this pass returns only validated targets.
//! It resolves static JSX and owned `h`/`jsx`/`jsxs`/`jsxDEV` descriptions,
//! follows aliases and re-export barrels, and summarizes fixed or
//! child-forwarding wrappers. It does not infer arbitrary dynamic evaluation:
//! demanded unresolved or opaque child flows become source-located errors.

use super::*;
use std::rc::Rc;
use swc_core::common::{Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, JSXAttrName, JSXAttrOrSpread, JSXAttrValue, JSXElement, JSXElementChild,
    JSXElementName, JSXExpr, JSXMemberExpr, JSXObject, MemberProp, ObjectLit, Prop, PropName,
    PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Definition {
    module: PathBuf,
    binding: String,
    position: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FunctionValue {
    definition: Definition,
    marker: String,
    route: Option<(PathBuf, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Boundary,
    Factory(FactoryKind),
    Function(FunctionValue),
    Namespace(PathBuf),
    SdkNamespace(String),
    Unsupported(String),
    Other,
}

impl Value {
    fn same_provider(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Function(left), Self::Function(right)) => left.definition == right.definition,
            _ => self == other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FactoryKind {
    H,
    Jsx,
}

// Shared within one discovery pass; binding lookups must not clone the AST.
struct SourceModule {
    ast: Module,
    source: String,
    client: bool,
}

/// Per-module declaration, call, and reference facts. Resolving aliases still
/// uses Discovery's cycle guard. Keep function bodies out of this index so
/// nested declarations do not clone the surrounding AST. SWC IDs distinguish
/// same-spelled lexical bindings.
#[derive(Default)]
struct NestedBindings {
    names: Vec<swc_core::ecma::ast::Ident>,
    functions: HashMap<swc_core::ecma::ast::Id, (String, Span)>,
    variables: HashMap<swc_core::ecma::ast::Id, NestedVariable>,
    factory_members: HashMap<swc_core::ecma::ast::Id, FactoryMember>,
    factory_functions: HashMap<swc_core::ecma::ast::Id, FactoryFunction>,
    calls: HashMap<swc_core::ecma::ast::Id, Vec<CallExpr>>,
    references: HashMap<swc_core::ecma::ast::Id, Vec<Span>>,
    direct_callees: HashSet<u32>,
    allowed_parameter_reads: HashSet<u32>,
    written_parameters: HashMap<BindingId, Vec<Span>>,
    owners: Vec<FactoryOwner>,
    #[cfg(test)]
    expression_visits: usize,
}

struct NestedVariable {
    kind: swc_core::ecma::ast::VarDeclKind,
    init: Option<NestedInitializer>,
}

enum NestedInitializer {
    Function { marker: String, span: Span },
    Alias(swc_core::ecma::ast::Ident),
    FactoryMember,
    Unsupported,
}

type BindingId = swc_core::ecma::ast::Id;

#[derive(Clone)]
struct FactoryOwner {
    binding: swc_core::ecma::ast::Ident,
    params: Vec<(Option<swc_core::ecma::ast::Ident>, bool)>,
}

#[derive(Clone)]
struct FactoryFunction {
    name: String,
}

#[derive(Clone)]
struct FactoryMember {
    factory: BindingId,
    index: usize,
    property: String,
    parameter: Option<swc_core::ecma::ast::Ident>,
    default: bool,
    rest: bool,
    invalid_pattern: bool,
}

fn static_member_name(member: &swc_core::ecma::ast::MemberExpr) -> Option<String> {
    match &member.prop {
        MemberProp::Ident(ident) => Some(ident.sym.to_string()),
        MemberProp::Computed(computed) => match unwrap_expr(&computed.expr) {
            Expr::Lit(Lit::Str(text)) => Some(atom_to_string(&text.value)),
            _ => None,
        },
        _ => None,
    }
}

fn factory_object_key(name: &PropName) -> Option<String> {
    match name {
        PropName::Num(number) => Some(number.value.to_string()),
        PropName::BigInt(number) => Some(number.value.to_string()),
        other => prop_name(other),
    }
}

fn object_pattern_members(
    pattern: &swc_core::ecma::ast::ObjectPat,
) -> Vec<(swc_core::ecma::ast::Ident, String)> {
    pattern
        .props
        .iter()
        .filter_map(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                Some((assign.key.clone().into(), assign.key.sym.to_string()))
            }
            swc_core::ecma::ast::ObjectPatProp::KeyValue(pair) => {
                let binding = match &*pair.value {
                    Pat::Ident(binding) => &binding.id,
                    Pat::Assign(assign) => match &*assign.left {
                        Pat::Ident(binding) => &binding.id,
                        _ => return None,
                    },
                    _ => return None,
                };
                Some((binding.clone(), factory_object_key(&pair.key)?))
            }
            _ => None,
        })
        .collect()
}

fn plain_object_pattern(pattern: &swc_core::ecma::ast::ObjectPat) -> bool {
    pattern.props.iter().all(|prop| match prop {
        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => assign.value.is_none(),
        swc_core::ecma::ast::ObjectPatProp::KeyValue(pair) => {
            !matches!(&pair.key, PropName::Computed(_)) && matches!(&*pair.value, Pat::Ident(_))
        }
        _ => false,
    })
}

impl NestedBindings {
    fn enter_function(&mut self, binding: swc_core::ecma::ast::Ident, params: &[Pat]) {
        let mut entries = Vec::new();
        for (index, pat) in params.iter().enumerate() {
            let (parameter, default) = match pat {
                Pat::Ident(ident) => (Some(ident.id.clone()), false),
                Pat::Assign(assign) => (
                    match &*assign.left {
                        Pat::Ident(ident) => Some(ident.id.clone()),
                        _ => None,
                    },
                    true,
                ),
                _ => (None, false),
            };
            let object = match pat {
                Pat::Object(object) => Some((object, false)),
                Pat::Assign(assign) => match &*assign.left {
                    Pat::Object(object) => Some((object, true)),
                    _ => None,
                },
                _ => None,
            };
            if let Some((object, has_default)) = object {
                let has_rest = object
                    .props
                    .iter()
                    .any(|prop| matches!(prop, swc_core::ecma::ast::ObjectPatProp::Rest(_)));
                for (target, property) in object_pattern_members(object) {
                    self.factory_members.insert(
                        target.to_id(),
                        FactoryMember {
                            factory: binding.to_id(),
                            index,
                            property,
                            parameter: None,
                            default: has_default,
                            rest: has_rest,
                            invalid_pattern: !plain_object_pattern(object) && !has_rest,
                        },
                    );
                }
            }
            entries.push((parameter, default));
        }
        self.factory_functions.insert(
            binding.to_id(),
            FactoryFunction {
                name: binding.sym.to_string(),
            },
        );
        self.owners.push(FactoryOwner {
            binding,
            params: entries,
        });
    }

    fn member_from_expr(&self, expr: &Expr) -> Option<FactoryMember> {
        let Expr::Member(member) = unwrap_expr(expr) else {
            return None;
        };
        let Expr::Ident(parameter) = unwrap_expr(&member.obj) else {
            return None;
        };
        let property = static_member_name(member)?;
        for owner in self.owners.iter().rev() {
            for (index, (candidate, default)) in owner.params.iter().enumerate() {
                if candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.to_id() == parameter.to_id())
                {
                    return Some(FactoryMember {
                        factory: owner.binding.to_id(),
                        index,
                        property,
                        parameter: Some(parameter.clone()),
                        default: *default,
                        rest: false,
                        invalid_pattern: false,
                    });
                }
            }
        }
        None
    }

    fn record_reference(&mut self, ident: &swc_core::ecma::ast::Ident) {
        self.references
            .entry(ident.to_id())
            .or_default()
            .push(ident.span);
    }

    fn record_parameter_write(&mut self, expr: &Expr) {
        let object = match unwrap_expr(expr) {
            Expr::Member(member) => unwrap_expr(&member.obj),
            other => other,
        };
        if let Expr::Ident(ident) = object {
            self.written_parameters
                .entry(ident.to_id())
                .or_default()
                .push(ident.span);
        }
    }
}

impl Visit for NestedBindings {
    fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
        self.names.push(node.ident.clone());
        self.functions.insert(
            node.ident.to_id(),
            (node.ident.sym.to_string(), node.function.span),
        );
        let params: Vec<_> = node
            .function
            .params
            .iter()
            .map(|param| param.pat.clone())
            .collect();
        self.enter_function(node.ident.clone(), &params);
        node.function.visit_children_with(self);
        self.owners.pop();
    }

    fn visit_var_decl(&mut self, node: &swc_core::ecma::ast::VarDecl) {
        for declaration in &node.decls {
            if let Pat::Ident(binding) = &declaration.name {
                self.names.push(binding.id.clone());
                let init = declaration
                    .init
                    .as_deref()
                    .map(|init| match unwrap_expr(init) {
                        Expr::Fn(function) => NestedInitializer::Function {
                            marker: function
                                .ident
                                .as_ref()
                                .map(|name| name.sym.to_string())
                                .unwrap_or_else(|| binding.id.sym.to_string()),
                            span: function.function.span,
                        },
                        Expr::Arrow(arrow) => NestedInitializer::Function {
                            marker: binding.id.sym.to_string(),
                            span: arrow.span,
                        },
                        Expr::Ident(alias) => NestedInitializer::Alias(alias.clone()),
                        other if self.member_from_expr(other).is_some() => {
                            self.factory_members
                                .insert(binding.id.to_id(), self.member_from_expr(other).unwrap());
                            NestedInitializer::FactoryMember
                        }
                        _ => NestedInitializer::Unsupported,
                    });
                self.variables.insert(
                    binding.id.to_id(),
                    NestedVariable {
                        kind: node.kind,
                        init,
                    },
                );
            } else if let Pat::Object(pattern) = &declaration.name {
                let owner = declaration.init.as_deref().and_then(|init| {
                    let Expr::Ident(parameter) = unwrap_expr(init) else {
                        return None;
                    };
                    self.owners.iter().rev().find_map(|owner| {
                        owner
                            .params
                            .iter()
                            .enumerate()
                            .find_map(|(index, (candidate, default))| {
                                candidate
                                    .as_ref()
                                    .filter(|candidate| candidate.to_id() == parameter.to_id())
                                    .map(|_| {
                                        (owner.binding.to_id(), index, *default, parameter.clone())
                                    })
                            })
                    })
                });
                let plain = plain_object_pattern(pattern);
                if let Some((factory, index, default, parameter)) = owner {
                    for (target, property) in object_pattern_members(pattern) {
                        self.names.push(target.clone());
                        if node.kind != swc_core::ecma::ast::VarDeclKind::Const || !plain {
                            self.variables.insert(
                                target.to_id(),
                                NestedVariable {
                                    kind: node.kind,
                                    init: Some(NestedInitializer::Unsupported),
                                },
                            );
                        } else {
                            self.factory_members.insert(
                                target.to_id(),
                                FactoryMember {
                                    factory: factory.clone(),
                                    index,
                                    property,
                                    parameter: Some(parameter.clone()),
                                    default,
                                    rest: false,
                                    invalid_pattern: false,
                                },
                            );
                        }
                    }
                    if plain && node.kind == swc_core::ecma::ast::VarDeclKind::Const {
                        self.allowed_parameter_reads.insert(parameter.span.lo.0);
                    }
                }
            }
            if let (Pat::Ident(binding), Some(init)) = (&declaration.name, &declaration.init) {
                match unwrap_expr(init) {
                    Expr::Arrow(arrow) => {
                        self.enter_function(binding.id.clone(), &arrow.params);
                        declaration.visit_children_with(self);
                        self.owners.pop();
                        continue;
                    }
                    Expr::Fn(function) => {
                        let params: Vec<_> = function
                            .function
                            .params
                            .iter()
                            .map(|param| param.pat.clone())
                            .collect();
                        self.enter_function(binding.id.clone(), &params);
                        declaration.visit_children_with(self);
                        self.owners.pop();
                        continue;
                    }
                    _ => {}
                }
            }
            declaration.visit_children_with(self);
        }
    }

    fn visit_expr(&mut self, node: &Expr) {
        #[cfg(test)]
        {
            self.expression_visits += 1;
        }
        if let Expr::Ident(ident) = node {
            self.record_reference(ident);
        }
        node.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if static_member_name(member).is_some() {
            if let Expr::Ident(parameter) = unwrap_expr(&member.obj) {
                self.allowed_parameter_reads.insert(parameter.span.lo.0);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(ident) = unwrap_expr(callee) {
                self.calls
                    .entry(ident.to_id())
                    .or_default()
                    .push(call.clone());
                self.direct_callees.insert(ident.span.lo.0);
            } else if let Expr::Seq(sequence) = unwrap_expr(callee) {
                if sequence.exprs.len() == 2
                    && matches!(unwrap_expr(&sequence.exprs[0]), Expr::Lit(Lit::Num(number)) if number.value == 0.0)
                {
                    if let Expr::Ident(ident) = unwrap_expr(&sequence.exprs[1]) {
                        self.calls
                            .entry(ident.to_id())
                            .or_default()
                            .push(call.clone());
                        self.direct_callees.insert(ident.span.lo.0);
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, node: &swc_core::ecma::ast::AssignExpr) {
        if let Some(ident) = node.left.as_ident() {
            self.written_parameters
                .entry(ident.id.to_id())
                .or_default()
                .push(ident.id.span);
        } else if let Some(swc_core::ecma::ast::SimpleAssignTarget::Member(member)) =
            node.left.as_simple()
        {
            self.record_parameter_write(&member.obj);
        }
        node.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, node: &swc_core::ecma::ast::UnaryExpr) {
        if node.op == swc_core::ecma::ast::UnaryOp::Delete {
            self.record_parameter_write(&node.arg);
        }
        node.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, node: &swc_core::ecma::ast::UpdateExpr) {
        self.record_parameter_write(&node.arg);
        node.visit_children_with(self);
    }

    fn visit_jsx_opening_element(&mut self, node: &swc_core::ecma::ast::JSXOpeningElement) {
        if let JSXElementName::Ident(ident) = &node.name {
            self.record_reference(ident);
        }
        node.visit_children_with(self);
    }

    fn visit_prop(&mut self, node: &Prop) {
        if let Prop::Shorthand(ident) = node {
            self.record_reference(ident);
        }
        node.visit_children_with(self);
    }
}

type FunctionReturn = (Expr, Vec<Pat>);
type FunctionReturnIndex = HashMap<u32, Rc<FunctionReturn>>;

/// Gather the same declarations the former NestedReturnFinder searched, but
/// visit the module once rather than once per function summary.
struct ReturnCollector<'a> {
    returns: FunctionReturnIndex,
    owned_factory_sites: &'a HashSet<u32>,
    #[cfg(test)]
    expression_visits: usize,
}

impl ReturnCollector<'_> {
    fn record(&mut self, position: u32, returned: Option<FunctionReturn>) {
        if let Some((expr, params)) = returned {
            // All other return expressions are Ordinary in summarize_wrapper.
            // Do not retain large ordinary object/literal returns in the index.
            if matches!(unwrap_expr(&expr), Expr::JSXElement(_) | Expr::Call(_)) {
                self.returns.insert(position, Rc::new((expr, params)));
            }
        }
    }

    fn record_function(&mut self, function: &swc_core::ecma::ast::Function) {
        self.record(
            function.span.lo.0,
            body_return(
                function.body.as_ref(),
                function
                    .params
                    .iter()
                    .map(|param| param.pat.clone())
                    .collect(),
                self.owned_factory_sites,
            ),
        );
    }
}

impl Visit for ReturnCollector<'_> {
    fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
        self.record_function(&node.function);
        node.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, node: &VarDeclarator) {
        if let Some(init) = node.init.as_deref().map(unwrap_expr) {
            match init {
                Expr::Arrow(arrow) => self.record(
                    arrow.span.lo.0,
                    arrow_return(arrow, self.owned_factory_sites),
                ),
                Expr::Fn(function) => self.record_function(&function.function),
                _ => {}
            }
        }
        node.visit_children_with(self);
    }

    #[cfg(test)]
    fn visit_expr(&mut self, node: &Expr) {
        self.expression_visits += 1;
        node.visit_children_with(self);
    }
}

#[derive(Clone)]
enum Form {
    Jsx(JSXElement),
    Call(CallExpr),
    Container(Expr),
}

enum Child {
    Target(FunctionValue),
    Empty,
    Dynamic(String),
}

#[derive(Clone)]
enum WrapperSummary {
    Fixed(FunctionValue),
    ForwardChild,
    Unsupported(String),
    Ordinary,
}

impl Form {
    fn span(&self) -> Span {
        match self {
            Self::Jsx(node) => node.span,
            Self::Call(node) => node.span,
            Self::Container(node) => node.span(),
        }
    }
}

#[derive(Default)]
struct FormCollector {
    forms: Vec<Form>,
}

impl Visit for FormCollector {
    fn visit_jsx_element(&mut self, node: &JSXElement) {
        self.forms.push(Form::Jsx(node.clone()));
        node.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, node: &CallExpr) {
        self.forms.push(Form::Call(node.clone()));
        node.visit_children_with(self);
    }

    fn visit_array_lit(&mut self, node: &swc_core::ecma::ast::ArrayLit) {
        self.forms.push(Form::Container(Expr::Array(node.clone())));
        node.visit_children_with(self);
    }

    fn visit_object_lit(&mut self, node: &ObjectLit) {
        self.forms.push(Form::Container(Expr::Object(node.clone())));
        node.visit_children_with(self);
    }
}

#[derive(Clone)]
struct FactoryCallSite {
    path: PathBuf,
    call: CallExpr,
}

#[derive(Default)]
struct FactoryReferenceCollector {
    calls: Vec<(Expr, CallExpr)>,
    references: Vec<(Expr, Span)>,
    direct: HashSet<(u32, u32)>,
    jsx: Vec<(JSXElementName, Span)>,
}

impl Visit for FactoryReferenceCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            let callee = unwrap_expr(callee);
            let direct = match callee {
                Expr::Seq(sequence)
                    if sequence.exprs.len() == 2
                        && matches!(unwrap_expr(&sequence.exprs[0]), Expr::Lit(Lit::Num(number)) if number.value == 0.0) =>
                {
                    unwrap_expr(&sequence.exprs[1])
                }
                other => other,
            };
            if matches!(direct, Expr::Ident(_) | Expr::Member(_)) {
                self.direct.insert((direct.span().lo.0, direct.span().hi.0));
                self.calls.push((direct.clone(), call.clone()));
            }
        }
        call.visit_children_with(self);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if matches!(expr, Expr::Ident(_) | Expr::Member(_)) {
            self.references.push((expr.clone(), expr.span()));
        }
        expr.visit_children_with(self);
    }

    fn visit_prop(&mut self, prop: &Prop) {
        if let Prop::Shorthand(ident) = prop {
            self.references
                .push((Expr::Ident(ident.clone()), ident.span));
        }
        prop.visit_children_with(self);
    }

    fn visit_export_default_expr(&mut self, node: &swc_core::ecma::ast::ExportDefaultExpr) {
        if !matches!(unwrap_expr(&node.expr), Expr::Ident(_)) {
            node.visit_children_with(self);
        }
    }

    fn visit_jsx_opening_element(&mut self, node: &swc_core::ecma::ast::JSXOpeningElement) {
        self.jsx.push((node.name.clone(), node.span));
        node.visit_children_with(self);
    }
}

struct Discovery<'a, R: Resolver> {
    resolver: &'a R,
    modules: BTreeMap<PathBuf, Rc<SourceModule>>,
    resolving: HashSet<(PathBuf, String)>,
    summarizing: HashSet<Definition>,
    summary_cache: BTreeMap<Definition, WrapperSummary>,
    deferred_forward_sites: HashSet<(PathBuf, u32)>,
    owned_factory_sites: HashMap<PathBuf, Rc<HashSet<u32>>>,
    nested_bindings: HashMap<PathBuf, Rc<NestedBindings>>,
    factory_proofs: HashMap<(Definition, usize, String), Value>,
    factory_member_checks: HashMap<(PathBuf, BindingId), Option<Value>>,
    proving_factories: HashSet<Definition>,
    calls_by_callee: BTreeMap<Definition, Vec<FactoryCallSite>>,
    escapes_by_callee: BTreeMap<Definition, Vec<(PathBuf, Span)>>,
    audited_modules: HashSet<PathBuf>,
    function_returns: HashMap<PathBuf, Rc<FunctionReturnIndex>>,
    primed_modules: HashSet<PathBuf>,
    #[cfg(test)]
    function_return_expression_visits: usize,
    #[cfg(test)]
    nested_binding_expression_visits: usize,
    #[cfg(test)]
    factory_proof_evaluations: usize,
    #[cfg(test)]
    factory_resolver_operations: usize,
}

impl<'a, R: Resolver> Discovery<'a, R> {
    fn new(resolver: &'a R, modules: BTreeMap<PathBuf, SourceModule>) -> Self {
        Self {
            resolver,
            modules: modules
                .into_iter()
                .map(|(path, module)| (path, Rc::new(module)))
                .collect(),
            resolving: HashSet::new(),
            summarizing: HashSet::new(),
            summary_cache: BTreeMap::new(),
            deferred_forward_sites: HashSet::new(),
            owned_factory_sites: HashMap::new(),
            nested_bindings: HashMap::new(),
            factory_proofs: HashMap::new(),
            factory_member_checks: HashMap::new(),
            proving_factories: HashSet::new(),
            calls_by_callee: BTreeMap::new(),
            escapes_by_callee: BTreeMap::new(),
            audited_modules: HashSet::new(),
            function_returns: HashMap::new(),
            primed_modules: HashSet::new(),
            #[cfg(test)]
            function_return_expression_visits: 0,
            #[cfg(test)]
            nested_binding_expression_visits: 0,
            #[cfg(test)]
            factory_proof_evaluations: 0,
            #[cfg(test)]
            factory_resolver_operations: 0,
        }
    }

    fn module(&mut self, path: &Path) -> ScanResult<Rc<SourceModule>> {
        if let Some(module) = self.modules.get(path) {
            return Ok(module.clone());
        }
        if !is_scannable_source(path) {
            return Err(ScanError::Registration {
                path: path.to_path_buf(),
                line: 1,
                column: 1,
                message: "demanded binding resolves to a non-JavaScript module; export a named function from a \"use client\" JS/TS module".into(),
            });
        }
        let source = self
            .resolver
            .read(path)
            .map_err(|message| ScanError::Resolver {
                path: path.to_path_buf(),
                message,
            })?;
        let parsed = parse_module(path, &source)?;
        let client = has_use_client_directive(&parsed);
        let (ast, _) = resolve_worker_bindings(parsed);
        let module = Rc::new(SourceModule {
            ast,
            source,
            client,
        });
        self.modules.insert(path.to_path_buf(), module.clone());
        Ok(module)
    }

    fn diagnostic(&self, path: &Path, span: Span, message: impl Into<String>) -> ScanError {
        let source = self.modules.get(path).map(|module| module.source.as_str());
        let (line, column) = line_column(source, span.lo.0);
        ScanError::Registration {
            path: path.to_path_buf(),
            line,
            column,
            message: format!(
                "{}; export a named function from a \"use client\" module and pass it directly as <Counter /> or h(Counter, props). Use separate boundaries for conditional children",
                message.into()
            ),
        }
    }

    fn definition_location(&self, definition: &Definition) -> String {
        let source = self
            .modules
            .get(&definition.module)
            .map(|module| module.source.as_str());
        let (line, column) = line_column(source, definition.position);
        format!("{}:{line}:{column}", definition.module.display())
    }

    fn resolve_source(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let dir = from.parent().unwrap_or_else(|| Path::new("."));
        self.resolver.resolve_demanded(dir, specifier)
    }

    fn sdk_export(specifier: &str, export: &str) -> Option<Value> {
        match (specifier, export) {
            ("@takazudo/zfb", "Island") => Some(Value::Boundary),
            ("@takazudo/zfb/zudo-react", "h") => Some(Value::Factory(FactoryKind::H)),
            ("@takazudo/zfb/zudo-react/jsx-runtime", "jsx" | "jsxs")
            | ("@takazudo/zfb/zudo-react/jsx-dev-runtime", "jsxDEV") => {
                Some(Value::Factory(FactoryKind::Jsx))
            }
            _ => None,
        }
    }

    fn is_sdk_namespace(specifier: &str) -> bool {
        matches!(
            specifier,
            "@takazudo/zfb"
                | "@takazudo/zfb/zudo-react"
                | "@takazudo/zfb/zudo-react/jsx-runtime"
                | "@takazudo/zfb/zudo-react/jsx-dev-runtime"
        )
    }

    fn resolve_import(&mut self, path: &Path, specifier: &str, name: &str) -> ScanResult<Value> {
        if let Some(value) = Self::sdk_export(specifier, name) {
            return Ok(value);
        }
        let Some(target) = self.resolve_source(path, specifier) else {
            return Ok(Value::Unsupported(format!(
                "demanded import {name:?} from {specifier:?} could not be resolved"
            )));
        };
        // Asset and configured-loader imports can appear in ordinary JSX
        // props next to a real boundary. Only a boundary child demands a
        // component from them; keep the unsupported value until that site is
        // evaluated so unrelated resource expressions do not fail the scan.
        if !is_scannable_source(&target) {
            return Ok(Value::Unsupported(format!(
                "demanded binding from {specifier:?} resolves to a non-JavaScript module"
            )));
        }
        self.resolve_export(&target, name)
    }

    fn definition(&self, path: &Path, binding: &str, marker: String, span: Span) -> Value {
        Value::Function(FunctionValue {
            definition: Definition {
                module: canonicalize_or_self(path),
                binding: binding.to_string(),
                position: span.lo.0,
            },
            marker,
            route: None,
        })
    }

    fn resolve_local(
        &mut self,
        path: &Path,
        ident: &swc_core::ecma::ast::Ident,
    ) -> ScanResult<Value> {
        let key = (
            path.to_path_buf(),
            format!("{}:{:?}", ident.sym, ident.ctxt),
        );
        if !self.resolving.insert(key.clone()) {
            return Ok(Value::Unsupported(format!(
                "recursive binding {}",
                ident.sym
            )));
        }
        let result = self.resolve_local_inner(path, ident);
        self.resolving.remove(&key);
        result
    }

    fn resolve_local_inner(
        &mut self,
        path: &Path,
        ident: &swc_core::ecma::ast::Ident,
    ) -> ScanResult<Value> {
        let module = self.module(path)?;
        for item in &module.ast.body {
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(default)) = item {
                match &default.decl {
                    DefaultDecl::Fn(function)
                        if function
                            .ident
                            .as_ref()
                            .is_some_and(|name| name.to_id() == ident.to_id()) =>
                    {
                        return Ok(self.definition(
                            path,
                            &ident.sym,
                            ident.sym.to_string(),
                            function.function.span,
                        ));
                    }
                    DefaultDecl::Class(class)
                        if class
                            .ident
                            .as_ref()
                            .is_some_and(|name| name.to_id() == ident.to_id()) =>
                    {
                        return Ok(Value::Unsupported(format!(
                            "class target {} is unsupported; use a named function",
                            ident.sym
                        )));
                    }
                    _ => {}
                }
            }
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
                if import.type_only {
                    continue;
                }
                for spec in &import.specifiers {
                    if spec.is_type_only() || spec.local().to_id() != ident.to_id() {
                        continue;
                    }
                    let source = atom_to_string(&import.src.value);
                    let value = match spec {
                        ImportSpecifier::Named(named) => {
                            let name = named
                                .imported
                                .as_ref()
                                .map(module_export_name)
                                .unwrap_or_else(|| named.local.sym.to_string());
                            self.resolve_import(path, &source, &name)
                        }
                        ImportSpecifier::Default(_) => {
                            self.resolve_import(path, &source, "default")
                        }
                        ImportSpecifier::Namespace(_) => {
                            if Self::is_sdk_namespace(&source) {
                                Ok(Value::SdkNamespace(source.clone()))
                            } else {
                                Ok(self
                                    .resolve_source(path, &source)
                                    .map(Value::Namespace)
                                    .unwrap_or_else(|| {
                                        Value::Unsupported(format!(
                                            "namespace import from {source:?} could not be resolved"
                                        ))
                                    }))
                            }
                        }
                    };
                    return value.map(|value| {
                        if value == Value::Other {
                            Value::Unsupported(format!(
                                "demanded binding {} from {source:?} is not exported",
                                ident.sym
                            ))
                        } else {
                            value
                        }
                    });
                }
            }
            let declaration = match item {
                ModuleItem::Stmt(Stmt::Decl(decl)) => Some(decl),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => Some(&export.decl),
                _ => None,
            };
            let Some(declaration) = declaration else {
                continue;
            };
            match declaration {
                Decl::Fn(function) if function.ident.to_id() == ident.to_id() => {
                    return Ok(self.definition(
                        path,
                        &function.ident.sym,
                        function.ident.sym.to_string(),
                        function.function.span,
                    ));
                }
                Decl::Var(variable) => {
                    for declarator in &variable.decls {
                        let Pat::Ident(binding) = &declarator.name else {
                            continue;
                        };
                        if binding.id.to_id() != ident.to_id() {
                            continue;
                        }
                        if variable.kind != swc_core::ecma::ast::VarDeclKind::Const {
                            return Ok(Value::Unsupported(format!(
                                "mutable target binding {} is not a stable function",
                                ident.sym
                            )));
                        }
                        let Some(init) = &declarator.init else {
                            return Ok(Value::Unsupported(format!(
                                "target binding {} has no initializer",
                                ident.sym
                            )));
                        };
                        return match unwrap_expr(init) {
                            Expr::Fn(function) => Ok(self.definition(
                                path,
                                &binding.id.sym,
                                function
                                    .ident
                                    .as_ref()
                                    .map(|id| id.sym.to_string())
                                    .unwrap_or_else(|| binding.id.sym.to_string()),
                                function.function.span,
                            )),
                            Expr::Arrow(arrow) => Ok(self.definition(
                                path,
                                &binding.id.sym,
                                binding.id.sym.to_string(),
                                arrow.span,
                            )),
                            Expr::Ident(alias) => self.resolve_local(path, alias),
                            other => {
                                if self
                                    .nested_bindings(path)?
                                    .factory_members
                                    .contains_key(&ident.to_id())
                                {
                                    self.resolve_factory_member(path, ident)
                                } else {
                                    Ok(Value::Unsupported(format!(
                                        "target {} has unsupported initializer {:?}",
                                        ident.sym,
                                        other.span()
                                    )))
                                }
                            }
                        };
                    }
                }
                Decl::Class(class) if class.ident.to_id() == ident.to_id() => {
                    return Ok(Value::Unsupported(format!(
                        "class target {} is unsupported; use a named function",
                        ident.sym
                    )));
                }
                _ => {}
            }
        }
        // Large packed dependencies contain many nested locals, parameters,
        // and globals. A missing name must not trigger a fresh full AST walk
        // for every reference (#3648, fixed-scanner CPU profiles).
        let bindings = self.nested_bindings(path)?;
        let id = ident.to_id();
        if let Some((marker, span)) = bindings.functions.get(&id) {
            return Ok(self.definition(path, &ident.sym, marker.clone(), *span));
        }
        if let Some(variable) = bindings.variables.get(&id) {
            if variable.kind != swc_core::ecma::ast::VarDeclKind::Const {
                return Ok(Value::Unsupported(format!(
                    "mutable target binding {} is not a stable function",
                    ident.sym
                )));
            }
            if let Some(init) = &variable.init {
                return match init {
                    NestedInitializer::Function { marker, span } => {
                        Ok(self.definition(path, &ident.sym, marker.clone(), *span))
                    }
                    NestedInitializer::Alias(alias) => self.resolve_local(path, alias),
                    NestedInitializer::FactoryMember => self.resolve_factory_member(path, ident),
                    NestedInitializer::Unsupported => Ok(Value::Unsupported(format!(
                        "target {} has unsupported initializer",
                        ident.sym
                    ))),
                };
            }
        }
        if bindings.factory_members.contains_key(&id) {
            return self.resolve_factory_member(path, ident);
        }
        Ok(Value::Other)
    }

    fn site_location(&self, path: &Path, span: Span) -> String {
        let source = self.modules.get(path).map(|module| module.source.as_str());
        let (line, column) = line_column(source, span.lo.0);
        format!("{}:{line}:{column}", path.display())
    }

    fn factory_failure_reason(target: &str, reason: String) -> String {
        format!(
            "target {target} has unsupported initializer: {reason}; see concepts/islands#boundary-discovery-and-migration"
        )
    }

    fn factory_failure(target: &str, reason: String) -> Value {
        Value::Unsupported(Self::factory_failure_reason(target, reason))
    }

    fn resolve_factory_member(
        &mut self,
        path: &Path,
        target: &swc_core::ecma::ast::Ident,
    ) -> ScanResult<Value> {
        let bindings = self.nested_bindings(path)?;
        let Some(member) = bindings.factory_members.get(&target.to_id()) else {
            return Ok(Value::Other);
        };
        let factory = self.factory_definition(path, member)?;
        let member_key = (path.to_path_buf(), target.to_id());
        let member_check = if let Some(checked) = self.factory_member_checks.get(&member_key) {
            checked.clone()
        } else {
            let checked = self.validate_factory_member(path, target, member, &bindings);
            self.factory_member_checks
                .insert(member_key, checked.clone());
            checked
        };
        if let Some(reason) = member_check {
            return Ok(reason);
        }
        let key = (factory.clone(), member.index, member.property.clone());
        if let Some(value) = self.factory_proofs.get(&key) {
            return Ok(match value {
                Value::Unsupported(reason) if reason.contains(" has unsupported initializer") => {
                    let suffix = reason
                        .split_once(" has unsupported initializer")
                        .map(|(_, suffix)| suffix)
                        .unwrap_or("");
                    Value::Unsupported(format!(
                        "target {} has unsupported initializer{suffix}",
                        target.sym
                    ))
                }
                other => other.clone(),
            });
        }
        if !self.proving_factories.insert(factory.clone()) {
            return Ok(Self::factory_failure(
                &target.sym,
                format!("factory {} has a recursive proof cycle", factory.binding),
            ));
        }
        #[cfg(test)]
        {
            self.factory_proof_evaluations += 1;
        }
        let result = self.resolve_factory_member_inner(path, target);
        self.proving_factories.remove(&factory);
        let value = result?;
        self.factory_proofs.insert(key, value.clone());
        Ok(value)
    }

    fn factory_definition(&self, path: &Path, member: &FactoryMember) -> ScanResult<Definition> {
        let bindings = self
            .nested_bindings
            .get(path)
            .expect("factory bindings indexed");
        let Some(ident) = bindings
            .names
            .iter()
            .find(|ident| ident.to_id() == member.factory)
        else {
            return Err(ScanError::Registration {
                path: path.to_path_buf(),
                line: 1,
                column: 1,
                message: "factory binding is missing from the module index".into(),
            });
        };
        // The function span, rather than the declaration identifier, is the
        // identity returned by resolve_local and followed through imports.
        let module = self.modules.get(path).expect("factory module loaded");
        let mut span = None;
        for item in &module.ast.body {
            let declaration = match item {
                ModuleItem::Stmt(Stmt::Decl(decl)) => Some(decl),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => Some(&export.decl),
                _ => None,
            };
            if let Some(declaration) = declaration {
                match declaration {
                    Decl::Fn(function) if function.ident.to_id() == member.factory => {
                        span = Some(function.function.span)
                    }
                    Decl::Var(variable) => {
                        for declarator in &variable.decls {
                            if matches!(&declarator.name, Pat::Ident(binding) if binding.id.to_id() == member.factory)
                            {
                                span = declarator
                                    .init
                                    .as_deref()
                                    .map(|expr| unwrap_expr(expr).span());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if span.is_none() {
            span = bindings
                .functions
                .get(&member.factory)
                .map(|(_, span)| *span)
                .or_else(|| {
                    bindings
                        .variables
                        .get(&member.factory)
                        .and_then(|variable| match &variable.init {
                            Some(NestedInitializer::Function { span, .. }) => Some(*span),
                            _ => None,
                        })
                });
        }
        Ok(Definition {
            module: canonicalize_or_self(path),
            binding: ident.sym.to_string(),
            position: span.unwrap_or(ident.span).lo.0,
        })
    }

    fn factory_reference_candidate(
        expr: &Expr,
        bindings: &NestedBindings,
        imports: &HashSet<BindingId>,
    ) -> bool {
        let candidate = |ident: &swc_core::ecma::ast::Ident| {
            let id = ident.to_id();
            imports.contains(&id) || bindings.factory_functions.contains_key(&id)
        };
        match expr {
            Expr::Ident(ident) => candidate(ident),
            Expr::Member(member) => match unwrap_expr(&member.obj) {
                Expr::Ident(ident) => candidate(ident),
                other => Self::factory_reference_candidate(other, bindings, imports),
            },
            _ => false,
        }
    }

    fn audit_factory_references(&mut self, path: &Path) -> ScanResult<()> {
        if !self.audited_modules.insert(path.to_path_buf()) {
            return Ok(());
        }
        let module = self.module(path)?;
        let bindings = self.nested_bindings(path)?;
        let mut collector = FactoryReferenceCollector::default();
        module.ast.visit_with(&mut collector);
        let imports: HashSet<BindingId> = module
            .ast
            .body
            .iter()
            .filter_map(|item| match item {
                ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Some(import),
                _ => None,
            })
            .flat_map(|import| {
                import
                    .specifiers
                    .iter()
                    .map(|specifier| specifier.local().to_id())
            })
            .collect();
        let mut local_calls = Vec::new();
        for (callee, call) in &collector.calls {
            if !Self::factory_reference_candidate(callee, &bindings, &imports)
                || matches!(callee, Expr::Ident(ident) if bindings.factory_members.contains_key(&ident.to_id()))
            {
                continue;
            }
            #[cfg(test)]
            {
                self.factory_resolver_operations += 1;
            }
            if let Value::Function(function) = self.resolve_expr(path, callee)? {
                local_calls.push((function.definition, call.clone()));
            }
        }
        let mut local_escapes = Vec::new();
        for (reference, span) in &collector.references {
            if collector.direct.contains(&(span.lo.0, span.hi.0)) {
                continue;
            }
            if !Self::factory_reference_candidate(reference, &bindings, &imports)
                || matches!(reference, Expr::Ident(ident) if bindings.factory_members.contains_key(&ident.to_id()))
            {
                continue;
            }
            #[cfg(test)]
            {
                self.factory_resolver_operations += 1;
            }
            if let Value::Function(function) = self.resolve_expr(path, reference)? {
                local_escapes.push((function.definition, *span));
            }
        }
        for (name, span) in &collector.jsx {
            if matches!(name, JSXElementName::Ident(ident) if bindings.factory_members.contains_key(&ident.to_id())
                || !imports.contains(&ident.to_id()) && !bindings.factory_functions.contains_key(&ident.to_id()))
            {
                continue;
            }
            #[cfg(test)]
            {
                self.factory_resolver_operations += 1;
            }
            if let Value::Function(function) = self.resolve_jsx_name(path, name)? {
                local_escapes.push((function.definition, *span));
            }
        }
        for (definition, call) in local_calls {
            self.calls_by_callee
                .entry(definition)
                .or_default()
                .push(FactoryCallSite {
                    path: path.to_path_buf(),
                    call,
                });
        }
        for (definition, span) in local_escapes {
            self.escapes_by_callee
                .entry(definition)
                .or_default()
                .push((path.to_path_buf(), span));
        }
        Ok(())
    }

    fn validate_factory_member(
        &self,
        path: &Path,
        target: &swc_core::ecma::ast::Ident,
        member: &FactoryMember,
        bindings: &NestedBindings,
    ) -> Option<Value> {
        let factory = bindings.factory_functions.get(&member.factory)?;
        let fail = |reason| Self::factory_failure(&target.sym, reason);
        if member.default {
            return Some(fail(format!(
                "parameter {} has a default value",
                member
                    .parameter
                    .as_ref()
                    .map(|p| p.sym.as_ref())
                    .unwrap_or(&factory.name)
            )));
        }
        if member.rest {
            return Some(fail(format!(
                "destructured parameter {} uses a rest pattern",
                target.sym
            )));
        }
        if member.invalid_pattern {
            return Some(Value::Unsupported(format!(
                "target {} has unsupported initializer",
                target.sym
            )));
        }
        let mut bad_reads = Vec::new();
        bad_reads.extend(
            bindings
                .written_parameters
                .get(&target.to_id())
                .into_iter()
                .flatten()
                .map(|span| (target.sym.to_string(), *span)),
        );
        if let Some(parameter) = &member.parameter {
            bad_reads.extend(
                bindings
                    .written_parameters
                    .get(&parameter.to_id())
                    .into_iter()
                    .flatten()
                    .chain(
                        bindings
                            .references
                            .get(&parameter.to_id())
                            .into_iter()
                            .flatten()
                            .filter(|span| !bindings.allowed_parameter_reads.contains(&span.lo.0)),
                    )
                    .map(|span| (parameter.sym.to_string(), *span)),
            );
        }
        if let Some((name, span)) = bad_reads.into_iter().min_by_key(|(_, span)| span.lo.0) {
            return Some(fail(format!(
                "parameter {name} is written or escapes at {}",
                self.site_location(path, span)
            )));
        }
        None
    }

    fn resolve_factory_member_inner(
        &mut self,
        path: &Path,
        target: &swc_core::ecma::ast::Ident,
    ) -> ScanResult<Value> {
        let bindings = self.nested_bindings(path)?;
        let Some(member) = bindings.factory_members.get(&target.to_id()) else {
            return Ok(Value::Other);
        };
        let Some(factory) = bindings.factory_functions.get(&member.factory) else {
            return Ok(Value::Other);
        };
        let fail = |reason| Self::factory_failure(&target.sym, reason);
        let definition = self.factory_definition(path, member)?;
        if let Some((escape_path, span)) = self
            .escapes_by_callee
            .get(&definition)
            .and_then(|escapes| escapes.iter().min_by_key(|(path, span)| (path, &span.lo.0)))
        {
            return Ok(fail(format!(
                "factory {} escapes as a value at {}",
                factory.name,
                self.site_location(escape_path, *span)
            )));
        }
        let Some(calls) = self
            .calls_by_callee
            .get(&definition)
            .filter(|calls| !calls.is_empty())
            .cloned()
        else {
            return Ok(fail(format!(
                "factory {} has no call site in the scanned modules",
                factory.name
            )));
        };
        let mut calls: Vec<_> = calls.iter().collect();
        calls.sort_by(|left, right| {
            (&left.path, left.call.span.lo.0).cmp(&(&right.path, right.call.span.lo.0))
        });
        // Validate each category across the complete call set before moving
        // to the next one. The diagnostic precedence is part of the grammar.
        for call in &calls {
            if call.call.args.len() <= member.index {
                return Ok(fail(format!(
                    "call site at {} passes too few arguments",
                    self.site_location(&call.path, call.call.span)
                )));
            }
        }
        for call in &calls {
            if call
                .call
                .args
                .iter()
                .take(member.index + 1)
                .any(|arg| arg.spread.is_some())
            {
                return Ok(fail(format!(
                    "call site at {} spreads positional arguments",
                    self.site_location(&call.path, call.call.span)
                )));
            }
        }
        for call in &calls {
            let literal = matches!(unwrap_expr(&call.call.args[member.index].expr), Expr::Object(object)
            if object.props.iter().all(|prop| match prop {
                PropOrSpread::Prop(prop) => match &**prop {
                    Prop::Shorthand(_) => true,
                    Prop::KeyValue(pair) => !matches!(&pair.key, PropName::Computed(_)) && factory_object_key(&pair.key).is_some(),
                    _ => false,
                },
                _ => false,
            }));
            if !literal {
                return Ok(fail(format!(
                    "call site at {} passes a non-literal argument",
                    self.site_location(&call.path, call.call.span)
                )));
            }
        }
        for call in &calls {
            let Expr::Object(object) = unwrap_expr(&call.call.args[member.index].expr) else {
                unreachable!()
            };
            let count = object
                .props
                .iter()
                .filter(|prop| match prop {
                    PropOrSpread::Prop(prop) => match &**prop {
                        Prop::Shorthand(ident) => ident.sym.as_ref() == member.property.as_str(),
                        Prop::KeyValue(pair) => {
                            factory_object_key(&pair.key).as_deref()
                                == Some(member.property.as_str())
                        }
                        _ => false,
                    },
                    _ => false,
                })
                .count();
            if count == 0 {
                return Ok(fail(format!(
                    "property {} missing at call site {}",
                    member.property,
                    self.site_location(&call.path, call.call.span)
                )));
            }
            if count > 1 {
                return Ok(fail(format!(
                    "call site at {} passes a non-literal argument",
                    self.site_location(&call.path, call.call.span)
                )));
            }
        }
        for call in &calls {
            let Expr::Object(object) = unwrap_expr(&call.call.args[member.index].expr) else {
                unreachable!()
            };
            let selected = object.props.iter().find_map(|prop| match prop {
                PropOrSpread::Prop(prop) => match &**prop {
                    Prop::Shorthand(ident) if ident.sym.as_ref() == member.property.as_str() => {
                        None
                    }
                    Prop::KeyValue(pair)
                        if factory_object_key(&pair.key).as_deref()
                            == Some(member.property.as_str()) =>
                    {
                        Some(&*pair.value)
                    }
                    _ => None,
                },
                _ => None,
            });
            if selected.is_some_and(|expr| {
                matches!(
                    unwrap_expr(expr),
                    Expr::Cond(_)
                        | Expr::Bin(swc_core::ecma::ast::BinExpr {
                            op: swc_core::ecma::ast::BinaryOp::NullishCoalescing
                                | swc_core::ecma::ast::BinaryOp::LogicalOr,
                            ..
                        })
                )
            }) {
                return Ok(fail(format!(
                    "call site at {} selects {} conditionally (??, ternary, ||)",
                    self.site_location(&call.path, call.call.span),
                    member.property
                )));
            }
        }
        let mut resolved: Vec<(FunctionValue, String)> = Vec::new();
        for call in &calls {
            let location = self.site_location(&call.path, call.call.span);
            if call
                .call
                .args
                .iter()
                .take(member.index + 1)
                .any(|arg| arg.spread.is_some())
            {
                return Ok(fail(format!(
                    "call site at {location} spreads positional arguments"
                )));
            }
            let Some(argument) = call.call.args.get(member.index) else {
                return Ok(fail(format!(
                    "call site at {location} passes too few arguments"
                )));
            };
            let Expr::Object(object) = unwrap_expr(&argument.expr) else {
                return Ok(fail(format!(
                    "call site at {location} passes a non-literal argument"
                )));
            };
            let mut value: Option<Expr> = None;
            for prop in &object.props {
                let (name, expr): (String, Expr) = match prop {
                    PropOrSpread::Prop(prop) => match &**prop {
                        Prop::Shorthand(ident) => {
                            (ident.sym.to_string(), Expr::Ident(ident.clone()))
                        }
                        Prop::KeyValue(pair) if !matches!(&pair.key, PropName::Computed(_)) => {
                            let Some(name) = factory_object_key(&pair.key) else {
                                return Ok(fail(format!(
                                    "call site at {location} passes a non-literal argument"
                                )));
                            };
                            (name, (*pair.value).clone())
                        }
                        _ => {
                            return Ok(fail(format!(
                                "call site at {location} passes a non-literal argument"
                            )))
                        }
                    },
                    _ => {
                        return Ok(fail(format!(
                            "call site at {location} passes a non-literal argument"
                        )))
                    }
                };
                if name == member.property {
                    if value.is_some() {
                        return Ok(fail(format!(
                            "call site at {location} passes a non-literal argument"
                        )));
                    }
                    value = Some(expr);
                }
            }
            let Some(expr) = value else {
                return Ok(fail(format!(
                    "property {} missing at call site {location}",
                    member.property
                )));
            };
            if matches!(
                unwrap_expr(&expr),
                Expr::Cond(_)
                    | Expr::Bin(swc_core::ecma::ast::BinExpr {
                        op: swc_core::ecma::ast::BinaryOp::NullishCoalescing
                            | swc_core::ecma::ast::BinaryOp::LogicalOr,
                        ..
                    })
            ) {
                return Ok(fail(format!(
                    "call site at {location} selects {} conditionally (??, ternary, ||)",
                    member.property
                )));
            }
            #[cfg(test)]
            {
                self.factory_resolver_operations += 1;
            }
            let function = match self.resolve_expr(&call.path, &expr)? {
                Value::Function(function) => function,
                Value::Unsupported(reason) => {
                    return Ok(fail(format!(
                        "property {} at {location} is not a function binding: {reason}",
                        member.property
                    )))
                }
                _ => {
                    let detail = match unwrap_expr(&expr) {
                        Expr::Call(call) => match &call.callee {
                            Callee::Expr(callee) => match unwrap_expr(callee) {
                                Expr::Ident(ident) => {
                                    format!("; {}(...) is a call result", ident.sym)
                                }
                                _ => String::new(),
                            },
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    return Ok(fail(format!(
                        "property {} at {location} is not a function binding{detail}",
                        member.property
                    )));
                }
            };
            resolved.push((function, location));
        }
        let (first, first_location) = &resolved[0];
        for (function, location) in resolved.iter().skip(1) {
            if first.definition != function.definition {
                return Ok(fail(format!(
                    "call sites disagree: {first_location} passes {}, {location} passes {}",
                    first.marker, function.marker
                )));
            }
        }
        Ok(Value::Function(first.clone()))
    }

    fn nested_bindings(&mut self, path: &Path) -> ScanResult<Rc<NestedBindings>> {
        if let Some(bindings) = self.nested_bindings.get(path) {
            return Ok(Rc::clone(bindings));
        }
        let module = self.module(path)?;
        let mut bindings = NestedBindings::default();
        module.ast.visit_with(&mut bindings);
        #[cfg(test)]
        {
            self.nested_binding_expression_visits += bindings.expression_visits;
        }
        let bindings = Rc::new(bindings);
        self.nested_bindings
            .insert(path.to_path_buf(), Rc::clone(&bindings));
        Ok(bindings)
    }

    fn resolve_export(&mut self, path: &Path, name: &str) -> ScanResult<Value> {
        let key = (path.to_path_buf(), format!("export:{name}"));
        if !self.resolving.insert(key.clone()) {
            return Ok(Value::Other);
        }
        let result = self.resolve_export_inner(path, name);
        self.resolving.remove(&key);
        result
    }

    fn resolve_export_inner(&mut self, path: &Path, name: &str) -> ScanResult<Value> {
        let module = self.module(path)?;
        let mut explicit = Vec::new();
        let mut stars = Vec::new();
        for item in &module.ast.body {
            let ModuleItem::ModuleDecl(decl) = item else {
                continue;
            };
            match decl {
                ModuleDecl::ExportDecl(export) => {
                    let ident = match &export.decl {
                        Decl::Fn(function) => Some(&function.ident),
                        Decl::Class(class) => Some(&class.ident),
                        Decl::Var(variable) => variable.decls.iter().find_map(|declarator| {
                            if let Pat::Ident(binding) = &declarator.name {
                                (binding.id.sym == name).then_some(&binding.id)
                            } else {
                                None
                            }
                        }),
                        _ => None,
                    };
                    if let Some(ident) = ident.filter(|ident| ident.sym == name) {
                        explicit.push(self.resolve_local(path, ident)?);
                    }
                }
                ModuleDecl::ExportDefaultDecl(default) if name == "default" => {
                    explicit.push(match &default.decl {
                        DefaultDecl::Fn(function) => self.definition(
                            path,
                            function
                                .ident
                                .as_ref()
                                .map(|id| id.sym.as_ref())
                                .unwrap_or("default"),
                            function
                                .ident
                                .as_ref()
                                .map(|id| id.sym.to_string())
                                .unwrap_or_else(|| "default".into()),
                            function.function.span,
                        ),
                        DefaultDecl::Class(class) => Value::Unsupported(format!(
                            "class target {} is unsupported; use a named function",
                            class
                                .ident
                                .as_ref()
                                .map(|name| name.sym.as_ref())
                                .unwrap_or("default")
                        )),
                        _ => Value::Unsupported("default target must be a function".into()),
                    });
                }
                ModuleDecl::ExportDefaultExpr(default) if name == "default" => {
                    explicit.push(match unwrap_expr(&default.expr) {
                        Expr::Ident(alias) => self.resolve_local(path, alias)?,
                        Expr::Fn(function) => self.definition(
                            path,
                            "default",
                            function
                                .ident
                                .as_ref()
                                .map(|id| id.sym.to_string())
                                .unwrap_or_else(|| "default".into()),
                            function.function.span,
                        ),
                        Expr::Arrow(arrow) => {
                            self.definition(path, "default", "default".into(), arrow.span)
                        }
                        _ => Value::Unsupported(
                            "opaque default target is not a literal function".into(),
                        ),
                    });
                }
                ModuleDecl::ExportNamed(named) if !named.type_only => {
                    for spec in &named.specifiers {
                        match spec {
                            ExportSpecifier::Named(spec) if !spec.is_type_only => {
                                let exported = spec.exported.as_ref().unwrap_or(&spec.orig);
                                if module_export_name(exported) != name {
                                    continue;
                                }
                                let value = if let Some(source) = &named.src {
                                    self.resolve_import(
                                        path,
                                        &atom_to_string(&source.value),
                                        &module_export_name(&spec.orig),
                                    )?
                                } else if let ModuleExportName::Ident(local) = &spec.orig {
                                    self.resolve_local(path, local)?
                                } else {
                                    Value::Other
                                };
                                explicit.push(value);
                            }
                            ExportSpecifier::Namespace(spec)
                                if module_export_name(&spec.name) == name =>
                            {
                                if let Some(source) = &named.src {
                                    let source = atom_to_string(&source.value);
                                    explicit.push(if Self::is_sdk_namespace(&source) {
                                        Value::SdkNamespace(source)
                                    } else {
                                        self.resolve_source(path, &source)
                                            .map(Value::Namespace)
                                            .unwrap_or(Value::Other)
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                ModuleDecl::ExportAll(all) if name != "default" && !all.type_only => {
                    stars.push(atom_to_string(&all.src.value));
                }
                _ => {}
            }
        }
        let mut value = if explicit.is_empty() {
            let mut providers = Vec::new();
            for source in stars {
                let candidate = self.resolve_import(path, &source, name)?;
                if candidate != Value::Other
                    && !providers
                        .iter()
                        .any(|provider: &Value| provider.same_provider(&candidate))
                {
                    providers.push(candidate);
                }
            }
            if providers.len() > 1 {
                Value::Unsupported(format!(
                    "ambiguous export * providers for {name:?} in {}",
                    path.display()
                ))
            } else {
                providers.pop().unwrap_or(Value::Other)
            }
        } else {
            explicit.remove(0)
        };
        if module.client && !is_virtual_scan_path(path) {
            if let Value::Function(function) = &mut value {
                function.route = Some((path.to_path_buf(), name.to_string()));
            }
        }
        Ok(value)
    }

    fn resolve_expr(&mut self, path: &Path, expr: &Expr) -> ScanResult<Value> {
        match unwrap_expr(expr) {
            Expr::Ident(ident) => self.resolve_local(path, ident),
            Expr::Member(member) => {
                let property = match &member.prop {
                    MemberProp::Ident(ident) => Some(ident.sym.to_string()),
                    MemberProp::Computed(computed) => match unwrap_expr(&computed.expr) {
                        Expr::Lit(Lit::Str(text)) => Some(atom_to_string(&text.value)),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(property) = property else {
                    return Ok(Value::Unsupported("dynamic namespace member".into()));
                };
                match self.resolve_expr(path, &member.obj)? {
                    Value::Namespace(source) => self.resolve_export(&source, &property),
                    Value::SdkNamespace(source) => {
                        Ok(Self::sdk_export(&source, &property).unwrap_or(Value::Other))
                    }
                    _ => Ok(Value::Other),
                }
            }
            Expr::Seq(sequence)
                if sequence.exprs.len() == 2
                    && matches!(unwrap_expr(&sequence.exprs[0]), Expr::Lit(Lit::Num(number)) if number.value == 0.0) =>
            {
                self.resolve_expr(path, &sequence.exprs[1])
            }
            _ => Ok(Value::Other),
        }
    }

    fn resolve_jsx_name(&mut self, path: &Path, name: &JSXElementName) -> ScanResult<Value> {
        match name {
            JSXElementName::Ident(ident) => self.resolve_local(path, ident),
            JSXElementName::JSXMemberExpr(JSXMemberExpr { obj, prop, .. }) => {
                let base = self.resolve_jsx_object(path, obj)?;
                match base {
                    Value::Namespace(source) => self.resolve_export(&source, &prop.sym),
                    Value::SdkNamespace(source) => {
                        Ok(Self::sdk_export(&source, &prop.sym).unwrap_or(Value::Other))
                    }
                    _ => Ok(Value::Other),
                }
            }
            _ => Ok(Value::Other),
        }
    }

    fn resolve_jsx_object(&mut self, path: &Path, object: &JSXObject) -> ScanResult<Value> {
        match object {
            JSXObject::Ident(ident) => self.resolve_local(path, ident),
            JSXObject::JSXMemberExpr(member) => {
                let base = self.resolve_jsx_object(path, &member.obj)?;
                match base {
                    Value::Namespace(source) => self.resolve_export(&source, &member.prop.sym),
                    Value::SdkNamespace(source) => {
                        Ok(Self::sdk_export(&source, &member.prop.sym).unwrap_or(Value::Other))
                    }
                    _ => Ok(Value::Other),
                }
            }
        }
    }

    fn child_from_jsx(&mut self, path: &Path, element: &JSXElement) -> ScanResult<Child> {
        match self.resolve_jsx_name(path, &element.opening.name)? {
            Value::Function(target) => Ok(Child::Target(target)),
            Value::Unsupported(reason) => Ok(Child::Dynamic(reason)),
            _ => Ok(Child::Dynamic(
                "boundary child must be one function component description".into(),
            )),
        }
    }

    fn child_from_expr(&mut self, path: &Path, expr: &Expr) -> ScanResult<Child> {
        self.child_from_expr_inner(path, expr, &mut HashSet::new())
    }

    fn child_from_expr_inner(
        &mut self,
        path: &Path,
        expr: &Expr,
        seen: &mut HashSet<swc_core::ecma::ast::Id>,
    ) -> ScanResult<Child> {
        match unwrap_expr(expr) {
            Expr::JSXElement(element) => self.child_from_jsx(path, element),
            Expr::Call(call) => {
                let Callee::Expr(callee) = &call.callee else {
                    return Ok(Child::Dynamic("dynamic child call".into()));
                };
                if !matches!(self.resolve_expr(path, callee)?, Value::Factory(_)) {
                    return Ok(Child::Dynamic(
                        "child call is not an owned h/jsx description".into(),
                    ));
                }
                let Some(first) = call.args.first().filter(|arg| arg.spread.is_none()) else {
                    return Ok(Child::Dynamic("description has no static component".into()));
                };
                match self.resolve_expr(path, &first.expr)? {
                    Value::Function(target) => Ok(Child::Target(target)),
                    Value::Unsupported(reason) => Ok(Child::Dynamic(reason)),
                    _ => Ok(Child::Dynamic(
                        "description target is not a static function binding".into(),
                    )),
                }
            }
            Expr::Ident(ident) => {
                // A single immutable description alias can be passed as the child.
                if !seen.insert(ident.to_id()) {
                    return Ok(Child::Dynamic(format!(
                        "recursive child alias {}",
                        ident.sym
                    )));
                }
                if let Some(init) = self.local_const_init(path, ident)? {
                    self.child_from_expr_inner(path, &init, seen)
                } else {
                    Ok(Child::Dynamic(format!(
                        "child binding {} is not a static description",
                        ident.sym
                    )))
                }
            }
            Expr::Lit(Lit::Null(_) | Lit::Bool(_)) => Ok(Child::Empty),
            Expr::Array(array) => {
                let mut child = Child::Empty;
                for element in &array.elems {
                    let Some(element) = element else { continue };
                    if element.spread.is_some() {
                        return Ok(Child::Dynamic("spread child array is dynamic".into()));
                    }
                    child = Self::merge_children(
                        child,
                        self.child_from_expr_inner(path, &element.expr, seen)?,
                    );
                }
                Ok(child)
            }
            _ => Ok(Child::Dynamic(
                "boundary child expression is dynamic".into(),
            )),
        }
    }

    fn merge_children(left: Child, right: Child) -> Child {
        match (left, right) {
            (Child::Empty, right) => right,
            (left, Child::Empty) => left,
            (Child::Dynamic(reason), _) | (_, Child::Dynamic(reason)) => Child::Dynamic(reason),
            (Child::Target(_), Child::Target(_)) => {
                Child::Dynamic("boundary has multiple component children".into())
            }
        }
    }

    fn local_const_init(
        &mut self,
        path: &Path,
        ident: &swc_core::ecma::ast::Ident,
    ) -> ScanResult<Option<Box<Expr>>> {
        // Parameters and globals cannot have a local const initializer. Reuse
        // the declaration index so wrapper forwarding does not scan the whole
        // module again for each incoming child/props reference.
        if !self
            .nested_bindings(path)?
            .variables
            .get(&ident.to_id())
            .is_some_and(|variable| {
                variable.kind == swc_core::ecma::ast::VarDeclKind::Const && variable.init.is_some()
            })
        {
            return Ok(None);
        }
        let module = self.module(path)?;
        struct Finder {
            id: swc_core::ecma::ast::Id,
            init: Option<Box<Expr>>,
        }
        impl Visit for Finder {
            fn visit_var_decl(&mut self, variable: &swc_core::ecma::ast::VarDecl) {
                if variable.kind == swc_core::ecma::ast::VarDeclKind::Const {
                    for declarator in &variable.decls {
                        if let Pat::Ident(binding) = &declarator.name {
                            if binding.id.to_id() == self.id {
                                self.init = declarator.init.clone();
                                return;
                            }
                        }
                    }
                }
                variable.visit_children_with(self);
            }
        }
        let mut finder = Finder {
            id: ident.to_id(),
            init: None,
        };
        module.ast.visit_with(&mut finder);
        Ok(finder.init)
    }

    fn child_from_jsx_boundary(&mut self, path: &Path, element: &JSXElement) -> ScanResult<Child> {
        let mut explicit = None;
        for attr in &element.opening.attrs {
            match attr {
                JSXAttrOrSpread::SpreadElement(_) => {
                    explicit = Some(Child::Dynamic(
                        "later JSX spread may overwrite children".into(),
                    ));
                }
                JSXAttrOrSpread::JSXAttr(attr) if matches!(&attr.name, JSXAttrName::Ident(name) if name.sym == "children") =>
                {
                    explicit = Some(match &attr.value {
                        Some(JSXAttrValue::JSXElement(child)) => {
                            self.child_from_jsx(path, child)?
                        }
                        Some(JSXAttrValue::JSXExprContainer(container)) => match &container.expr {
                            JSXExpr::Expr(expr) => self.child_from_expr(path, expr)?,
                            _ => Child::Empty,
                        },
                        _ => Child::Dynamic("children attribute is not a description".into()),
                    });
                }
                _ => {}
            }
        }
        let mut implicit = Child::Empty;
        for child in &element.children {
            let next = match child {
                JSXElementChild::JSXText(text) if text.value.trim().is_empty() => Child::Empty,
                JSXElementChild::JSXElement(element) => self.child_from_jsx(path, element)?,
                JSXElementChild::JSXExprContainer(container) => match &container.expr {
                    JSXExpr::Expr(expr) => self.child_from_expr(path, expr)?,
                    _ => Child::Empty,
                },
                _ => Child::Dynamic("boundary child is not a single description".into()),
            };
            implicit = Self::merge_children(implicit, next);
        }
        Ok(if element.children.is_empty() {
            explicit.unwrap_or(Child::Empty)
        } else {
            implicit
        })
    }

    fn child_from_object(&mut self, path: &Path, object: &ObjectLit) -> ScanResult<Child> {
        let mut child = Child::Empty;
        for prop in &object.props {
            match prop {
                PropOrSpread::Spread(_) => {
                    child = Child::Dynamic("later props spread may overwrite children".into());
                }
                PropOrSpread::Prop(prop) => match &**prop {
                    Prop::KeyValue(pair) if prop_name(&pair.key).as_deref() == Some("children") => {
                        child = self.child_from_expr(path, &pair.value)?;
                    }
                    Prop::KeyValue(pair)
                        if matches!(&pair.key, PropName::Computed(_))
                            && prop_name(&pair.key).is_none() =>
                    {
                        child = Child::Dynamic("computed props key may overwrite children".into());
                    }
                    Prop::Shorthand(ident) if ident.sym == "children" => {
                        child = self.child_from_expr(path, &Expr::Ident(ident.clone()))?;
                    }
                    _ => {}
                },
            }
        }
        Ok(child)
    }

    fn child_from_call_props(
        &mut self,
        path: &Path,
        call: &CallExpr,
        h: bool,
    ) -> ScanResult<Child> {
        if h && call.args.len() > 2 {
            let mut child = Child::Empty;
            for arg in &call.args[2..] {
                if arg.spread.is_some() {
                    return Ok(Child::Dynamic(
                        "spread positional children are dynamic".into(),
                    ));
                }
                child = Self::merge_children(child, self.child_from_expr(path, &arg.expr)?);
            }
            return Ok(child);
        }
        match call.args.get(1) {
            Some(arg) if arg.spread.is_none() => match unwrap_expr(&arg.expr) {
                Expr::Object(object) => self.child_from_object(path, object),
                _ => Ok(Child::Dynamic(
                    "boundary props are not a static object".into(),
                )),
            },
            _ => Ok(Child::Empty),
        }
    }

    fn resolve_form(&mut self, path: &Path, form: &Form) -> ScanResult<Option<FunctionValue>> {
        if self
            .deferred_forward_sites
            .contains(&(canonicalize_or_self(path), form.span().lo.0))
        {
            return Ok(None);
        }
        let child = match form {
            Form::Container(container) => {
                let mut values: Vec<&Expr> = Vec::new();
                match container {
                    Expr::Array(array) => {
                        for element in array.elems.iter().flatten() {
                            values.push(&element.expr);
                        }
                    }
                    Expr::Object(object) => {
                        for prop in &object.props {
                            match prop {
                                PropOrSpread::Spread(spread) => values.push(&spread.expr),
                                PropOrSpread::Prop(prop) => {
                                    match &**prop {
                                        Prop::KeyValue(pair) => values.push(&pair.value),
                                        Prop::Shorthand(ident) => {
                                            let value = self.resolve_local(path, ident)?;
                                            if self.is_boundary_value(&value)? {
                                                let factory = ident.to_id();
                                                let target = self
                                                    .nested_bindings(path)?
                                                    .factory_members
                                                    .iter()
                                                    .filter(|(_, member)| member.factory == factory)
                                                    .map(|(id, _)| id.0.to_string())
                                                    .min();
                                                if let Some(target) = target {
                                                    let reason = Self::factory_failure_reason(
                                                        &target,
                                                        format!(
                                                            "factory {} escapes as a value at {}",
                                                            ident.sym,
                                                            self.site_location(path, ident.span)
                                                        ),
                                                    );
                                                    return Err(
                                                        self.diagnostic(path, ident.span, reason)
                                                    );
                                                }
                                                return Err(self.diagnostic(path, form.span(), "boundary wrapper escapes into an opaque object"));
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                for expr in values {
                    let value = self.resolve_expr(path, expr)?;
                    if self.is_boundary_value(&value)? {
                        return Err(self.diagnostic(
                            path,
                            form.span(),
                            "boundary wrapper escapes into an opaque container",
                        ));
                    }
                }
                return Ok(None);
            }
            Form::Jsx(element) => match self.resolve_jsx_name(path, &element.opening.name)? {
                Value::Boundary => self.child_from_jsx_boundary(path, element)?,
                Value::Unsupported(reason) if reason.contains("ambiguous export *") => {
                    return Err(self.diagnostic(path, form.span(), reason));
                }
                Value::Function(wrapper) => match self.summarize_wrapper(&wrapper)? {
                    WrapperSummary::Fixed(target) => Child::Target(target),
                    WrapperSummary::ForwardChild => self.child_from_jsx_boundary(path, element)?,
                    WrapperSummary::Unsupported(reason) => Child::Dynamic(reason),
                    WrapperSummary::Ordinary => return Ok(None),
                },
                _ => return Ok(None),
            },
            Form::Call(call) => {
                let Callee::Expr(callee) = &call.callee else {
                    return Ok(None);
                };
                match self.resolve_expr(path, callee)? {
                    Value::Boundary => {
                        let Some(props) = call.args.first().filter(|arg| arg.spread.is_none())
                        else {
                            return Err(self.diagnostic(
                                path,
                                form.span(),
                                "Island call has no static props",
                            ));
                        };
                        match unwrap_expr(&props.expr) {
                            Expr::Object(object) => self.child_from_object(path, object)?,
                            _ => Child::Dynamic("Island props are dynamic".into()),
                        }
                    }
                    Value::Factory(kind) => {
                        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none())
                        else {
                            return Ok(None);
                        };
                        match self.resolve_expr(path, &first.expr)? {
                            Value::Boundary => {
                                self.child_from_call_props(path, call, kind == FactoryKind::H)?
                            }
                            Value::Unsupported(reason) if reason.contains("ambiguous export *") => {
                                Child::Dynamic(reason)
                            }
                            Value::Function(wrapper) => match self.summarize_wrapper(&wrapper)? {
                                WrapperSummary::Fixed(target) => Child::Target(target),
                                WrapperSummary::ForwardChild => {
                                    self.child_from_call_props(path, call, kind == FactoryKind::H)?
                                }
                                WrapperSummary::Unsupported(reason) => Child::Dynamic(reason),
                                WrapperSummary::Ordinary => return Ok(None),
                            },
                            _ => return Ok(None),
                        }
                    }
                    Value::Function(wrapper) => match self.summarize_wrapper(&wrapper)? {
                        WrapperSummary::Fixed(target) => Child::Target(target),
                        WrapperSummary::ForwardChild => {
                            let Some(props) = call.args.first().filter(|arg| arg.spread.is_none())
                            else {
                                return Err(self.diagnostic(
                                    path,
                                    form.span(),
                                    "forwarding wrapper has no static props",
                                ));
                            };
                            match unwrap_expr(&props.expr) {
                                Expr::Object(object) => self.child_from_object(path, object)?,
                                _ => Child::Dynamic("forwarding wrapper props are dynamic".into()),
                            }
                        }
                        WrapperSummary::Unsupported(reason) => Child::Dynamic(reason),
                        WrapperSummary::Ordinary => return Ok(None),
                    },
                    _ => {
                        for arg in &call.args {
                            let value = self.resolve_expr(path, &arg.expr)?;
                            let escaped = match value {
                                Value::Boundary => true,
                                Value::Function(function) => !matches!(
                                    self.summarize_wrapper(&function)?,
                                    WrapperSummary::Ordinary
                                ),
                                _ => false,
                            };
                            if escaped {
                                return Err(self.diagnostic(
                                    path,
                                    form.span(),
                                    "SDK boundary or wrapper escapes into an opaque call",
                                ));
                            }
                        }
                        return Ok(None);
                    }
                }
            }
        };
        match child {
            Child::Target(target) => Ok(Some(target)),
            Child::Empty => {
                Err(self.diagnostic(path, form.span(), "Island has no component child"))
            }
            Child::Dynamic(reason) => Err(self.diagnostic(path, form.span(), reason)),
        }
    }

    fn is_boundary_value(&mut self, value: &Value) -> ScanResult<bool> {
        match value {
            Value::Boundary => Ok(true),
            Value::Function(function) => Ok(!matches!(
                self.summarize_wrapper(function)?,
                WrapperSummary::Ordinary
            )),
            _ => Ok(false),
        }
    }

    /// Classify each module's factory calls once. A compiled package can put
    /// hundreds of functions in one module; repeating this walk for every
    /// function multiplies binding resolution and filesystem work (#3648).
    fn owned_factory_sites(&mut self, path: &Path) -> ScanResult<Rc<HashSet<u32>>> {
        if let Some(sites) = self.owned_factory_sites.get(path) {
            return Ok(Rc::clone(sites));
        }
        let module = self.module(path)?;
        // Owned description factories are pure with respect to the incoming
        // wrapper props. Their arguments may read those props as runtime
        // component values while the component binding remains static.
        let mut collector = FormCollector::default();
        module.ast.visit_with(&mut collector);
        let mut owned_factory_sites = HashSet::new();
        for form in collector.forms {
            if let Form::Call(call) = form {
                if let Callee::Expr(callee) = &call.callee {
                    if matches!(self.resolve_expr(path, callee)?, Value::Factory(_)) {
                        owned_factory_sites.insert(call.span.lo.0);
                    }
                }
            }
        }
        let sites = Rc::new(owned_factory_sites);
        self.owned_factory_sites
            .insert(path.to_path_buf(), Rc::clone(&sites));
        Ok(sites)
    }

    fn function_return(
        &mut self,
        function: &FunctionValue,
    ) -> ScanResult<Option<Rc<FunctionReturn>>> {
        let path = &function.definition.module;
        let module = self.module(path)?;
        let owned_factory_sites = self.owned_factory_sites(path)?;
        // Preserve the existing default-export branch. Other supported
        // declarations are located by their unique source span in the index.
        if function.definition.binding == "default" {
            for item in &module.ast.body {
                if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(default)) = item {
                    if let DefaultDecl::Fn(decl) = &default.decl {
                        return Ok(body_return(
                            decl.function.body.as_ref(),
                            decl.function
                                .params
                                .iter()
                                .map(|param| param.pat.clone())
                                .collect(),
                            &owned_factory_sites,
                        )
                        .map(Rc::new));
                    }
                }
                if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(default)) = item {
                    let returned = match unwrap_expr(&default.expr) {
                        Expr::Arrow(arrow) => arrow_return(arrow, &owned_factory_sites),
                        Expr::Fn(decl) => body_return(
                            decl.function.body.as_ref(),
                            decl.function
                                .params
                                .iter()
                                .map(|param| param.pat.clone())
                                .collect(),
                            &owned_factory_sites,
                        ),
                        _ => None,
                    };
                    return Ok(returned.map(Rc::new));
                }
            }
        }
        let returns = self.function_return_index(path, &owned_factory_sites)?;
        Ok(returns.get(&function.definition.position).cloned())
    }

    fn function_return_index(
        &mut self,
        path: &Path,
        owned_factory_sites: &HashSet<u32>,
    ) -> ScanResult<Rc<FunctionReturnIndex>> {
        if let Some(returns) = self.function_returns.get(path) {
            return Ok(Rc::clone(returns));
        }
        let module = self.module(path)?;
        let mut collector = ReturnCollector {
            returns: HashMap::new(),
            owned_factory_sites,
            #[cfg(test)]
            expression_visits: 0,
        };
        module.ast.visit_with(&mut collector);
        #[cfg(test)]
        {
            self.function_return_expression_visits += collector.expression_visits;
        }
        let returns = Rc::new(collector.returns);
        self.function_returns
            .insert(path.to_path_buf(), Rc::clone(&returns));
        Ok(returns)
    }

    fn summarize_wrapper(&mut self, function: &FunctionValue) -> ScanResult<WrapperSummary> {
        if let Some(summary) = self.summary_cache.get(&function.definition) {
            return Ok(summary.clone());
        }
        if !self.summarizing.insert(function.definition.clone()) {
            // A cycle of ordinary components is not evidence of an SDK
            // boundary. Direct boundary sites in the cycle are inspected
            // separately by the static form walk.
            return Ok(WrapperSummary::Ordinary);
        }
        let result = self
            .summarize_wrapper_inner(function)
            .map(|summary| match summary {
                WrapperSummary::Unsupported(reason) => WrapperSummary::Unsupported(format!(
                    "wrapper {} in {}: {reason}",
                    function.definition.binding,
                    function.definition.module.display(),
                )),
                other => other,
            });
        self.summarizing.remove(&function.definition);
        if let Ok(summary) = &result {
            self.summary_cache
                .insert(function.definition.clone(), summary.clone());
        }
        result
    }

    fn summarize_wrapper_inner(&mut self, function: &FunctionValue) -> ScanResult<WrapperSummary> {
        let path = &function.definition.module;
        let Some(returned) = self.function_return(function)? else {
            return Ok(WrapperSummary::Ordinary);
        };
        let (returned, params) = returned.as_ref();
        let returned = unwrap_expr(returned);
        match returned {
            Expr::JSXElement(element) => {
                let value = self.resolve_jsx_name(path, &element.opening.name)?;
                let summary = match value {
                    Value::Boundary => {
                        let child = self.child_from_jsx_boundary(path, element)?;
                        self.summarize_child(path, child, params, &Form::Jsx((**element).clone()))?
                    }
                    Value::Function(inner) => {
                        self.compose_wrapper_jsx(path, element, params, &inner)?
                    }
                    _ => WrapperSummary::Ordinary,
                };
                Ok(summary)
            }
            Expr::Call(call) => {
                let Callee::Expr(callee) = &call.callee else {
                    return Ok(WrapperSummary::Ordinary);
                };
                match self.resolve_expr(path, callee)? {
                    Value::Boundary => {
                        let child = match call.args.first().map(|arg| unwrap_expr(&arg.expr)) {
                            Some(Expr::Object(object)) => self.child_from_object(path, object)?,
                            _ => Child::Dynamic("boundary wrapper props are dynamic".into()),
                        };
                        self.summarize_child(path, child, params, &Form::Call(call.clone()))
                    }
                    Value::Function(inner) => self.compose_wrapper_call(path, call, params, &inner),
                    Value::Factory(kind) => {
                        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none())
                        else {
                            return Ok(WrapperSummary::Ordinary);
                        };
                        match self.resolve_expr(path, &first.expr)? {
                            Value::Boundary => {
                                let child =
                                    self.child_from_call_props(path, call, kind == FactoryKind::H)?;
                                self.summarize_child(path, child, params, &Form::Call(call.clone()))
                            }
                            Value::Function(inner) => match self.summarize_wrapper(&inner)? {
                                WrapperSummary::Fixed(target) => Ok(WrapperSummary::Fixed(target)),
                                WrapperSummary::ForwardChild => {
                                    let child = self.child_from_call_props(
                                        path,
                                        call,
                                        kind == FactoryKind::H,
                                    )?;
                                    self.summarize_child(
                                        path,
                                        child,
                                        params,
                                        &Form::Call(call.clone()),
                                    )
                                }
                                other => Ok(other),
                            },
                            _ => Ok(WrapperSummary::Ordinary),
                        }
                    }
                    _ => Ok(WrapperSummary::Ordinary),
                }
            }
            _ => Ok(WrapperSummary::Ordinary),
        }
    }

    fn summarize_child(
        &mut self,
        path: &Path,
        child: Child,
        params: &[Pat],
        form: &Form,
    ) -> ScanResult<WrapperSummary> {
        match child {
            Child::Target(target) => Ok(WrapperSummary::Fixed(target)),
            // A failed factory proof must reach its boundary diagnostic. The
            // forwarding check can otherwise mistake its parameter read for
            // an unchanged child and defer this site.
            Child::Dynamic(reason)
                if reason.contains("has unsupported initializer")
                    && reason.contains("boundary-discovery-and-migration") =>
            {
                Ok(WrapperSummary::Unsupported(reason))
            }
            Child::Empty | Child::Dynamic(_) if self.form_forwards_child(path, form, params)? => {
                self.deferred_forward_sites
                    .insert((path.to_path_buf(), form.span().lo.0));
                Ok(WrapperSummary::ForwardChild)
            }
            Child::Empty => Ok(WrapperSummary::Unsupported(
                "boundary wrapper has no child".into(),
            )),
            Child::Dynamic(reason) => Ok(WrapperSummary::Unsupported(reason)),
        }
    }

    fn form_forwards_child(
        &mut self,
        path: &Path,
        form: &Form,
        params: &[Pat],
    ) -> ScanResult<bool> {
        let mut incoming_props = None;
        let mut incoming_child = None;
        if let Some(first) = params.first() {
            match first {
                Pat::Ident(binding) => incoming_props = Some(binding.id.to_id()),
                Pat::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            swc_core::ecma::ast::ObjectPatProp::Assign(assign)
                                if assign.key.sym == "children" =>
                            {
                                incoming_child = Some(assign.key.to_id());
                            }
                            swc_core::ecma::ast::ObjectPatProp::KeyValue(pair)
                                if prop_name(&pair.key).as_deref() == Some("children") =>
                            {
                                if let Pat::Ident(binding) = &*pair.value {
                                    incoming_child = Some(binding.id.to_id());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if self.form_has_unsafe_effects(path, form, params)? {
            return Ok(false);
        }
        match form {
            Form::Jsx(element) => {
                if !element.children.is_empty() {
                    let meaningful: Vec<_> = element
                        .children
                        .iter()
                        .filter(|child| !matches!(child, JSXElementChild::JSXText(text) if text.value.trim().is_empty()))
                        .collect();
                    if meaningful.len() != 1 {
                        return Ok(false);
                    }
                    return match meaningful[0] {
                        JSXElementChild::JSXExprContainer(container) => match &container.expr {
                            JSXExpr::Expr(expr) => {
                                self.forward_value(path, expr, &incoming_props, &incoming_child)
                            }
                            _ => Ok(false),
                        },
                        _ => Ok(false),
                    };
                }
                for attr in element.opening.attrs.iter().rev() {
                    match attr {
                        JSXAttrOrSpread::SpreadElement(spread) => {
                            return self.forward_value(
                                path,
                                &spread.expr,
                                &incoming_props,
                                &incoming_child,
                            );
                        }
                        JSXAttrOrSpread::JSXAttr(attr) if matches!(&attr.name, JSXAttrName::Ident(name) if name.sym == "children") =>
                        {
                            if let Some(JSXAttrValue::JSXExprContainer(container)) = &attr.value {
                                if let JSXExpr::Expr(expr) = &container.expr {
                                    return self.forward_value(
                                        path,
                                        expr,
                                        &incoming_props,
                                        &incoming_child,
                                    );
                                }
                            }
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
            }
            Form::Call(call) => {
                let factory_kind = match &call.callee {
                    Callee::Expr(callee) => match self.resolve_expr(path, callee)? {
                        Value::Factory(kind) => Some(kind),
                        _ => None,
                    },
                    _ => None,
                };
                if factory_kind == Some(FactoryKind::H) && call.args.len() > 2 {
                    return if call.args.len() == 3 && call.args[2].spread.is_none() {
                        self.forward_value(
                            path,
                            &call.args[2].expr,
                            &incoming_props,
                            &incoming_child,
                        )
                    } else {
                        Ok(false)
                    };
                }
                let props = call.args.get(if factory_kind.is_some() { 1 } else { 0 });
                if let Some(first) = props {
                    if self.forward_value(path, &first.expr, &incoming_props, &incoming_child)? {
                        return Ok(true);
                    }
                    if let Expr::Object(object) = unwrap_expr(&first.expr) {
                        for prop in object.props.iter().rev() {
                            match prop {
                                PropOrSpread::Spread(spread) => {
                                    return self.forward_value(
                                        path,
                                        &spread.expr,
                                        &incoming_props,
                                        &incoming_child,
                                    );
                                }
                                PropOrSpread::Prop(prop) => match &**prop {
                                    Prop::KeyValue(pair)
                                        if prop_name(&pair.key).as_deref() == Some("children") =>
                                    {
                                        return self.forward_value(
                                            path,
                                            &pair.value,
                                            &incoming_props,
                                            &incoming_child,
                                        );
                                    }
                                    Prop::KeyValue(pair)
                                        if matches!(&pair.key, PropName::Computed(_))
                                            && prop_name(&pair.key).is_none() =>
                                    {
                                        return Ok(false);
                                    }
                                    Prop::Shorthand(ident) if ident.sym == "children" => {
                                        return self.forward_value(
                                            path,
                                            &Expr::Ident(ident.clone()),
                                            &incoming_props,
                                            &incoming_child,
                                        );
                                    }
                                    _ => {}
                                },
                            }
                        }
                    }
                }
            }
            Form::Container(_) => {}
        }
        Ok(false)
    }

    fn forward_value(
        &mut self,
        path: &Path,
        expr: &Expr,
        incoming_props: &Option<swc_core::ecma::ast::Id>,
        incoming_child: &Option<swc_core::ecma::ast::Id>,
    ) -> ScanResult<bool> {
        let mut seen = HashSet::new();
        self.forward_value_inner(path, expr, incoming_props, incoming_child, &mut seen)
    }

    fn forward_value_inner(
        &mut self,
        path: &Path,
        expr: &Expr,
        incoming_props: &Option<swc_core::ecma::ast::Id>,
        incoming_child: &Option<swc_core::ecma::ast::Id>,
        seen: &mut HashSet<swc_core::ecma::ast::Id>,
    ) -> ScanResult<bool> {
        match unwrap_expr(expr) {
            Expr::Ident(ident) => {
                let id = ident.to_id();
                if incoming_props.as_ref() == Some(&id) || incoming_child.as_ref() == Some(&id) {
                    return Ok(true);
                }
                if !seen.insert(id) {
                    return Ok(false);
                }
                if let Some(init) = self.local_const_init(path, ident)? {
                    self.forward_value_inner(path, &init, incoming_props, incoming_child, seen)
                } else {
                    Ok(false)
                }
            }
            Expr::Member(member) if member.prop.is_ident_with("children") => {
                self.forward_value_inner(path, &member.obj, incoming_props, &None, seen)
            }
            _ => Ok(false),
        }
    }

    fn expression_uses_incoming(
        &mut self,
        path: &Path,
        expr: &Expr,
        incoming: &HashSet<swc_core::ecma::ast::Id>,
        seen: &mut HashSet<swc_core::ecma::ast::Id>,
    ) -> ScanResult<bool> {
        struct Idents(Vec<swc_core::ecma::ast::Ident>);
        impl Visit for Idents {
            fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
                self.0.push(ident.clone());
            }
        }
        let mut idents = Idents(Vec::new());
        expr.visit_with(&mut idents);
        for ident in idents.0 {
            let id = ident.to_id();
            if incoming.contains(&id) {
                return Ok(true);
            }
            if seen.insert(id) {
                if let Some(init) = self.local_const_init(path, &ident)? {
                    if self.expression_uses_incoming(path, &init, incoming, seen)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn form_has_unsafe_effects(
        &mut self,
        path: &Path,
        form: &Form,
        params: &[Pat],
    ) -> ScanResult<bool> {
        struct Incoming(HashSet<swc_core::ecma::ast::Id>);
        impl Visit for Incoming {
            fn visit_binding_ident(&mut self, binding: &swc_core::ecma::ast::BindingIdent) {
                self.0.insert(binding.id.to_id());
            }
        }
        let mut incoming = Incoming(HashSet::new());
        for param in params {
            param.visit_with(&mut incoming);
        }
        struct Effects {
            calls: Vec<CallExpr>,
            writes: bool,
        }
        impl Visit for Effects {
            fn visit_call_expr(&mut self, call: &CallExpr) {
                self.calls.push(call.clone());
                call.visit_children_with(self);
            }
            fn visit_assign_expr(&mut self, _: &swc_core::ecma::ast::AssignExpr) {
                self.writes = true;
            }
            fn visit_update_expr(&mut self, _: &swc_core::ecma::ast::UpdateExpr) {
                self.writes = true;
            }
        }
        let mut effects = Effects {
            calls: Vec::new(),
            writes: false,
        };
        match form {
            Form::Jsx(element) => element.visit_with(&mut effects),
            Form::Call(call) => call.visit_with(&mut effects),
            Form::Container(expr) => expr.visit_with(&mut effects),
        }
        if effects.writes {
            return Ok(true);
        }
        for call in effects.calls {
            if matches!(form, Form::Call(root) if root.span.lo == call.span.lo) {
                continue;
            }
            let Callee::Expr(callee) = &call.callee else {
                continue;
            };
            if matches!(self.resolve_expr(path, callee)?, Value::Factory(_)) {
                continue;
            }
            if self.expression_uses_incoming(path, callee, &incoming.0, &mut HashSet::new())? {
                return Ok(true);
            }
            for arg in &call.args {
                if self.expression_uses_incoming(
                    path,
                    &arg.expr,
                    &incoming.0,
                    &mut HashSet::new(),
                )? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn compose_wrapper_jsx(
        &mut self,
        path: &Path,
        element: &JSXElement,
        params: &[Pat],
        inner: &FunctionValue,
    ) -> ScanResult<WrapperSummary> {
        let inner_summary = self.summarize_wrapper(inner)?;
        let result = match inner_summary {
            WrapperSummary::Fixed(target) => Ok(WrapperSummary::Fixed(target)),
            WrapperSummary::ForwardChild => {
                let child = self.child_from_jsx_boundary(path, element)?;
                self.summarize_child(path, child, params, &Form::Jsx(element.clone()))
            }
            other => Ok(other),
        }?;
        if matches!(
            result,
            WrapperSummary::ForwardChild | WrapperSummary::Unsupported(_)
        ) {
            self.deferred_forward_sites
                .insert((path.to_path_buf(), element.span.lo.0));
        }
        Ok(result)
    }

    fn compose_wrapper_call(
        &mut self,
        path: &Path,
        call: &CallExpr,
        params: &[Pat],
        inner: &FunctionValue,
    ) -> ScanResult<WrapperSummary> {
        let inner_summary = self.summarize_wrapper(inner)?;
        let result = match inner_summary {
            WrapperSummary::Fixed(target) => Ok(WrapperSummary::Fixed(target)),
            WrapperSummary::ForwardChild => {
                let child = match call.args.first().map(|arg| unwrap_expr(&arg.expr)) {
                    Some(Expr::Object(object)) => self.child_from_object(path, object)?,
                    _ => Child::Dynamic("wrapper call props are dynamic".into()),
                };
                self.summarize_child(path, child, params, &Form::Call(call.clone()))
            }
            other => Ok(other),
        }?;
        if matches!(
            result,
            WrapperSummary::ForwardChild | WrapperSummary::Unsupported(_)
        ) {
            self.deferred_forward_sites
                .insert((path.to_path_buf(), call.span.lo.0));
        }
        Ok(result)
    }

    fn prime_wrapper_summaries(&mut self) -> ScanResult<()> {
        let paths: Vec<PathBuf> = self
            .modules
            .keys()
            .filter(|path| !self.primed_modules.contains(*path))
            .cloned()
            .collect();
        for path in paths {
            let module = self.module(&path)?;
            let mut names = Vec::new();
            let mut has_default = false;
            for item in &module.ast.body {
                if matches!(
                    item,
                    ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(_))
                        | ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_))
                ) {
                    has_default = true;
                }
                let decl = match item {
                    ModuleItem::Stmt(Stmt::Decl(decl)) => Some(decl),
                    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => Some(&export.decl),
                    _ => None,
                };
                if let Some(decl) = decl {
                    match decl {
                        Decl::Fn(function) => names.push(function.ident.clone()),
                        Decl::Var(variable) => {
                            for declarator in &variable.decls {
                                if let Pat::Ident(binding) = &declarator.name {
                                    names.push(binding.id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Retain the original top-level-first, then nested source order.
            names.extend(self.nested_bindings(&path)?.names.iter().cloned());
            let mut seen_names = HashSet::new();
            names.retain(|ident| seen_names.insert(ident.to_id()));
            for ident in names {
                if let Value::Function(function) = self.resolve_local(&path, &ident)? {
                    self.summarize_wrapper(&function)?;
                }
            }
            if has_default {
                if let Value::Function(function) = self.resolve_export(&path, "default")? {
                    self.summarize_wrapper(&function)?;
                }
            }
            self.primed_modules.insert(path);
        }
        Ok(())
    }

    fn find_client_route(
        &mut self,
        target: &FunctionValue,
    ) -> ScanResult<Option<(PathBuf, String)>> {
        let paths: Vec<PathBuf> = self.modules.keys().cloned().collect();
        let mut routes = Vec::new();
        for path in paths {
            let module = self.module(&path)?;
            if !module.client || is_virtual_scan_path(&path) {
                continue;
            }
            let mut names = BTreeSet::new();
            for item in &module.ast.body {
                let ModuleItem::ModuleDecl(decl) = item else {
                    continue;
                };
                match decl {
                    ModuleDecl::ExportDecl(export) => match &export.decl {
                        Decl::Fn(function) => {
                            names.insert(function.ident.sym.to_string());
                        }
                        Decl::Var(variable) => {
                            for declarator in &variable.decls {
                                if let Pat::Ident(binding) = &declarator.name {
                                    names.insert(binding.id.sym.to_string());
                                }
                            }
                        }
                        _ => {}
                    },
                    ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                        names.insert("default".into());
                    }
                    ModuleDecl::ExportNamed(named) if !named.type_only => {
                        for spec in &named.specifiers {
                            match spec {
                                ExportSpecifier::Named(spec) if !spec.is_type_only => {
                                    names.insert(module_export_name(
                                        spec.exported.as_ref().unwrap_or(&spec.orig),
                                    ));
                                }
                                ExportSpecifier::Namespace(spec) => {
                                    names.insert(module_export_name(&spec.name));
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            names.insert(target.definition.binding.clone());
            names.insert(target.marker.clone());
            for name in names {
                if let Value::Function(candidate) = self.resolve_export(&path, &name)? {
                    if candidate.definition == target.definition {
                        routes.push((path.clone(), name));
                    }
                }
            }
        }
        routes.sort();
        Ok(routes.into_iter().next())
    }
}

fn prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(text) => Some(atom_to_string(&text.value)),
        PropName::Computed(computed) => match unwrap_expr(&computed.expr) {
            Expr::Lit(Lit::Str(text)) => Some(atom_to_string(&text.value)),
            _ => None,
        },
        _ => None,
    }
}

fn line_column(source: Option<&str>, byte_position: u32) -> (usize, usize) {
    let offset = byte_position.saturating_sub(1) as usize;
    let prefix = source
        .map(|source| &source.as_bytes()[..offset.min(source.len())])
        .unwrap_or(&[]);
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column =
        String::from_utf8_lossy(prefix.rsplit(|byte| *byte == b'\n').next().unwrap_or(&[]))
            .chars()
            .count()
            + 1;
    (line, column)
}

fn body_return(
    body: Option<&BlockStmt>,
    params: Vec<Pat>,
    owned_factory_sites: &HashSet<u32>,
) -> Option<(Expr, Vec<Pat>)> {
    single_return(body, &params, owned_factory_sites).map(|expr| (expr, params))
}

fn single_return(
    body: Option<&BlockStmt>,
    params: &[Pat],
    owned_factory_sites: &HashSet<u32>,
) -> Option<Expr> {
    let body = body?;
    // A return after control flow is not proof that the boundary forwards
    // the original child. Permit only immutable local values before a final
    // return; declarations that write during initialization are not aliases.
    let (last, preceding) = body.stmts.split_last()?;
    let Stmt::Return(returned) = last else {
        return None;
    };
    struct ParamBindings(HashSet<swc_core::ecma::ast::Id>);
    impl Visit for ParamBindings {
        fn visit_binding_ident(&mut self, binding: &swc_core::ecma::ast::BindingIdent) {
            self.0.insert(binding.id.to_id());
        }
    }
    let mut bindings = ParamBindings(HashSet::new());
    for param in params {
        param.visit_with(&mut bindings);
    }
    struct ParamUse<'a> {
        bindings: &'a HashSet<swc_core::ecma::ast::Id>,
        used: bool,
    }
    impl Visit for ParamUse<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            self.used |= self.bindings.contains(&ident.to_id());
        }
    }
    struct Writes<'a> {
        bindings: &'a HashSet<swc_core::ecma::ast::Id>,
        owned_factory_sites: &'a HashSet<u32>,
        unsafe_effect: bool,
    }
    impl Visit for Writes<'_> {
        fn visit_assign_expr(&mut self, _: &swc_core::ecma::ast::AssignExpr) {
            self.unsafe_effect = true;
        }
        fn visit_update_expr(&mut self, _: &swc_core::ecma::ast::UpdateExpr) {
            self.unsafe_effect = true;
        }
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if !self.owned_factory_sites.contains(&call.span.lo.0) {
                for arg in &call.args {
                    let mut usage = ParamUse {
                        bindings: self.bindings,
                        used: false,
                    };
                    arg.expr.visit_with(&mut usage);
                    if usage.used {
                        self.unsafe_effect = true;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    for statement in preceding {
        match statement {
            Stmt::Empty(_) => {}
            Stmt::Decl(Decl::Var(variable))
                if variable.kind == swc_core::ecma::ast::VarDeclKind::Const =>
            {
                for declaration in &variable.decls {
                    let mut writes = Writes {
                        bindings: &bindings.0,
                        owned_factory_sites,
                        unsafe_effect: false,
                    };
                    declaration.visit_with(&mut writes);
                    if writes.unsafe_effect {
                        return None;
                    }
                    if let (Pat::Ident(binding), Some(init)) =
                        (&declaration.name, &declaration.init)
                    {
                        let mut usage = ParamUse {
                            bindings: &bindings.0,
                            used: false,
                        };
                        init.visit_with(&mut usage);
                        if usage.used {
                            bindings.0.insert(binding.id.to_id());
                        }
                    }
                }
            }
            _ => return None,
        }
    }
    returned.arg.as_ref().map(|expr| (**expr).clone())
}

fn arrow_return(arrow: &ArrowExpr, owned_factory_sites: &HashSet<u32>) -> Option<(Expr, Vec<Pat>)> {
    let expr = match &*arrow.body {
        swc_core::ecma::ast::BlockStmtOrExpr::Expr(expr) => (**expr).clone(),
        swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
            single_return(Some(body), &arrow.params, owned_factory_sites)?
        }
    };
    Some((expr, arrow.params.clone()))
}

pub(super) fn discover<R: Resolver>(
    resolver: &R,
    modules: BTreeMap<PathBuf, (Module, String)>,
) -> ScanResult<(IslandsSet, Vec<PathBuf>)> {
    let initial_paths: BTreeSet<PathBuf> = modules.keys().cloned().collect();
    let modules = modules
        .into_iter()
        .map(|(path, (module, source))| {
            let client = has_use_client_directive(&module);
            let (ast, _) = resolve_worker_bindings(module);
            (
                path,
                SourceModule {
                    ast,
                    source,
                    client,
                },
            )
        })
        .collect();
    let mut discovery = Discovery::new(resolver, modules);
    let mut targets: BTreeMap<Definition, (FunctionValue, PathBuf, Span)> = BTreeMap::new();
    let mut scanned = BTreeSet::new();
    let mut expanded_demanded = HashSet::new();
    let mut primed_client_reexports = HashSet::new();
    loop {
        // Resolving importer callees and namespace references can demand
        // modules across the ordinary package traversal gate. Collect each
        // module's facts once, reaching a fixed module set before any proof.
        loop {
            // Client route selection follows re-exports. Demand those
            // sources before proving a factory, including when its first
            // tentative proof would otherwise fail before route lookup.
            let client_paths: Vec<_> = discovery
                .modules
                .keys()
                .filter(|path| !primed_client_reexports.contains(*path))
                .cloned()
                .collect();
            for path in client_paths {
                primed_client_reexports.insert(path.clone());
                let module = discovery.module(&path)?;
                if !module.client {
                    continue;
                }
                let sources: Vec<_> = module
                    .ast
                    .body
                    .iter()
                    .filter_map(|item| match item {
                        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)) => {
                            named.src.as_ref().map(|src| atom_to_string(&src.value))
                        }
                        ModuleItem::ModuleDecl(ModuleDecl::ExportAll(all)) if !all.type_only => {
                            Some(atom_to_string(&all.src.value))
                        }
                        _ => None,
                    })
                    .collect();
                for source in sources {
                    if let Some(target) = discovery.resolve_source(&path, &source) {
                        if is_scannable_source(&target) {
                            discovery.module(&target)?;
                        }
                    }
                }
            }
            // The scanner later walks every import of a demanded module for
            // ScanMeta. Bring that same closure into registration before a
            // factory proof can be accepted. Each demanded root is expanded
            // once, even when a proof demands another module later.
            let fresh_demanded: Vec<_> = discovery
                .modules
                .keys()
                .filter(|path| !initial_paths.contains(*path) && !expanded_demanded.contains(*path))
                .cloned()
                .collect();
            if !fresh_demanded.is_empty() {
                expanded_demanded.extend(fresh_demanded.iter().cloned());
                let closure = scan_reachable_modules_with_meta(&fresh_demanded, resolver)?;
                expanded_demanded.extend(closure.modules.iter().cloned());
                for path in closure.modules {
                    if is_scannable_source(&path) {
                        discovery.module(&path)?;
                    }
                }
            }
            let unaudited: Vec<_> = discovery
                .modules
                .keys()
                .filter(|path| !discovery.audited_modules.contains(*path))
                .cloned()
                .collect();
            if unaudited.is_empty() && fresh_demanded.is_empty() {
                break;
            }
            for path in unaudited {
                discovery.audit_factory_references(&path)?;
            }
        }
        let paths: Vec<PathBuf> = discovery
            .modules
            .keys()
            .filter(|path| !scanned.contains(*path))
            .cloned()
            .collect();
        if paths.is_empty() {
            // Route lookup can follow a client re-export and demand another
            // module. Audit its import closure before accepting any cached
            // factory proof from this pass.
            for (target, _, _) in targets.values() {
                discovery.find_client_route(target)?;
            }
            if discovery
                .modules
                .keys()
                .any(|path| !discovery.audited_modules.contains(path))
            {
                discovery.factory_proofs.clear();
                discovery.summary_cache.clear();
                discovery.primed_modules.clear();
                discovery.deferred_forward_sites.clear();
                targets.clear();
                scanned.clear();
                continue;
            }
            break;
        }
        let mut first_error = discovery.prime_wrapper_summaries().err();
        for path in paths {
            scanned.insert(path.clone());
            let module = discovery.module(&path)?;
            let mut collector = FormCollector::default();
            module.ast.visit_with(&mut collector);
            for form in collector.forms {
                match discovery.resolve_form(&path, &form) {
                    Ok(Some(target)) => {
                        targets.entry(target.definition.clone()).or_insert((
                            target,
                            path.clone(),
                            form.span(),
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }
        if discovery
            .modules
            .keys()
            .any(|path| !discovery.audited_modules.contains(path))
        {
            // A target or wrapper demanded another module. Re-evaluate all
            // forms against the expanded call/reference closure.
            discovery.factory_proofs.clear();
            discovery.summary_cache.clear();
            discovery.primed_modules.clear();
            discovery.deferred_forward_sites.clear();
            targets.clear();
            scanned.clear();
            continue;
        }
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    let mut selected = Vec::new();
    let mut markers: BTreeMap<String, (Definition, PathBuf)> = BTreeMap::new();
    for (definition, (target, use_path, use_span)) in targets {
        let route = discovery
            .find_client_route(&target)?
            .or(target.route.clone());
        let Some((source_path, component_name)) = route else {
            return Err(discovery.diagnostic(
                &use_path,
                use_span,
                format!(
                    "target {} defined at {} has no runtime export through a \"use client\" module",
                    definition.binding,
                    discovery.definition_location(&definition),
                ),
            ));
        };
        if let Some((previous, previous_use)) = markers.insert(
            target.marker.clone(),
            (definition.clone(), use_path.clone()),
        ) {
            if previous != definition {
                return Err(discovery.diagnostic(
                    &use_path,
                    use_span,
                    format!(
                        "ambiguous owned island marker {:?}: {} ({}) used at {} and {} ({}); rename one defining function",
                        target.marker,
                        discovery.definition_location(&previous),
                        previous.binding,
                        previous_use.display(),
                        discovery.definition_location(&definition),
                        definition.binding,
                    ),
                ));
            }
        }
        selected.push(Island::with_marker_name(
            component_name,
            source_path,
            target.marker,
        ));
    }
    selected.sort_by(|a, b| {
        (&a.source_path, &a.component_name).cmp(&(&b.source_path, &b.component_name))
    });
    let demanded_paths = discovery
        .modules
        .keys()
        .filter(|path| !initial_paths.contains(*path))
        .cloned()
        .collect();
    Ok((selected, demanded_paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(files: &[(&str, &str)]) -> ScanResult<IslandsSet> {
        let mut resolver = InMemoryResolver::new();
        for (path, source) in files {
            resolver.insert(PathBuf::from(format!("/proj/{path}")), *source);
        }
        scan_islands(&[PathBuf::from("/proj/pages/home.tsx")], &resolver)
    }

    fn markers(islands: &[Island]) -> Vec<String> {
        islands
            .iter()
            .map(|island| island.marker_name.clone())
            .collect()
    }

    #[test]
    fn packed_module_wrapper_priming_has_a_linear_ast_budget() {
        const FUNCTIONS: usize = 96;
        let path = PathBuf::from("/proj/packed.tsx");
        let mut source =
            String::from("import { Island } from '@takazudo/zfb'; function outer() {\n");
        for index in 0..FUNCTIONS {
            source.push_str(&format!(
                "function wrapper{index}({{ children }}) {{ return <Island>{{children}}</Island>; }}\n"
            ));
        }
        source.push_str("return null; }");
        let (ast, _) = resolve_worker_bindings(parse_module(&path, &source).unwrap());
        let resolver = InMemoryResolver::new().with_file(path.clone(), source.clone());
        let mut discovery = Discovery::new(
            &resolver,
            BTreeMap::from([(
                path.clone(),
                SourceModule {
                    ast,
                    source,
                    client: false,
                },
            )]),
        );
        discovery.prime_wrapper_summaries().unwrap();
        for index in 0..FUNCTIONS {
            assert!(discovery.summary_cache.iter().any(|(definition, summary)| {
                definition.binding == format!("wrapper{index}")
                    && matches!(summary, WrapperSummary::ForwardChild)
            }));
        }
        assert_eq!(discovery.deferred_forward_sites.len(), FUNCTIONS);
        let first_pass = (
            discovery.nested_binding_expression_visits,
            discovery.function_return_expression_visits,
        );
        assert!(first_pass.0 > 0 && first_pass.1 > 0);
        assert!(
            first_pass.0 + first_pass.1 < FUNCTIONS * 8,
            "AST visits: {first_pass:?}"
        );
        // Discovery revisits priming when demanded imports load more modules.
        // Already-primed modules must retain their summaries without AST work.
        discovery.prime_wrapper_summaries().unwrap();
        assert_eq!(
            first_pass,
            (
                discovery.nested_binding_expression_visits,
                discovery.function_return_expression_visits,
            )
        );
        assert_eq!(discovery.deferred_forward_sites.len(), FUNCTIONS);
        let later_path = PathBuf::from("/proj/later.tsx");
        let later_source = "import { Island } from '@takazudo/zfb'; export function Later({ children }) { return <Island>{children}</Island>; }";
        let (ast, _) = resolve_worker_bindings(parse_module(&later_path, later_source).unwrap());
        discovery.modules.insert(
            later_path,
            Rc::new(SourceModule {
                ast,
                source: later_source.into(),
                client: false,
            }),
        );
        discovery.prime_wrapper_summaries().unwrap();
        assert!(discovery.summary_cache.iter().any(|(definition, summary)| {
            definition.binding == "Later" && matches!(summary, WrapperSummary::ForwardChild)
        }));
        assert_eq!(discovery.deferred_forward_sites.len(), FUNCTIONS + 1);
        let extra_visits = discovery.nested_binding_expression_visits
            + discovery.function_return_expression_visits
            - first_pass.0
            - first_pass.1;
        assert!(
            extra_visits > 0 && extra_visits < 16,
            "new module AST visits: {extra_visits}"
        );
    }

    #[test]
    fn return_index_preserves_supported_default_wrapper_forms() {
        for definition in [
            "export default function ({ children }) { return <Island>{children}</Island>; }",
            "export default ({ children }) => <Island>{children}</Island>;",
            "const Wrap = function ({ children }) { return <Island>{children}</Island>; }; export default Wrap;",
            "function Wrap({ children }) { return <Island>{children}</Island>; } export default Wrap;",
        ] {
            let wrapper = format!("import {{ Island }} from '@takazudo/zfb'; {definition}");
            let islands = scan(&[
                ("pages/home.tsx", "import Wrap from '../wrapper'; import { Counter } from '../counter'; export default function Page() { return <Wrap><Counter /></Wrap>; }"),
                ("wrapper.tsx", &wrapper),
                ("counter.tsx", "'use client'; export function Counter() { return null; }"),
            ]).unwrap();
            assert_eq!(markers(&islands), ["Counter"], "{definition}");
        }
    }

    #[test]
    fn dependency_member_target_registers_the_proven_function() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                "import '../factory'; export default function Page() { return <html />; }",
            ),
            (
                "factory.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Panel } from './panel';
                function createPanel(deps) {
                    const Target = deps.Panel;
                    function PanelIsland() { return <Island><Target /></Island>; }
                    return PanelIsland;
                }
                createPanel({ flag: true, Panel });
            "#,
            ),
            (
                "panel.tsx",
                "'use client'; export function Panel() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Panel"]);
        assert_eq!(islands[0].source_path, PathBuf::from("/proj/panel.tsx"));
    }

    #[test]
    fn dependency_member_spread_keeps_a_source_located_diagnostic() {
        let error = scan(&[
            ("pages/home.tsx", "import '../factory';"),
            (
                "factory.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Panel } from './panel';
                function createPanel(deps) {
                    const Target = deps.Panel;
                    return <Island><Target /></Island>;
                }
                createPanel({ ...extra, Panel });
            "#,
            ),
            (
                "panel.tsx",
                "'use client'; export function Panel() { return null; }",
            ),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains("/proj/factory.tsx:"), "{error}");
        assert!(
            error.contains("target Target has unsupported initializer"),
            "{error}"
        );
        assert!(error.contains("passes a non-literal argument"), "{error}");
    }

    fn factory_case(body: &str, call: &str) -> ScanResult<IslandsSet> {
        let source = format!(
            "import {{ Island }} from '@takazudo/zfb'; import {{ Counter, Other }} from './counter';\n{body}\n{call}"
        );
        scan(&[
            ("pages/home.tsx", "import '../factory';"),
            ("factory.tsx", &source),
            ("counter.tsx", "'use client'; export function Counter() { return null; } export function Other() { return null; }"),
        ])
    }

    #[test]
    fn factory_member_binding_forms_keep_counter_identity() {
        for body in [
            "function create(deps) { const Target = deps.Counter; if (!Target) return null; return <Island><Target /></Island>; }",
            "function create(deps) { const Target = deps[\"Counter\"]; return <Island><Target /></Island>; }",
            "function create(deps) { const Target = deps.Counter as unknown as Function; return <Island><Target /></Island>; }",
            "function create(deps) { const { Counter: Target } = deps; return <Island><Target /></Island>; }",
            "function create({ Counter: Target }) { return <Island><Target /></Island>; }",
            "function create({ Counter }) { return <Island><Counter /></Island>; }",
            "const create = (deps) => { const Target = deps.Counter; return <Island><Target /></Island>; };",
        ] {
            let islands = factory_case(body, "create({ flag: true, Counter });").unwrap();
            assert_eq!(markers(&islands), ["Counter"], "{body}");
            assert_eq!(islands[0].source_path, PathBuf::from("/proj/counter.tsx"));
        }
        let islands = factory_case(
            "function create({ Target }) { return <Island><Target /></Island>; }",
            "create({ Target: Counter });",
        )
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        let islands = factory_case(
            "function create(deps) { const { Target } = deps; return <Island><Target /></Island>; }",
            "create({ Target: Counter });",
        )
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        let islands = factory_case(
            "function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }",
            "create({ 1: true, Counter });",
        )
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn factory_member_registers_owned_call_children_once() {
        for boundary in [
            "Island({ children: jsx(Target, {}) })",
            "h(Island, { children: h(Target, {}) })",
        ] {
            let source = format!(
                "import {{ Island }} from '@takazudo/zfb'; import {{ jsx }} from '@takazudo/zfb/zudo-react/jsx-runtime'; import {{ h }} from '@takazudo/zfb/zudo-react'; import {{ Counter }} from './counter'; function create(deps) {{ const Target = deps.Counter; return {boundary}; }} create({{ Counter }}); create({{ Counter }}); <Island><Counter /></Island>;"
            );
            let islands = scan(&[
                ("pages/home.tsx", "import '../factory';"),
                ("factory.tsx", &source),
                (
                    "counter.tsx",
                    "'use client'; export function Counter() { return null; }",
                ),
            ])
            .unwrap();
            assert_eq!(markers(&islands), ["Counter"], "{boundary}");
        }
    }

    #[test]
    fn nested_boundary_uses_ancestor_factory_parameter() {
        let islands = factory_case(
            "function create(deps) { function Nested() { const Target = deps.Counter; return <Island><Target /></Island>; } return Nested; }",
            "create({ Counter });",
        ).unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn factory_member_proof_is_memoized_across_200_call_sites() {
        struct CountingResolver {
            inner: InMemoryResolver,
            demands: std::cell::Cell<usize>,
        }
        impl Resolver for CountingResolver {
            fn resolve(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.inner.resolve(dir, specifier)
            }
            fn resolve_demanded(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.demands.set(self.demands.get() + 1);
                self.inner.resolve(dir, specifier)
            }
            fn read(&self, path: &Path) -> Result<String, String> {
                self.inner.read(path)
            }
        }
        let path = PathBuf::from("/proj/factory.tsx");
        let mut source = String::from("import { Counter } from './counter'; function create(deps) { const Target = deps.Counter; return Target; }\n");
        for _ in 0..200 {
            source.push_str("create({ Counter });\n");
        }
        let (ast, _) = resolve_worker_bindings(parse_module(&path, &source).unwrap());
        let resolver = CountingResolver {
            inner: InMemoryResolver::new()
                .with_file(path.clone(), source.clone())
                .with_file(
                    "/proj/counter.tsx",
                    "'use client'; export function Counter() { return null; }",
                ),
            demands: std::cell::Cell::new(0),
        };
        let mut discovery = Discovery::new(
            &resolver,
            BTreeMap::from([(
                path.clone(),
                SourceModule {
                    ast,
                    source,
                    client: false,
                },
            )]),
        );
        discovery.audit_factory_references(&path).unwrap();
        let target = discovery
            .nested_bindings(&path)
            .unwrap()
            .names
            .iter()
            .find(|ident| ident.sym == "Target")
            .unwrap()
            .clone();
        assert_eq!(
            discovery
                .nested_bindings(&path)
                .unwrap()
                .calls
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            200
        );
        for _ in 0..3 {
            assert!(matches!(
                discovery.resolve_local(&path, &target).unwrap(),
                Value::Function(_)
            ));
        }
        assert_eq!(discovery.factory_proof_evaluations, 1);
        assert!(discovery.nested_binding_expression_visits < 200 * 10);
        assert!(discovery.factory_resolver_operations < 200 * 5);
        assert!(resolver.demands.get() < 200 * 4);
    }

    #[test]
    fn factory_member_cache_does_not_skip_later_target_write() {
        for (first, second) in [("A", "B"), ("B", "A")] {
            let body = format!(
                "function create(deps) {{ const A = deps.Counter; const B = deps.Counter; B = Other; <Island><{first} /></Island>; <Island><{second} /></Island>; }}"
            );
            let error = factory_case(&body, "create({ Counter });")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("parameter B is written or escapes"),
                "{error}"
            );
        }
    }

    #[test]
    fn demanded_module_side_effect_closure_joins_factory_proof() {
        struct DemandOnlyResolver(InMemoryResolver);
        impl Resolver for DemandOnlyResolver {
            fn resolve(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.0.resolve(dir, specifier)
            }
            fn resolve_demanded(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                if specifier == "late-pkg" {
                    Some(PathBuf::from("/proj/factory.tsx"))
                } else {
                    self.0.resolve(dir, specifier)
                }
            }
            fn read(&self, path: &Path) -> Result<String, String> {
                self.0.read(path)
            }
        }
        let factory = "import './late'; import { Island } from '@takazudo/zfb'; export function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }";
        for (late, expected) in [
            ("import { create } from './factory'; import { Other } from './counter'; create({ Counter: Other });", "call sites disagree"),
            ("import { create } from './factory'; use(create);", "factory create escapes as a value"),
        ] {
            let resolver = DemandOnlyResolver(InMemoryResolver::new()
                .with_file("/proj/pages/home.tsx", "import { create } from 'late-pkg'; import { Counter } from '../counter'; create({ Counter });")
                .with_file("/proj/factory.tsx", factory)
                .with_file("/proj/late.tsx", late)
                .with_file("/proj/counter.tsx", "'use client'; export function Counter() { return null; } export function Other() { return null; }"));
            let error = scan_islands(&[PathBuf::from("/proj/pages/home.tsx")], &resolver)
                .unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("/proj/factory.tsx:"), "{error}");
        }
    }

    #[test]
    fn client_route_reexport_closure_joins_factory_proof() {
        struct RouteDemandResolver(InMemoryResolver);
        impl Resolver for RouteDemandResolver {
            fn resolve(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.0.resolve(dir, specifier)
            }
            fn resolve_demanded(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                if specifier == "route-pkg" {
                    Some(PathBuf::from("/proj/bridge.ts"))
                } else {
                    self.0.resolve(dir, specifier)
                }
            }
            fn read(&self, path: &Path) -> Result<String, String> {
                self.0.read(path)
            }
        }
        let resolver = RouteDemandResolver(InMemoryResolver::new()
            .with_file("/proj/pages/home.tsx", "import '../route'; import { create } from '../factory'; import { Counter } from '../counter'; create({ Counter });")
            .with_file("/proj/route.ts", "'use client'; export { Counter } from 'route-pkg';")
            .with_file("/proj/bridge.ts", "import './late'; export { Counter } from './counter';")
            .with_file("/proj/late.ts", "import { create } from './factory'; use(create);")
            .with_file("/proj/factory.tsx", "import { Island } from '@takazudo/zfb'; export function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }")
            .with_file("/proj/counter.tsx", "'use client'; export function Counter() { return null; }"));
        let error = scan_islands(&[PathBuf::from("/proj/pages/home.tsx")], &resolver)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("factory create escapes as a value"),
            "{error}"
        );
        assert!(error.contains("/proj/late.ts:"), "{error}");
    }

    #[test]
    fn factory_member_rejections_name_the_failed_proof() {
        let body = "function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }";
        let mut mismatches = Vec::new();
        for (body, call, reason) in [
            (body, "", "factory create has no call site"),
            (body, "create(value);", "passes a non-literal argument"),
            (body, "create({});", "property Counter missing"),
            (body, "create();", "passes too few arguments"),
            (body, "create(...args);", "spreads positional arguments"),
            (body, "create({ [\"Counter\"]: Counter });", "passes a non-literal argument"),
            (body, "create({ Counter: memo(Counter) });", "memo(...) is a call result"),
            (body, "create({ Counter: flag ? Counter : Other });", "selects Counter conditionally"),
            (body, "create({ Counter: Counter ?? Other });", "selects Counter conditionally"),
            (body, "create({ Counter: Counter || Other });", "selects Counter conditionally"),
            (body, "create({ Counter }); create({ Counter: Other });", "call sites disagree"),
            (body, "create({ Counter }); use(create);", "factory create escapes as a value"),
            (body, "create({ Counter }); create.call(null, { Counter });", "factory create escapes as a value"),
            (body, "create({ Counter }); const stored = { create };", "factory create escapes as a value"),
            ("function create(deps) { deps.Counter = Other; const Target = deps.Counter; return <Island><Target /></Island>; }", "create({ Counter });", "parameter deps is written or escapes"),
            ("function create(deps) { delete deps.Counter; const Target = deps.Counter; return <Island><Target /></Island>; }", "create({ Counter });", "parameter deps is written or escapes"),
            ("function create(deps) { const d = deps; const Target = deps.Counter; return <Island><Target /></Island>; }", "create({ Counter });", "parameter deps is written or escapes"),
            ("function create(deps) { use(deps); const Target = deps.Counter; return <Island><Target /></Island>; }", "create({ Counter });", "parameter deps is written or escapes"),
            ("function create({ Counter: Target }) { Target = Other; return <Island><Target /></Island>; }", "create({ Counter });", "parameter Target is written or escapes"),
            ("function create(deps = {}) { const Target = deps.Counter; return <Island><Target /></Island>; }", "create({ Counter });", "parameter deps has a default value"),
            ("function create({ Counter: Target, ...rest }) { return <Island><Target /></Island>; }", "create({ Counter });", "uses a rest pattern"),
        ] {
            match factory_case(body, call) {
                Ok(islands) => mismatches.push(format!(
                    "{call:?} / {reason}: expected a diagnostic, registered {:?}",
                    markers(&islands)
                )),
                Err(error) => {
                    let error = error.to_string();
                    if !error.contains(reason) {
                        mismatches.push(format!("{call:?} / {reason}: {error}"));
                    }
                    if !error.contains("boundary-discovery-and-migration") {
                        mismatches.push(format!("{call:?} / missing seam anchor: {error}"));
                    }
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }

    #[test]
    fn factory_member_rejection_precedence_spans_all_calls() {
        let body = "function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }";
        for (calls, reason) in [
            ("create(...args); create();", "passes too few arguments"),
            (
                "create({}); create(value);",
                "passes a non-literal argument",
            ),
            (
                "create({ Counter }); create({ Counter: Other }); create({ Counter: 1 });",
                "is not a function binding",
            ),
        ] {
            let error = factory_case(body, calls).unwrap_err().to_string();
            assert!(error.contains(reason), "{calls}: {error}");
        }
    }

    #[test]
    fn factory_used_as_jsx_is_a_value_escape() {
        let error = factory_case(
            "function Create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }",
            "Create({ Counter }); <Create />;",
        ).unwrap_err().to_string();
        assert!(
            error.contains("factory Create escapes as a value"),
            "{error}"
        );
    }

    #[test]
    fn ordinary_boundary_wrapper_shorthand_keeps_opaque_object_diagnostic() {
        let error = scan(&[
            ("pages/home.tsx", "import '../factory';"),
            (
                "factory.tsx",
                "import { Island } from '@takazudo/zfb'; import { Counter } from './counter'; function Wrap() { return <Island><Counter /></Island>; } const stored = { Wrap };",
            ),
            ("counter.tsx", "'use client'; export function Counter() { return null; }"),
        ])
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("boundary wrapper escapes into an opaque object"),
            "{error}"
        );
    }

    #[test]
    fn exported_factory_accepts_cross_module_calls() {
        let body = "import { Island } from '@takazudo/zfb'; export function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }";
        for factory_import in [
            "import { create } from '../factory';",
            "import { create } from '../barrel';",
            "import * as F from '../barrel';",
        ] {
            let call = if factory_import.contains("* as F") {
                "F.create({ Counter });"
            } else {
                "create({ Counter });"
            };
            let page = format!("{factory_import} import {{ Counter }} from '../counter'; {call}");
            let islands = scan(&[
                ("pages/home.tsx", &format!("import './other'; {page}")),
                ("pages/other.tsx", &page),
                ("factory.tsx", body),
                ("barrel.ts", "export { create } from './factory';"),
                (
                    "counter.tsx",
                    "'use client'; export function Counter() { return null; }",
                ),
            ])
            .unwrap();
            assert_eq!(markers(&islands), ["Counter"]);
        }
    }

    #[test]
    fn cross_module_rejections_include_late_calls_and_escapes() {
        let factory = "import { Island } from '@takazudo/zfb'; export function create(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }";
        let counter = "'use client'; export function Counter() { return null; } export function Other() { return null; }";
        for (extra, expected) in [
            ("create({ Counter: Other });", "call sites disagree"),
            ("use(create);", "factory create escapes as a value"),
            (
                "create({ ...extra, Counter });",
                "passes a non-literal argument",
            ),
        ] {
            let first = "import { create } from '../factory'; import { Counter } from '../counter'; create({ Counter });";
            let late = format!("import {{ create }} from '../factory'; import {{ Counter, Other }} from '../counter'; {extra}");
            for (a, b) in [(first, late.as_str()), (late.as_str(), first)] {
                let error = scan(&[
                    ("pages/home.tsx", &format!("import './other'; {a}")),
                    ("pages/other.tsx", b),
                    ("factory.tsx", factory),
                    ("counter.tsx", counter),
                ])
                .unwrap_err()
                .to_string();
                assert!(error.contains(expected), "{extra}: {error}");
            }
        }
    }

    #[test]
    fn cross_module_factory_cycle_has_a_finite_diagnostic() {
        let error = scan(&[
            ("pages/home.tsx", "import { left } from '../left'; import { right } from '../right'; left({ Counter: right }); right({ Counter: left });"),
            ("left.tsx", "import { Island } from '@takazudo/zfb'; export function left(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }"),
            ("right.tsx", "import { Island } from '@takazudo/zfb'; export function right(deps) { const Target = deps.Counter; return <Island><Target /></Island>; }"),
        ]).unwrap_err().to_string();
        assert!(
            error.contains("escapes as a value") || error.contains("recursive proof cycle"),
            "{error}"
        );
    }

    #[test]
    fn destructured_declarators_obey_const_and_whole_pattern_shape() {
        for (declaration, reason) in [
            (
                "let { Counter: Target } = deps;",
                "mutable target binding Target",
            ),
            (
                "var { Counter: Target } = deps;",
                "mutable target binding Target",
            ),
            (
                "const { Counter: Target = Other } = deps;",
                "target Target has unsupported initializer",
            ),
            (
                "const { Counter: Target, ...rest } = deps;",
                "target Target has unsupported initializer",
            ),
            (
                "const { Counter: Target, nested: { Other: Nested } } = deps;",
                "target Target has unsupported initializer",
            ),
        ] {
            let body = format!(
                "function create(deps) {{ {declaration} return <Island><Target /></Island>; }}"
            );
            let error = factory_case(&body, "create({ Counter });")
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{declaration}: {error}");
        }
    }

    #[test]
    fn packed_module_nested_binding_search_has_a_linear_ast_budget() {
        const FUNCTIONS: usize = 96;
        let path = PathBuf::from("/proj/packed.ts");
        let mut source = String::from("function outer() {\n");
        for index in 0..FUNCTIONS {
            source.push_str(&format!(
                "function helper{index}(callback{index}) {{ const alias{index} = callback{index}; return missing{index}(alias{index}); }}\n"
            ));
        }
        source.push_str("return null; }");
        let (ast, _) = resolve_worker_bindings(parse_module(&path, &source).unwrap());
        let resolver = InMemoryResolver::new().with_file(path.clone(), source.clone());
        let mut discovery = Discovery::new(
            &resolver,
            BTreeMap::from([(
                path.clone(),
                SourceModule {
                    ast,
                    source,
                    client: false,
                },
            )]),
        );
        struct Names(Vec<swc_core::ecma::ast::Ident>);
        impl Visit for Names {
            fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
                self.0.push(ident.clone());
            }
        }
        let mut names = Names(Vec::new());
        discovery.module(&path).unwrap().ast.visit_with(&mut names);
        let mut seen = HashSet::new();
        names.0.retain(|ident| seen.insert(ident.to_id()));
        for _ in 0..3 {
            let mut functions = 0;
            let mut missing = 0;
            for ident in &names.0 {
                let value = discovery.resolve_local(&path, ident).unwrap();
                if ident.sym.starts_with("helper") {
                    let Value::Function(function) = value else {
                        panic!("nested helper must retain its function identity");
                    };
                    assert_eq!(function.marker, ident.sym.to_string());
                    functions += 1;
                } else if ident.sym.starts_with("missing")
                    || ident.sym.starts_with("callback")
                    || ident.sym.starts_with("alias")
                {
                    assert!(matches!(value, Value::Other));
                    missing += 1;
                }
            }
            assert_eq!(functions, FUNCTIONS);
            assert_eq!(missing, FUNCTIONS * 3);
        }
        // Measure AST work, not elapsed time. Parameters, globals and aliases
        // must not each walk every packed function again, including on misses.
        assert!(discovery.nested_binding_expression_visits > 0);
        assert!(
            discovery.nested_binding_expression_visits < FUNCTIONS * 8,
            "{} AST expression visits for {FUNCTIONS} packed functions",
            discovery.nested_binding_expression_visits,
        );
    }

    #[test]
    fn indexed_nested_wrapper_alias_keeps_parameter_shadowing() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Counter } from '../counter';
                export default function Page() {
                    const Fixed = () => <Island><Counter /></Island>;
                    const Alias = Fixed;
                    function Ordinary(Alias) { return <Alias />; }
                    return <Alias />;
                }
            "#,
            ),
            (
                "counter.tsx",
                "'use client'; export function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn indexed_nested_alias_cycle_keeps_a_finite_diagnostic() {
        let error = scan(&[(
            "pages/home.tsx",
            r#"
            import { Island } from '@takazudo/zfb';
            import { h } from '@takazudo/zfb/zudo-react';
            export default function Page() {
                const First = Second;
                const Second = First;
                return h(Island, { children: h(First, {}) });
            }
        "#,
        )])
        .unwrap_err()
        .to_string();
        assert!(error.contains("recursive binding"), "{error}");
        assert!(error.contains("/proj/pages/home.tsx:"), "{error}");
    }

    #[test]
    fn packed_module_factory_resolution_scales_with_call_sites() {
        struct CountingResolver {
            inner: InMemoryResolver,
            demands: std::cell::Cell<usize>,
        }
        impl Resolver for CountingResolver {
            fn resolve(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.inner.resolve(dir, specifier)
            }
            fn resolve_demanded(&self, dir: &Path, specifier: &str) -> Option<PathBuf> {
                self.demands.set(self.demands.get() + 1);
                self.inner.resolve(dir, specifier)
            }
            fn read(&self, path: &Path) -> Result<String, String> {
                self.inner.read(path)
            }
        }
        const FUNCTIONS: usize = 48;
        let mut source = String::from(
            "import { h } from './factory';\nimport { Island } from '@takazudo/zfb';\nimport { Counter } from './counter';\n",
        );
        for index in 0..FUNCTIONS {
            source.push_str(&format!(
                "function helper{index}() {{ return h('span', {{ children: '{index}' }}); }}\n"
            ));
        }
        source.push_str(
            "export default function Page() { return h(Island, { children: h(Counter, {}) }); }",
        );
        let resolver = CountingResolver {
            inner: InMemoryResolver::new()
                .with_file("/proj/page.ts", source)
                .with_file(
                    "/proj/factory.ts",
                    "export { h } from '@takazudo/zfb/zudo-react';",
                )
                .with_file(
                    "/proj/counter.ts",
                    "'use client'; export function Counter() { return null; }",
                ),
            demands: std::cell::Cell::new(0),
        };
        let islands = scan_islands(&[PathBuf::from("/proj/page.ts")], &resolver).unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        // Allow classification, summary, and form walks plus fixed overhead.
        // Resolving every call again for each helper exceeds this linear budget.
        assert!(
            resolver.demands.get() < FUNCTIONS * 10,
            "factory classification repeated across functions: {} resolutions for {FUNCTIONS} functions",
            resolver.demands.get(),
        );
    }

    #[test]
    fn sdk_namespace_matching_requires_an_exact_specifier() {
        assert!(Discovery::<InMemoryResolver>::is_sdk_namespace(
            "@takazudo/zfb"
        ));
        assert!(Discovery::<InMemoryResolver>::is_sdk_namespace(
            "@takazudo/zfb/zudo-react/jsx-runtime"
        ));
        assert!(!Discovery::<InMemoryResolver>::is_sdk_namespace(
            "@takazudo/zfb-extra"
        ));
    }

    #[test]
    fn direct_jsx_registers_target_but_not_unused_client_helper() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island as Boundary } from '@takazudo/zfb';
                import Counter from '../components/counter';
                <Boundary><Counter /></Boundary>;
            "#,
            ),
            (
                "components/counter.tsx",
                r#"
                'use client';
                export const readState = () => 1;
                export default function Counter() { return null; }
            "#,
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        assert_eq!(islands[0].component_name, "default");
    }

    #[test]
    fn configured_loader_import_in_ordinary_props_does_not_block_a_boundary() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Counter } from '../components/counter';
                <Island><Counter /></Island>;
            "#,
            ),
            (
                "components/counter.tsx",
                r#"
                'use client';
                import configuredLoader from './entry.fixture';
                export function Counter() {
                    return <button data-contract={[configuredLoader].join('|')}>count</button>;
                }
            "#,
            ),
            ("components/entry.fixture", "configured loader payload"),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn non_javascript_import_as_boundary_child_is_an_actionable_error() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Resource from '../components/entry.fixture';
                <Island><Resource /></Island>;
            "#,
            ),
            ("components/entry.fixture", "configured loader payload"),
        ])
        .expect_err("a resource cannot be registered as a function component");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("pages/home.tsx"), "{diagnostic}");
        assert!(diagnostic.contains("non-JavaScript module"), "{diagnostic}");
    }

    #[test]
    fn aliases_and_star_barrels_share_one_definition() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Counter as A, Alias as B } from '../components/barrel';
                <Island><A /></Island>;
                <Island><B /></Island>;
            "#,
            ),
            (
                "components/barrel.ts",
                r#"'use client'; export * from './counter';"#,
            ),
            (
                "components/counter.tsx",
                r#"
                function Counter() { return null; }
                const Alias = Counter;
                export { Counter, Alias };
            "#,
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn same_marker_from_two_definitions_is_hard_error() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import A from '../components/a';
                import B from '../components/b';
                <Island><A /></Island>;
                <Island><B /></Island>;
            "#,
            ),
            (
                "components/a.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
            (
                "components/b.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap_err();
        assert!(
            error.to_string().contains("ambiguous owned island marker"),
            "{error}"
        );
    }

    #[test]
    fn dynamic_child_fails_with_source_position_and_rewrite() {
        let error = scan(&[(
            "pages/home.tsx",
            r#"
                import { Island } from '@takazudo/zfb';
                const child = chooseChild();
                <Island>{child}</Island>;
            "#,
        )])
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("home.tsx:4:"), "{message}");
        assert!(message.contains("pass it directly"), "{message}");
    }

    #[test]
    fn direct_calls_and_forwarding_wrapper_register_exact_target() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { h } from '@takazudo/zfb/zudo-react';
                import Counter from '../components/counter';
                function LazyBoundary({ children }) { return <Island>{children}</Island>; }
                <LazyBoundary><Counter /></LazyBoundary>;
                Island({ children: h(Counter, {}) });
                h(Island, { children: h(Counter, {}) });
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn sdk_namespace_and_packed_factory_calls_resolve_static_members() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import * as SDK from '@takazudo/zfb';
                import { jsx as _jsx } from '@takazudo/zfb/zudo-react/jsx-runtime';
                import * as UI from '../components/client';
                (0, _jsx)(SDK.Island, { children: (0, _jsx)(UI['Counter'], {}) });
            "#,
            ),
            (
                "components/client.tsx",
                "'use client'; export const Counter = () => null;",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn shadowed_and_type_only_island_bindings_do_not_register() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import type { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                function render(Island) { return <Island><Counter /></Island>; }
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert!(islands.is_empty());
    }

    #[test]
    fn fixed_wrapper_is_static_even_without_call_site() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                function CounterBoundary() { return <Island><Counter /></Island>; }
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn unused_forwarding_wrapper_contributes_no_target() {
        let islands = scan(&[(
            "pages/home.tsx",
            r#"
                import { Island } from '@takazudo/zfb';
                function LazyBoundary(props) { return Island(props); }
            "#,
        )])
        .unwrap();
        assert!(islands.is_empty());
    }

    #[test]
    fn control_flow_or_write_before_return_cannot_prove_forwarding() {
        for statement in [
            "if (props.enabled) props.children = null;",
            "const ignored = (props.children = null);",
            "const ignored = unknownCall(props);",
            "const alias = props; const ignored = unknownCall(alias);",
        ] {
            let page = format!(
                r#"
                import {{ Island }} from '@takazudo/zfb';
                import Counter from '../components/counter';
                function LazyBoundary(props) {{ {statement} return <Island>{{props.children}}</Island>; }}
                <LazyBoundary><Counter /></LazyBoundary>;
            "#
            );
            let error = scan(&[
                ("pages/home.tsx", &page),
                (
                    "components/counter.tsx",
                    "'use client'; export default function Counter() { return null; }",
                ),
            ])
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported island registration"),
                "{error}"
            );
        }
    }

    #[test]
    fn unknown_return_option_call_cannot_mutate_forwarded_child() {
        for body in [
            "return <Island when={unknownCall(props)}>{props.children}</Island>;",
            "const alias = props; return <Island when={unknownCall(alias)}>{alias.children}</Island>;",
            "return Island({ when: unknownCall(props), children: props.children });",
        ] {
            let page = format!(
                r#"
                import {{ Island }} from '@takazudo/zfb';
                import Counter from '../components/counter';
                function Boundary(props) {{ {body} }}
                <Boundary><Counter /></Boundary>;
            "#
            );
            let error = scan(&[
                ("pages/home.tsx", &page),
                (
                    "components/counter.tsx",
                    "'use client'; export default function Counter() { return null; }",
                ),
            ])
            .unwrap_err();
            assert!(error.to_string().contains("unsupported island registration"), "{error}");
        }
    }

    #[test]
    fn owned_factory_can_prepare_static_child_with_runtime_wrapper_props() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { h as node } from '@takazudo/zfb/zudo-react';
                import Counter from '../components/counter';
                function Boundary(props) {
                    const child = node(Counter, { count: props.count });
                    return <Island>{child}</Island>;
                }
                <Boundary count={1} />;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn helper_only_client_module_keeps_graph_facts_without_registration() {
        let resolver = InMemoryResolver::new()
            .with_file(
                "/proj/pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                import '../components/helper-client';
                <Island><Counter /></Island>;
            "#,
            )
            .with_file(
                "/proj/components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            )
            .with_file(
                "/proj/components/helper-client.ts",
                "'use client'; import './resource'; export const readState = () => 1;",
            )
            .with_file("/proj/components/resource.ts", "export const resource = 1;");
        let (islands, meta) =
            scan_islands_with_meta(&[PathBuf::from("/proj/pages/home.tsx")], &resolver).unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        assert!(meta
            .island_reachable_modules
            .contains(&PathBuf::from("/proj/components/helper-client.ts")));
        assert!(meta
            .island_reachable_modules
            .contains(&PathBuf::from("/proj/components/resource.ts")));
    }

    #[test]
    fn opaque_boundary_escape_is_rejected() {
        let error = scan(&[(
            "pages/home.tsx",
            r#"
                import { Island } from '@takazudo/zfb';
                const wrappers = [Island];
            "#,
        )])
        .unwrap_err();
        assert!(error.to_string().contains("opaque container"), "{error}");
    }

    #[test]
    fn final_explicit_children_override_earlier_spread() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                <Island {...unknown} children={<Counter />} />;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn later_unknown_spread_is_rejected() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                <Island children={<Counter />} {...unknown} />;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("spread"), "{error}");
    }

    #[test]
    fn marker_comes_from_named_expression_not_export_alias() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Public as Local } from '../components/counter';
                <Island><Local /></Island>;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export const Public = function Inner() { return null; };",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Inner"]);
        assert_eq!(islands[0].component_name, "Public");
    }

    #[test]
    fn anonymous_literal_default_keeps_default_marker() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Local from '../components/counter';
                <Island><Local /></Island>;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default () => null;",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["default"]);
    }

    #[test]
    fn factory_wrapper_forwards_child_without_registering_wrapper() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { h } from '@takazudo/zfb/zudo-react';
                import Counter from '../components/counter';
                const Pass = props => h(Island, props);
                <Pass><Counter /></Pass>;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn ambiguous_demanded_star_export_is_rejected() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '../lib/barrel';
                <Island />;
            "#,
            ),
            (
                "lib/barrel.ts",
                "export * from '@takazudo/zfb'; export * from './other';",
            ),
            ("lib/other.ts", "export function Island() { return null; }"),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("ambiguous export *"), "{error}");
    }

    #[test]
    fn distinct_same_name_functions_in_one_client_module_collide() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { A, B } from '../components/duo';
                <Island><A /></Island>;
                <Island><B /></Island>;
            "#,
            ),
            (
                "components/duo.tsx",
                "'use client'; export const A = function Same() { return null; }; export const B = function Same() { return null; };",
            ),
        ])
        .unwrap_err();
        assert!(
            error.to_string().contains("ambiguous owned island marker"),
            "{error}"
        );
    }

    #[test]
    fn default_reexport_aliases_deduplicate_to_defining_binding() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import { Counter, Alias } from '../components/client';
                <Island><Counter /></Island>;
                <Island><Alias /></Island>;
            "#,
            ),
            (
                "components/client.ts",
                "'use client'; export { default as Counter, default as Alias } from './implementation';",
            ),
            (
                "components/implementation.tsx",
                "export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
        assert_eq!(
            islands[0].source_path,
            PathBuf::from("/proj/components/client.ts")
        );
    }

    #[test]
    fn factory_target_is_not_claimed_as_same_function_alias() {
        let error = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                <Island><Counter /></Island>;
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; const Counter = memo(Original); export default Counter;",
            ),
        ])
        .unwrap_err();
        assert!(
            error.to_string().contains("unsupported initializer"),
            "{error}"
        );
    }

    #[test]
    fn unrelated_package_jsx_is_not_treated_as_boundary() {
        let islands = scan(&[(
            "pages/home.tsx",
            r#"
                import { ClientRouter } from '@takazudo/zfb-runtime';
                <ClientRouter />;
            "#,
        )])
        .unwrap();
        assert!(islands.is_empty());
    }

    #[test]
    fn ordinary_recursive_component_cycle_is_not_a_boundary() {
        let islands = scan(&[(
            "pages/home.tsx",
            r#"
                function A(props) { return <B {...props} />; }
                function B(props) { return <A {...props} />; }
                <A />;
            "#,
        )])
        .unwrap();
        assert!(islands.is_empty());
    }

    #[test]
    fn nested_local_forwarding_wrapper_uses_lexical_binding() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                function Page() {
                    const Lazy = ({ children }) => <Island>{children}</Island>;
                    return <Lazy><Counter /></Lazy>;
                }
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }

    #[test]
    fn nested_wrapper_binding_and_summary_are_resolved() {
        let path = PathBuf::from("/proj/pages/home.tsx");
        let source = r#"
            import { Island } from '@takazudo/zfb';
            function Page() {
                const Lazy = ({ children }) => <Island>{children}</Island>;
                return <Lazy />;
            }
        "#;
        let ast = parse_module(&path, source).unwrap();
        let resolver = InMemoryResolver::new().with_file(path.clone(), source);
        let mut modules = BTreeMap::new();
        modules.insert(path.clone(), (ast, source.to_string()));
        let modules = modules
            .into_iter()
            .map(|(path, (ast, source))| {
                let (ast, _) = resolve_worker_bindings(ast);
                (
                    path,
                    SourceModule {
                        ast,
                        source,
                        client: false,
                    },
                )
            })
            .collect();
        let mut discovery = Discovery::new(&resolver, modules);
        let module = discovery.module(&path).unwrap();
        let mut collector = FormCollector::default();
        module.ast.visit_with(&mut collector);
        let lazy = collector.forms.iter().find_map(|form| match form {
            Form::Jsx(element) if matches!(&element.opening.name, JSXElementName::Ident(ident) if ident.sym == "Lazy") => Some(element),
            _ => None,
        }).unwrap();
        struct LazyBinding {
            id: Option<swc_core::ecma::ast::Id>,
        }
        impl Visit for LazyBinding {
            fn visit_var_declarator(&mut self, node: &VarDeclarator) {
                if let Pat::Ident(binding) = &node.name {
                    if binding.id.sym == "Lazy" {
                        self.id = Some(binding.id.to_id());
                    }
                }
                node.visit_children_with(self);
            }
        }
        let mut binding = LazyBinding { id: None };
        module.ast.visit_with(&mut binding);
        let JSXElementName::Ident(lazy_ref) = &lazy.opening.name else {
            unreachable!()
        };
        assert_eq!(
            Some(lazy_ref.to_id()),
            binding.id,
            "SWC lexical JSX binding mismatch"
        );
        let Value::Function(function) = discovery
            .resolve_jsx_name(&path, &lazy.opening.name)
            .unwrap()
        else {
            panic!("nested Lazy JSX name did not resolve to its lexical function binding");
        };
        assert!(
            matches!(
                discovery.summarize_wrapper(&function).unwrap(),
                WrapperSummary::ForwardChild
            ),
            "nested Lazy function was not summarized as ForwardChild"
        );
    }

    #[test]
    fn nested_parameter_shadow_does_not_resolve_outer_wrapper() {
        let islands = scan(&[
            (
                "pages/home.tsx",
                r#"
                import { Island } from '@takazudo/zfb';
                import Counter from '../components/counter';
                function Page() {
                    const Lazy = ({ children }) => <Island>{children}</Island>;
                    function Inner(Lazy) { return <Lazy />; }
                    return <Lazy><Counter /></Lazy>;
                }
            "#,
            ),
            (
                "components/counter.tsx",
                "'use client'; export default function Counter() { return null; }",
            ),
        ])
        .unwrap();
        assert_eq!(markers(&islands), ["Counter"]);
    }
}
