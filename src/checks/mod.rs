//! Built-in checks: function length, file length and duplication.

pub mod exclude;
pub mod length;

pub const FUNCTION_LENGTH: &str = "function-length";
pub const FILE_LENGTH: &str = "file-length";
pub const DUPLICATION: &str = "duplication";
pub const PARSE_ERROR: &str = "parse-error";
