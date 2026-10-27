//! STUB — implemented by the parser agent. See docs/theory.md and the research brief.

use super::{ExportInput, ParseError, ParseOptions};
use crate::model::ParsedExport;
#[allow(unused_imports)]
use std::path::Path;

pub fn parse(_export: &ExportInput, _opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    Err(ParseError::Invalid(
        "whatsapp parser not implemented yet".into(),
    ))
}

/// Whether a zip holds a WhatsApp chat (a .txt whose first lines look like WhatsApp lines).
pub fn zip_has_chat(_zip: &Path) -> bool {
    false
}

/// Whether the first bytes of a .txt look like a WhatsApp export.
pub fn sniff(_head: &[u8]) -> bool {
    false
}
