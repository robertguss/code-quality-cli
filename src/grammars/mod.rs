//! Bundled tree-sitter grammars and, per language, which nodes are functions
//! and which nodes contribute to a function's qualified name.

use tree_sitter::{Node, Parser, Tree};

use crate::hash::Fnv;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Java,
    C,
    Cpp,
    Ruby,
    Bash,
    CSharp,
    Php,
}

pub const ALL: &[Lang] = &[
    Lang::Rust,
    Lang::Python,
    Lang::JavaScript,
    Lang::TypeScript,
    Lang::Tsx,
    Lang::Go,
    Lang::Java,
    Lang::C,
    Lang::Cpp,
    Lang::Ruby,
    Lang::Bash,
    Lang::CSharp,
    Lang::Php,
];

/// How to find functions in one language's syntax tree.
struct Spec {
    /// Nodes measured as functions.
    functions: &'static [&'static str],
    /// Nodes whose name prefixes the functions inside them.
    containers: &'static [&'static str],
}

impl Lang {
    pub fn detect(path: &str) -> Option<Lang> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        Some(match ext.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "rb" => Lang::Ruby,
            "sh" | "bash" => Lang::Bash,
            "cs" => Lang::CSharp,
            "php" => Lang::Php,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::C => "c",
            Lang::Cpp => "cpp",
            Lang::Ruby => "ruby",
            Lang::Bash => "bash",
            Lang::CSharp => "csharp",
            Lang::Php => "php",
        }
    }

    fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        }
    }

    fn spec(self) -> Spec {
        const JS_FUNCTIONS: &[&str] = &[
            "function_declaration",
            "generator_function_declaration",
            "function_expression",
            "function",
            "generator_function",
            "arrow_function",
            "method_definition",
        ];
        const JS_CONTAINERS: &[&str] =
            &["class_declaration", "class", "abstract_class_declaration"];
        match self {
            Lang::Rust => Spec {
                functions: &["function_item"],
                containers: &["impl_item", "trait_item", "mod_item"],
            },
            Lang::Python => Spec {
                functions: &["function_definition"],
                containers: &["class_definition"],
            },
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => Spec {
                functions: JS_FUNCTIONS,
                containers: JS_CONTAINERS,
            },
            Lang::Go => Spec {
                functions: &["function_declaration", "method_declaration", "func_literal"],
                containers: &[],
            },
            Lang::Java => Spec {
                functions: &["method_declaration", "constructor_declaration"],
                containers: &[
                    "class_declaration",
                    "interface_declaration",
                    "enum_declaration",
                    "record_declaration",
                ],
            },
            Lang::C => Spec {
                functions: &["function_definition"],
                containers: &[],
            },
            Lang::Cpp => Spec {
                functions: &["function_definition", "lambda_expression"],
                containers: &[
                    "class_specifier",
                    "struct_specifier",
                    "namespace_definition",
                ],
            },
            Lang::Ruby => Spec {
                functions: &["method", "singleton_method"],
                containers: &["class", "module"],
            },
            Lang::Bash => Spec {
                functions: &["function_definition"],
                containers: &[],
            },
            Lang::CSharp => Spec {
                functions: &[
                    "method_declaration",
                    "constructor_declaration",
                    "local_function_statement",
                ],
                containers: &[
                    "class_declaration",
                    "struct_declaration",
                    "interface_declaration",
                    "record_declaration",
                    "namespace_declaration",
                ],
            },
            Lang::Php => Spec {
                functions: &["function_definition", "method_declaration"],
                containers: &[
                    "class_declaration",
                    "trait_declaration",
                    "interface_declaration",
                ],
            },
        }
    }
}

/// A function found in source, with its 1-based line span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Container names and the function name joined by `::`.
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Hash of the function's tokens, ignoring its name, whitespace and comments.
    /// Equal hashes mean a moved or renamed function with an unchanged body.
    pub body_hash: u64,
}

impl Function {
    pub fn lines(&self) -> u32 {
        self.end_line - self.start_line + 1
    }
}

pub struct Parsed {
    pub tree: Tree,
    pub functions: Vec<Function>,
    pub has_errors: bool,
}

pub fn parse(lang: Lang, source: &str) -> Option<Parsed> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).ok()?;
    let tree = parser.parse(source, None)?;
    let spec = lang.spec();
    let mut functions = Vec::new();
    let mut path = Vec::new();
    collect(
        lang,
        &spec,
        tree.root_node(),
        source,
        &mut path,
        &mut functions,
    );
    let has_errors = tree.root_node().has_error();
    Some(Parsed {
        tree,
        functions,
        has_errors,
    })
}

