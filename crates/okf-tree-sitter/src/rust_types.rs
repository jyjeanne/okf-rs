//! Receiver-type hints and type facts for Rust, so `okf-analyzer` can
//! resolve a method call by the type of its receiver instead of by name
//! alone — `store.concurrency()` calls `Store::concurrency`, not one of the
//! other types' `concurrency` methods.
//!
//! This is syntax-only inference, deliberately conservative: a receiver
//! is described ([`TypeHint`]) only when the file states enough to trace
//! it — `self`, a typed parameter or `let`, a field, a chained call, a
//! `Type::f()` constructor, a struct literal — and the description is
//! resolved to a type name later, against facts from the whole project
//! ([`TypeFacts`](crate::TypeFacts)). Anything else (closure parameters,
//! `for` bindings, untyped generics, macros) yields no hint, so the call
//! keeps today's name-only resolution.

use crate::common::node_text;
use crate::TypeHint;
use std::collections::HashMap;
use tree_sitter::Node;

/// Wrapper types a method call reaches through (auto-deref, `?`, or an
/// unwrap), so `Arc<Store>` and `Option<&Store>` both resolve to `Store`.
/// Any type whose name ends in `Result` (`Result`, `io::Result`,
/// `BackendResult`, ...) is treated the same way.
const WRAPPERS: &[&str] = &[
    "Arc",
    "Rc",
    "Box",
    "Option",
    "Cow",
    "Pin",
    "RefCell",
    "Cell",
    "Mutex",
    "RwLock",
    "MutexGuard",
    "RwLockReadGuard",
    "RwLockWriteGuard",
    "Ref",
    "RefMut",
];

/// Marker and derive-style standard traits, which say nothing about the
/// project methods a value has: skipped when picking the trait that a
/// multi-bound generic (`T: Send + Repo`) or trait object
/// (`dyn Send + Repo`) stands for, so the receiver is typed `Repo` rather
/// than taken for the external type `Send`.
const MARKER_TRAITS: &[&str] = &[
    "Send",
    "Sync",
    "Sized",
    "Unpin",
    "Copy",
    "Clone",
    "Debug",
    "Display",
    "Default",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Hash",
    "UnwindSafe",
    "RefUnwindSafe",
];

/// Future types: `.await` on one yields its `Output`, which the type as
/// written doesn't name in a form traced here. They get no type at all, so
/// a call on the awaited value keeps name-only resolution instead of being
/// taken for a call on the external `Future`.
const FUTURES: &[&str] = &["Future", "BoxFuture", "LocalBoxFuture"];

/// Reduces a type as written to the bare name of the type a method call on
/// it would reach: references, lifetimes, `mut`, `dyn`/`impl` and wrapper
/// types removed, generics dropped, last path segment kept. `None` for
/// tuples, slices, arrays, function pointers, futures, and `_`.
///
/// `&'a mut Arc<crate::store::Store>` → `Store`; `Result<Self, E>` →
/// `Self`; `dyn Repo + Send` → `Repo`; `(A, B)` → `None`.
pub(crate) fn normalize_type(text: &str) -> Option<String> {
    let path = reached_path(text)?;
    let last = path.rsplit("::").next()?.trim();
    let first = last.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    Some(last.to_string())
}

/// The path of the type a method call on `text` reaches, before
/// [`normalize_type`] keeps only its last segment:
/// `&Arc<crate::store::Store>` → `crate::store::Store`,
/// `Option<&mut Self::Item>` → `Self::Item`. `None` where
/// [`normalize_type`] gives `None`.
fn reached_path(text: &str) -> Option<String> {
    let t = strip_type_prefixes(text);
    if t.is_empty() || t.starts_with(['(', '[', '_', '!', '*']) || t.starts_with("fn(") {
        return None;
    }
    let (head, args) = match t.find('<') {
        Some(open) => {
            let close = t.rfind('>')?;
            (&t[..open], (close > open).then(|| &t[open + 1..close]))
        }
        None => (t, None),
    };
    let head = pick_trait(head.split('+'))?;
    let last = head.rsplit("::").next()?.trim();
    // Only a wrapper *with* type arguments is unwrapped: a plain struct
    // that happens to be named `SearchResult` stays itself.
    if WRAPPERS.contains(&last) || last.ends_with("Result") {
        if let Some(args) = args {
            return reached_path(first_type_argument(args)?);
        }
    }
    if FUTURES.contains(&last) {
        return None;
    }
    Some(head.to_string())
}

