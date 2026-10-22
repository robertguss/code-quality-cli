//! TypeScript, TSX and JavaScript rules. The three are separate grammars, and
//! `Query::new` rejects node kinds a grammar lacks, so each gets its own query.

use tree_sitter::Tree;

use super::Hit;
use crate::grammars::Lang;
use crate::rules::Rule;

pub const RULES: &[Rule] = &[];

pub fn hits(_lang: Lang, _tree: &Tree, _src: &str, _out: &mut Vec<Hit>) {}
