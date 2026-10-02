//! Binding-aware discovery of concrete SDK island boundary children.
//!
//! This pass is intentionally separate from the module walk in `scanner.rs`:
//! imports and client-module resource facts remain rooted at every reachable
//! directive-bearing module, while this pass returns only validated targets.

use super::*;
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

#[derive(Clone)]
struct SourceModule {
    ast: Module,
    source: String,
    client: bool,
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

struct Discovery<'a, R: Resolver> {
    resolver: &'a R,
    modules: BTreeMap<PathBuf, SourceModule>,
    resolving: HashSet<(PathBuf, String)>,
    summarizing: HashSet<Definition>,
    summary_cache: BTreeMap<Definition, WrapperSummary>,
    deferred_forward_sites: HashSet<(PathBuf, u32)>,
}

impl<'a, R: Resolver> Discovery<'a, R> {
    fn new(resolver: &'a R, modules: BTreeMap<PathBuf, SourceModule>) -> Self {
        Self {
            resolver,
            modules,
            resolving: HashSet::new(),
            summarizing: HashSet::new(),
            summary_cache: BTreeMap::new(),
            deferred_forward_sites: HashSet::new(),
        }
    }

    fn module(&mut self, path: &Path) -> ScanResult<SourceModule> {
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
        let module = SourceModule {
            ast,
            source,
            client,
        };
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
                            other => Ok(Value::Unsupported(format!(
                                "target {} has unsupported initializer {:?}",
                                ident.sym,
                                other.span()
                            ))),
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
        // Wrappers may be local to a page function. SWC's syntax context
        // distinguishes that binding from same-spelled module imports.
        struct NestedFinder {
            id: swc_core::ecma::ast::Id,
            function: Option<swc_core::ecma::ast::FnDecl>,
            variable: Option<(swc_core::ecma::ast::VarDeclKind, VarDeclarator)>,
        }
        impl Visit for NestedFinder {
            fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
                if node.ident.to_id() == self.id {
                    self.function = Some(node.clone());
                } else {
                    node.visit_children_with(self);
                }
            }

            fn visit_var_decl(&mut self, node: &swc_core::ecma::ast::VarDecl) {
                for declaration in &node.decls {
                    if let Pat::Ident(binding) = &declaration.name {
                        if binding.id.to_id() == self.id {
                            self.variable = Some((node.kind, declaration.clone()));
                            return;
                        }
                    }
                }
                node.visit_children_with(self);
            }
        }
        let mut finder = NestedFinder {
            id: ident.to_id(),
            function: None,
            variable: None,
        };
        module.ast.visit_with(&mut finder);
        if let Some(function) = finder.function {
            return Ok(self.definition(
                path,
                &function.ident.sym,
                function.ident.sym.to_string(),
                function.function.span,
            ));
        }
        if let Some((kind, variable)) = finder.variable {
            if kind != swc_core::ecma::ast::VarDeclKind::Const {
                return Ok(Value::Unsupported(format!(
                    "mutable target binding {} is not a stable function",
                    ident.sym
                )));
            }
            if let Some(init) = variable.init {
                return match unwrap_expr(&init) {
                    Expr::Fn(function) => Ok(self.definition(
                        path,
                        &ident.sym,
                        function
                            .ident
                            .as_ref()
                            .map(|name| name.sym.to_string())
                            .unwrap_or_else(|| ident.sym.to_string()),
                        function.function.span,
                    )),
                    Expr::Arrow(arrow) => {
                        Ok(self.definition(path, &ident.sym, ident.sym.to_string(), arrow.span))
                    }
                    Expr::Ident(alias) => self.resolve_local(path, alias),
                    _ => Ok(Value::Unsupported(format!(
                        "target {} has unsupported initializer",
                        ident.sym
                    ))),
                };
            }
        }
        Ok(Value::Other)
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