/// `text` without its leading references, lifetimes, `mut`, `dyn` and
/// `impl`: `&'a mut dyn Repo` → `Repo`.
fn strip_type_prefixes(text: &str) -> &str {
    let mut t = text.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if let Some(rest) = t.strip_prefix('\'') {
            t = rest
                .split_once(char::is_whitespace)
                .map(|(_, r)| r)
                .unwrap_or("")
                .trim_start();
        }
        for keyword in ["mut ", "dyn ", "impl "] {
            if let Some(rest) = t.strip_prefix(keyword) {
                t = rest.trim_start();
            }
        }
        if t == before {
            return t;
        }
    }
}

/// Of the `+`-separated parts of a bound list or trait object, the first
/// that is neither a lifetime nor a [`MARKER_TRAITS`] entry — or the first
/// part at all when every part is one.
fn pick_trait<'a>(parts: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let parts: Vec<&str> = parts
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with('\''))
        .collect();
    let is_marker = |p: &&str| {
        let last = p.rsplit("::").next().unwrap_or(p).trim();
        MARKER_TRAITS.contains(&last)
    };
    parts
        .iter()
        .find(|p| !is_marker(p))
        .or(parts.first())
        .copied()
}

/// The first comma-separated argument of a generic argument list, at the
/// top level only (`Result<HashMap<K, V>, E>` → `HashMap<K, V>`).
fn first_type_argument(args: &str) -> Option<&str> {
    let mut depth = 0i32;
    for (i, c) in args.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => return Some(args[..i].trim()),
            _ => {}
        }
    }
    let whole = args.trim();
    (!whole.is_empty()).then_some(whole)
}

/// Generic parameter name → its first trait bound, for every generic in
/// scope at a node (innermost declaration wins). An unbounded generic maps
/// to `None`: a receiver of that type gets no hint.
type Generics = HashMap<String, Option<String>>;

fn generics_in_scope(src: &str, node: Node) -> Generics {
    let mut generics = Generics::new();
    let mut current = Some(node);
    while let Some(n) = current {
        if matches!(
            n.kind(),
            "function_item"
                | "function_signature_item"
                | "impl_item"
                | "trait_item"
                | "struct_item"
        ) {
            if let Some(params) = n.child_by_field_name("type_parameters") {
                let mut cursor = params.walk();
                for param in params.named_children(&mut cursor) {
                    let (name, bound) = match param.kind() {
                        "type_identifier" => (Some(param), None),
                        "constrained_type_parameter" => (
                            param.child_by_field_name("left"),
                            param.child_by_field_name("bounds"),
                        ),
                        "type_parameter" => (
                            param.child_by_field_name("name"),
                            param.child_by_field_name("bounds"),
                        ),
                        _ => (None, None),
                    };
                    if let Some(name) = name {
                        let bound = bound.and_then(|b| first_bound(src, b));
                        generics
                            .entry(node_text(src, name).to_string())
                            .or_insert(bound);
                    }
                }
            }
            let mut cursor = n.walk();
            for child in n.named_children(&mut cursor) {
                if child.kind() != "where_clause" {
                    continue;
                }
                let mut predicates = child.walk();
                for predicate in child.named_children(&mut predicates) {
                    if let (Some(left), Some(bounds)) = (
                        predicate.child_by_field_name("left"),
                        predicate.child_by_field_name("bounds"),
                    ) {
                        let bound = first_bound(src, bounds);
                        let entry = generics
                            .entry(node_text(src, left).to_string())
                            .or_insert(None);
                        if entry.is_none() {
                            *entry = bound;
                        }
                    }
                }
            }
        }
        current = n.parent();
    }
    generics
}

