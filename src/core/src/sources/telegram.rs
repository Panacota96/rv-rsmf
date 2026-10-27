//! STUB — implemented by the parser agent. See docs/theory.md and the research brief.

use super::{ExportInput, ParseError, ParseOptions};
use crate::model::ParsedExport;
#[allow(unused_imports)]
use std::path::Path;

pub fn parse(_export: &ExportInput, _opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    Err(ParseError::Invalid(
        "telegram parser not implemented yet".into(),
    ))
}