    fn function_return(
        &mut self,
        function: &FunctionValue,
    ) -> ScanResult<Option<(Expr, Vec<Pat>)>> {
        let path = &function.definition.module;
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
        for item in &module.ast.body {
            let declaration = match item {
                ModuleItem::Stmt(Stmt::Decl(decl)) => Some(decl),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => Some(&export.decl),
                _ => None,
            };
            if let Some(declaration) = declaration {
                match declaration {
                    Decl::Fn(decl)
                        if decl.ident.sym == function.definition.binding
                            && decl.function.span.lo.0 == function.definition.position =>
                    {
                        let params = decl
                            .function
                            .params
                            .iter()
                            .map(|param| param.pat.clone())
                            .collect();
                        let body = decl.function.body.as_ref();
                        return Ok(body_return(body, params, &owned_factory_sites));
                    }
                    Decl::Var(variable) => {
                        for declarator in &variable.decls {
                            let Pat::Ident(binding) = &declarator.name else {
                                continue;
                            };
                            if binding.id.sym != function.definition.binding {
                                continue;
                            }
                            let init_position = match declarator.init.as_deref().map(unwrap_expr) {
                                Some(Expr::Arrow(arrow)) => arrow.span.lo.0,
                                Some(Expr::Fn(inner)) => inner.function.span.lo.0,
                                _ => 0,
                            };
                            if init_position != function.definition.position {
                                continue;
                            }
                            return Ok(match declarator.init.as_deref().map(unwrap_expr) {
                                Some(Expr::Arrow(arrow)) => {
                                    arrow_return(arrow, &owned_factory_sites)
                                }
                                Some(Expr::Fn(function)) => body_return(
                                    function.function.body.as_ref(),
                                    function
                                        .function
                                        .params
                                        .iter()
                                        .map(|param| param.pat.clone())
                                        .collect(),
                                    &owned_factory_sites,
                                ),
                                _ => None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            if function.definition.binding == "default" {
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
                        ));
                    }
                }
                if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(default)) = item {
                    return Ok(match unwrap_expr(&default.expr) {
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
                    });
                }
            }
        }
        struct NestedReturnFinder<'a> {
            position: u32,
            result: Option<(Expr, Vec<Pat>)>,
            owned_factory_sites: &'a HashSet<u32>,
        }
        impl Visit for NestedReturnFinder<'_> {
            fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
                if node.function.span.lo.0 == self.position {
                    self.result = body_return(
                        node.function.body.as_ref(),
                        node.function
                            .params
                            .iter()
                            .map(|param| param.pat.clone())
                            .collect(),
                        self.owned_factory_sites,
                    );
                } else {
                    node.visit_children_with(self);
                }
            }

            fn visit_var_declarator(&mut self, node: &VarDeclarator) {
                if let Some(init) = node.init.as_deref().map(unwrap_expr) {
                    match init {
                        Expr::Arrow(arrow) if arrow.span.lo.0 == self.position => {
                            self.result = arrow_return(arrow, self.owned_factory_sites);
                            return;
                        }
                        Expr::Fn(inner) if inner.function.span.lo.0 == self.position => {
                            self.result = body_return(
                                inner.function.body.as_ref(),
                                inner
                                    .function
                                    .params
                                    .iter()
                                    .map(|param| param.pat.clone())
                                    .collect(),
                                self.owned_factory_sites,
                            );
                            return;
                        }
                        _ => {}
                    }
                }
                node.visit_children_with(self);
            }
        }
        let mut finder = NestedReturnFinder {
            position: function.definition.position,
            result: None,
            owned_factory_sites: &owned_factory_sites,
        };
        module.ast.visit_with(&mut finder);
        if finder.result.is_some() {
            return Ok(finder.result);
        }
        Ok(None)
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
        let Some((returned, params)) = self.function_return(function)? else {
            return Ok(WrapperSummary::Ordinary);
        };
        let returned = unwrap_expr(&returned);
        match returned {
            Expr::JSXElement(element) => {
                let value = self.resolve_jsx_name(path, &element.opening.name)?;
                let summary = match value {
                    Value::Boundary => {
                        let child = self.child_from_jsx_boundary(path, element)?;
                        self.summarize_child(path, child, &params, &Form::Jsx((**element).clone()))?
                    }
                    Value::Function(inner) => {
                        self.compose_wrapper_jsx(path, element, &params, &inner)?
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
                        self.summarize_child(path, child, &params, &Form::Call(call.clone()))
                    }
                    Value::Function(inner) => {
                        self.compose_wrapper_call(path, call, &params, &inner)
                    }
                    Value::Factory(kind) => {
                        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none())
                        else {
                            return Ok(WrapperSummary::Ordinary);
                        };
                        match self.resolve_expr(path, &first.expr)? {
                            Value::Boundary => {
                                let child =
                                    self.child_from_call_props(path, call, kind == FactoryKind::H)?;
                                self.summarize_child(
                                    path,
                                    child,
                                    &params,
                                    &Form::Call(call.clone()),
                                )
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
                                        &params,
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
        let paths: Vec<PathBuf> = self.modules.keys().cloned().collect();
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
            struct NestedNames {
                names: Vec<swc_core::ecma::ast::Ident>,
            }
            impl Visit for NestedNames {
                fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
                    self.names.push(node.ident.clone());
                    node.visit_children_with(self);
                }
                fn visit_var_decl(&mut self, node: &swc_core::ecma::ast::VarDecl) {
                    for declarator in &node.decls {
                        if let Pat::Ident(binding) = &declarator.name {
                            self.names.push(binding.id.clone());
                        }
                    }
                    node.visit_children_with(self);
                }
            }
            let mut nested = NestedNames { names: Vec::new() };
            module.ast.visit_with(&mut nested);
            names.extend(nested.names);
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
    loop {
        let paths: Vec<PathBuf> = discovery
            .modules
            .keys()
            .filter(|path| !scanned.contains(*path))
            .cloned()
            .collect();
        if paths.is_empty() {
            break;
        }
        discovery.prime_wrapper_summaries()?;
        for path in paths {
            scanned.insert(path.clone());
            let module = discovery.module(&path)?;
            let mut collector = FormCollector::default();
            module.ast.visit_with(&mut collector);
            for form in collector.forms {
                if let Some(target) = discovery.resolve_form(&path, &form)? {
                    targets.entry(target.definition.clone()).or_insert((
                        target,
                        path.clone(),
                        form.span(),
                    ));
                }
            }
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