/// The trait a `trait_bounds` node stands for: its first trait that isn't
/// a [`MARKER_TRAITS`] entry (`T: Send + Repo` → `Repo`), else its first
/// trait, skipping lifetimes and `?Sized`-style relaxations.
fn first_bound(src: &str, bounds: Node) -> Option<String> {
    let mut cursor = bounds.walk();
    let traits: Vec<String> = bounds
        .named_children(&mut cursor)
        .filter(|b| !matches!(b.kind(), "lifetime" | "removed_trait_bound"))
        .filter_map(|b| normalize_type(node_text(src, b)))
        .collect();
    traits
        .iter()
        .find(|t| !MARKER_TRAITS.contains(&t.as_str()))
        .or(traits.first())
        .cloned()
}

/// The type `Self` means at `node`: the nearest enclosing `impl` block's
/// type, or the trait's own name inside a trait.
pub(crate) fn self_type(src: &str, node: Node) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        match n.kind() {
            "impl_item" => return normalize_type(node_text(src, n.child_by_field_name("type")?)),
            "trait_item" => {
                return Some(node_text(src, n.child_by_field_name("name")?).to_string())
            }
            _ => {}
        }
        current = n.parent();
    }
    None
}

/// [`normalize_type`] in the context of `node`: `Self` becomes the
/// enclosing type, and a generic parameter becomes its trait bound (or
/// `None` when it has none).
pub(crate) fn resolve_type_text(src: &str, node: Node, text: &str) -> Option<String> {
    let name = normalize_type(text)?;
    let generics = generics_in_scope(src, node);
    // An associated type (`Self::Item`, `&mut T::Output`,
    // `Option<Self::Item>`) depends on an impl this file may not show:
    // unknown, not a type named `Item`. Checked on the path the type
    // reaches, after references, `mut` and wrappers are stripped.
    let path = reached_path(text)?;
    let segments: Vec<&str> = path.split("::").map(str::trim).collect();
    if segments.len() > 1 && (segments[0] == "Self" || generics.contains_key(segments[0])) {
        return None;
    }
    if name == "Self" {
        return self_type(src, node);
    }
    match generics.get(&name) {
        Some(bound) => bound.clone(),
        None => Some(name),
    }
}

/// Builds receiver hints for the calls in one file. Holds a per-function
/// index of `let` bindings, built on first use, so each function body is
/// scanned once however many calls it contains.
pub(crate) struct HintContext<'s, 't> {
    src: &'s str,
    lets: HashMap<usize, Vec<(String, Node<'t>, bool)>>,
}

/// Recursion bound for nested receiver expressions and `let` chains.
const MAX_DEPTH: usize = 8;

impl<'s, 't> HintContext<'s, 't> {
    pub(crate) fn new(src: &'s str) -> Self {
        HintContext {
            src,
            lets: HashMap::new(),
        }
    }

    /// The hint for a call whose callee name is `call_name` — the
    /// `field_identifier` of `recv.method(..)` or the `identifier` of
    /// `Type::function(..)` — or `None` for a plain `function(..)` call and
    /// receivers that can't be described.
    pub(crate) fn receiver_hint(&mut self, call_name: Node<'t>) -> Option<TypeHint> {
        let parent = call_name.parent()?;
        match parent.kind() {
            "field_expression" => {
                let value = parent.child_by_field_name("value")?;
                self.expr_hint(value, value.start_byte(), 0)
            }
            "scoped_identifier" => {
                let path = parent.child_by_field_name("path")?;
                self.type_qualifier(path).map(TypeHint::Named)
            }
            _ => None,
        }
    }

