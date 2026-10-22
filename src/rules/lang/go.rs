//! Go rules.

use tree_sitter::Tree;

use super::Hit;
use crate::rules::Rule;

pub const RULES: &[Rule] = &[];

pub fn hits(_tree: &Tree, _src: &str, _out: &mut Vec<Hit>) {}