fn collect(
    lang: Lang,
    spec: &Spec,
    node: Node,
    src: &str,
    path: &mut Vec<String>,
    out: &mut Vec<Function>,
) {
    let kind = node.kind();
    let mut pushed = false;
    if node.is_named() && spec.functions.contains(&kind) {
        let name = function_name(lang, node, src);
        let qualified = path
            .iter()
            .cloned()
            .chain(std::iter::once(name.clone()))
            .collect::<Vec<_>>()
            .join("::");
        let start = node.start_position();
        let mut end = node.end_position();
        if end.column == 0 && end.row > start.row {
            end.row -= 1;
        }
        out.push(Function {
            name: qualified,
            start_line: start.row as u32 + 1,
            end_line: end.row as u32 + 1,
            body_hash: body_hash(node, src),
        });
        path.push(name);
        pushed = true;
    } else if node.is_named() && spec.containers.contains(&kind) {
        path.push(container_name(lang, node, src));
        pushed = true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(lang, spec, child, src, path, out);
    }
    if pushed {
        path.pop();
    }
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

fn function_name(lang: Lang, node: Node, src: &str) -> String {
    if let Some(name) = node.child_by_field_name("name") {
        let name = text(name, src).to_string();
        if lang == Lang::Go
            && node.kind() == "method_declaration"
            && let Some(recv) = node
                .child_by_field_name("receiver")
                .and_then(|r| find_kind(r, "type_identifier"))
        {
            return format!("{}::{name}", text(recv, src));
        }
        return name;
    }
    if let Some(decl) = node.child_by_field_name("declarator") {
        return declarator_name(decl, src);
    }
    // Anonymous functions take the name they are bound to, if any.
    let mut parent = node.parent();
    while let Some(p) = parent {
        match p.kind() {
            "variable_declarator"
            | "pair"
            | "public_field_definition"
            | "field_definition"
            | "property_definition"
            | "assignment_expression"
            | "init_declarator" => {
                let target = p
                    .child_by_field_name("name")
                    .or_else(|| p.child_by_field_name("key"))
                    .or_else(|| p.child_by_field_name("property"))
                    .or_else(|| p.child_by_field_name("left"))
                    .or_else(|| p.child_by_field_name("declarator"));
                if let Some(t) = target {
                    return text(t, src).to_string();
                }
                break;
            }
            "parenthesized_expression" | "as_expression" | "satisfies_expression" => {
                parent = p.parent();
            }
            _ => break,
        }
    }
    "<anonymous>".to_string()
}

/// The identifier inside a C/C++ declarator chain such as `*foo(int)`.
fn declarator_name(node: Node, src: &str) -> String {
    match node.kind() {
        "identifier"
        | "field_identifier"
        | "qualified_identifier"
        | "operator_name"
        | "destructor_name" => text(node, src).to_string(),
        _ => match node.child_by_field_name("declarator") {
            Some(inner) => declarator_name(inner, src),
            None => text(node, src)
                .split('(')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        },
    }
}

fn container_name(lang: Lang, node: Node, src: &str) -> String {
    if lang == Lang::Rust && node.kind() == "impl_item" {
        let ty = node.child_by_field_name("type").map(|t| text(t, src));
        let tr = node.child_by_field_name("trait").map(|t| text(t, src));
        return match (tr, ty) {
            (Some(tr), Some(ty)) => format!("<{ty} as {tr}>"),
            (None, Some(ty)) => ty.to_string(),
            _ => "<impl>".to_string(),
        };
    }
    node.child_by_field_name("name")
        .map(|n| text(n, src).to_string())
        .unwrap_or_else(|| "<anonymous>".to_string())
}

fn find_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.into_iter().find_map(|c| find_kind(c, kind))
}

fn body_hash(node: Node, src: &str) -> u64 {
    let skip = node.child_by_field_name("name").map(|n| n.id());
    let mut hasher = Fnv::default();
    for_each_token(node, src, &mut |n, t| {
        if Some(n.id()) != skip {
            hasher.write(t.as_bytes());
            hasher.write(&[0]);
        }
    });
    hasher.finish()
}

/// Visit every leaf token under `node`, skipping comments.
pub fn for_each_token<'t>(node: Node<'t>, src: &str, f: &mut impl FnMut(Node<'t>, &str)) {
    if is_comment(node.kind()) {
        return;
    }
    if node.child_count() == 0 {
        let t = text(node, src);
        if !t.trim().is_empty() {
            f(node, t);
        }
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children {
        // A named leaf-like node, such as a string, is one token.
        if child.child_count() > 0 && is_atomic(child.kind()) {
            f(child, text(child, src));
        } else {
            for_each_token(child, src, f);
        }
    }
}

fn is_comment(kind: &str) -> bool {
    kind.contains("comment")
}

fn is_atomic(kind: &str) -> bool {
    matches!(
        kind,
        "string"
            | "string_literal"
            | "raw_string_literal"
            | "template_string"
            | "interpreted_string_literal"
    )
}