    /// A `Type` or `Self` path qualifier as a type name; `None` for a
    /// module path (`git::push_updates`), whose last segment is lowercase.
    fn type_qualifier(&self, path: Node<'t>) -> Option<String> {
        let text = node_text(self.src, path);
        let last = text.rsplit("::").next()?.split('<').next()?.trim();
        if !last.chars().next()?.is_uppercase() {
            return None;
        }
        resolve_type_text(self.src, path, last)
    }

    fn expr_hint(&mut self, node: Node<'t>, before: usize, depth: usize) -> Option<TypeHint> {
        if depth > MAX_DEPTH {
            return None;
        }
        match node.kind() {
            "self" => self_type(self.src, node).map(TypeHint::Named),
            "identifier" => self.binding_hint(node, before, depth),
            "field_expression" => {
                let value = node.child_by_field_name("value")?;
                let field = node.child_by_field_name("field")?;
                let base = self.expr_hint(value, before, depth + 1)?;
                Some(TypeHint::Field {
                    base: Box::new(base),
                    field: node_text(self.src, field).to_string(),
                })
            }
            "call_expression" => {
                let function = node.child_by_field_name("function")?;
                match function.kind() {
                    "field_expression" => {
                        let value = function.child_by_field_name("value")?;
                        let method = function.child_by_field_name("field")?;
                        let base = self.expr_hint(value, before, depth + 1)?;
                        Some(TypeHint::MethodReturn {
                            base: Box::new(base),
                            method: node_text(self.src, method).to_string(),
                        })
                    }
                    "scoped_identifier" => {
                        let name = function.child_by_field_name("name")?;
                        let raw_owner = function
                            .child_by_field_name("path")
                            .map(|p| node_text(self.src, p))
                            .and_then(|p| p.rsplit("::").next())
                            .map(|p| p.split('<').next().unwrap_or(p).trim());
                        // `Arc::new(x)`, `Box::new(x)`, ...: the wrapper is
                        // transparent to method calls, so the value has
                        // `x`'s type.
                        if raw_owner.is_some_and(|o| WRAPPERS.contains(&o)) {
                            let arguments = node.child_by_field_name("arguments")?;
                            let first = arguments.named_child(0)?;
                            return self.expr_hint(first, before, depth + 1);
                        }
                        let owner = function
                            .child_by_field_name("path")
                            .and_then(|p| self.type_qualifier(p));
                        Some(TypeHint::FnReturn {
                            owner,
                            function: node_text(self.src, name).to_string(),
                        })
                    }
                    "identifier" => Some(TypeHint::FnReturn {
                        owner: None,
                        function: node_text(self.src, function).to_string(),
                    }),
                    _ => None,
                }
            }
            "struct_expression" => {
                let name = node.child_by_field_name("name")?;
                resolve_type_text(self.src, node, node_text(self.src, name)).map(TypeHint::Named)
            }
            "reference_expression" => {
                let value = node.child_by_field_name("value")?;
                self.expr_hint(value, before, depth + 1)
            }
            "try_expression"
            | "await_expression"
            | "parenthesized_expression"
            | "unary_expression" => {
                let inner = node.named_child(0)?;
                self.expr_hint(inner, before, depth + 1)
            }
            _ => None,
        }
    }

    /// The hint for identifier `ident`: the nearest `let` of that name in
    /// scope at `before` in the enclosing function (its annotation, else
    /// its value), else a parameter of that name. `None` when a binding
    /// that carries no traced type shadows it: a closure parameter, a `for`
    /// binding, an `if let`/`while let`/`match` pattern, or a destructuring
    /// `let`.
    fn binding_hint(&mut self, ident: Node<'t>, before: usize, depth: usize) -> Option<TypeHint> {
        let name = node_text(self.src, ident);
        let mut function = None;
        let mut prev = ident;
        let mut current = ident.parent();
        while let Some(n) = current {
            if n.kind() == "function_item" {
                function = Some(n);
                break;
            }
            if shadows(self.src, n, prev, name) {
                return None;
            }
            prev = n;
            current = n.parent();
        }
        let function = function?;

        let lets = self
            .lets
            .entry(function.id())
            .or_insert_with(|| collect_lets(self.src, function));
        // A `let` is in scope once its statement has ended (so the `x` in
        // `let x = x.f();` is still the earlier `x`), and only inside the
        // block that holds it.
        let binding = lets
            .iter()
            .filter(|(n, node, _)| {
                n == name
                    && node.end_byte() <= before
                    && node.parent().is_some_and(|block| {
                        block.start_byte() <= before && before < block.end_byte()
                    })
            })
            .max_by_key(|(_, node, _)| node.start_byte())
            .map(|(_, node, simple)| (*node, *simple));
        if let Some((binding, simple)) = binding {
            if !simple {
                // `let (a, b) = ...`: the part's type isn't traced.
                return None;
            }
            if let Some(ty) = binding.child_by_field_name("type") {
                return resolve_type_text(self.src, binding, node_text(self.src, ty))
                    .map(TypeHint::Named);
            }
            let value = binding.child_by_field_name("value")?;
            return self.expr_hint(value, binding.start_byte(), depth + 1);
        }

        if let Some(params) = function.child_by_field_name("parameters") {
            let mut cursor = params.walk();
            let found = params.named_children(&mut cursor).find(|p| {
                p.kind() == "parameter"
                    && p.child_by_field_name("pattern")
                        .is_some_and(|pat| pattern_name(self.src, pat) == Some(name))
            });
            if let Some(param) = found {
                let ty = param.child_by_field_name("type")?;
                return resolve_type_text(self.src, ty, node_text(self.src, ty))
                    .map(TypeHint::Named);
            }
        }

        // Not a binding: a CamelCase identifier is a unit struct used as a
        // value (`Arc::new(Store)`), whose type is itself. ALL_CAPS
        // constants are left alone.
        let unit_struct = name.chars().next().is_some_and(char::is_uppercase)
            && name.chars().any(char::is_lowercase);
        unit_struct.then(|| TypeHint::Named(name.to_string()))
    }
}

/// Every name a `let` in `function`'s body (outside nested functions)
/// binds, with its `let_declaration` and whether the pattern is a plain
/// identifier (`x`, `mut x`) — only those carry a traceable type; a
/// destructuring `let (a, b) = ..` still shadows `a` and `b`.
fn collect_lets<'t>(src: &str, function: Node<'t>) -> Vec<(String, Node<'t>, bool)> {
    let mut lets = Vec::new();
    let Some(body) = function.child_by_field_name("body") else {
        return lets;
    };
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if node.kind() == "let_declaration" {
            if let Some(pattern) = node.child_by_field_name("pattern") {
                match pattern_name(src, pattern) {
                    Some(name) => lets.push((name.to_string(), node, true)),
                    None => {
                        let mut names = Vec::new();
                        bound_names(src, pattern, &mut names);
                        lets.extend(names.into_iter().map(|name| (name, node, false)));
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "function_item" {
                stack.push(child);
            }
        }
    }
    lets
}

/// The bound name of a simple pattern: `x` or `mut x`.
fn pattern_name<'s>(src: &'s str, pattern: Node) -> Option<&'s str> {
    match pattern.kind() {
        "identifier" => Some(node_text(src, pattern)),
        "mut_pattern" => pattern.named_child(0).and_then(|p| pattern_name(src, p)),
        _ => None,
    }
}

/// Whether `node` (a closure's parameters, a `for` pattern) binds `name`
/// anywhere inside it.
fn binds(src: &str, node: Node, name: &str) -> bool {
    if is_binding_name(node) && node_text(src, node) == name {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|c| binds(src, c, name));
    found
}

/// Every name a pattern may bind (an over-approximation: a tuple-struct
/// pattern's type name is included too, which only ever makes a lookup
/// more conservative).
fn bound_names(src: &str, node: Node, out: &mut Vec<String>) {
    if is_binding_name(node) {
        out.push(node_text(src, node).to_string());
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        bound_names(src, child, out);
    }
}

fn is_binding_name(node: Node) -> bool {
    matches!(node.kind(), "identifier" | "shorthand_field_identifier")
}

/// Whether ancestor `n` of an identifier — reached from its child `prev` —
/// binds `name` in a pattern whose type isn't traced: a closure parameter,
/// a `for` binding, an `if let`/`while let` pattern seen from the body it
/// guards (or from a later `let` of the same chain), or a `match` arm's
/// pattern seen from the arm's guard or body.
fn shadows(src: &str, n: Node, prev: Node, name: &str) -> bool {
    let is_field = |field: &str| {
        n.child_by_field_name(field)
            .is_some_and(|child| child.id() == prev.id())
    };
    let condition_binds = || {
        n.child_by_field_name("condition")
            .is_some_and(|condition| let_patterns_bind(src, condition, None, name))
    };
    match n.kind() {
        "closure_expression" => n
            .child_by_field_name("parameters")
            .is_some_and(|params| binds(src, params, name)),
        "for_expression" => n
            .child_by_field_name("pattern")
            .is_some_and(|pattern| binds(src, pattern, name)),
        "if_expression" => is_field("consequence") && condition_binds(),
        "while_expression" => is_field("body") && condition_binds(),
        "let_chain" => let_patterns_bind(src, n, Some(prev), name),
        "match_arm" => n.child_by_field_name("pattern").is_some_and(|pattern| {
            // The guard (`Some(x) if x.ok()`) sits inside the pattern node
            // but binds nothing itself.
            let guard = pattern.child_by_field_name("condition").map(|g| g.id());
            let mut cursor = pattern.walk();
            let found = pattern
                .named_children(&mut cursor)
                .filter(|child| Some(child.id()) != guard)
                .any(|child| binds(src, child, name));
            found || (pattern.named_child_count() == 0 && binds(src, pattern, name))
        }),
        _ => false,
    }
}

/// Whether a `let_condition`, or one of the `let_condition`s of a
/// `let_chain` before `stop` (all of them when `stop` is `None`), binds
/// `name`.
fn let_patterns_bind(src: &str, node: Node, stop: Option<Node>, name: &str) -> bool {
    let pattern_binds = |condition: Node| {
        condition
            .child_by_field_name("pattern")
            .is_some_and(|pattern| binds(src, pattern, name))
    };
    match node.kind() {
        "let_condition" => pattern_binds(node),
        "let_chain" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if stop.is_some_and(|s| s.id() == child.id()) {
                    break;
                }
                if child.kind() == "let_condition" && pattern_binds(child) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_type_strips_references_wrappers_and_paths() {
        for (text, expected) in [
            ("Store", Some("Store")),
            ("&Store", Some("Store")),
            ("&'a mut Store", Some("Store")),
            ("Arc<Store>", Some("Store")),
            ("&Arc<crate::store::Store>", Some("Store")),
            ("Option<&Store>", Some("Store")),
            ("Result<Self, Error>", Some("Self")),
            ("BackendResult<Commit>", Some("Commit")),
            ("io::Result<File>", Some("File")),
            ("Box<dyn Backend>", Some("Backend")),
            ("dyn Repo + Send", Some("Repo")),
            ("dyn Send + Repo", Some("Repo")),
            ("Box<dyn Send + Sync + 'static + Repo>", Some("Repo")),
            ("impl Iterator<Item = u8>", Some("Iterator")),
            ("impl Future<Output = Store>", None),
            ("Pin<Box<dyn Future<Output = Store> + Send>>", None),
            ("BoxFuture<'_, Store>", None),
            ("HashMap<K, V>", Some("HashMap")),
            ("SearchResult", Some("SearchResult")),
            ("Result<HashMap<K, V>, E>", Some("HashMap")),
            ("(A, B)", None),
            ("[u8]", None),
            ("fn(u8) -> u8", None),
            ("_", None),
        ] {
            assert_eq!(normalize_type(text).as_deref(), expected, "{text}");
        }
    }
}
