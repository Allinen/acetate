//! The track editor's engine, with no UI: the operations a change is made of (`model`), the
//! line they are typed in (`parse`) and what its words mean (`resolve`), the credits found in
//! titles (`title_parse`), the work with the server (`engine`: loading, planning, previews,
//! applying) and what changed in words (`diff`). The desktop draws it; the phone will too.

pub mod diff;
pub mod engine;
pub mod forms;
pub mod import;
pub mod model;
pub mod page_key;
pub mod panel;
pub mod parse;
pub mod resolve;
pub mod session;
pub mod source_line;
pub mod title_parse;

/// A word with its first letter in capitals.
pub fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The kinds a grouping can be.
pub const GROUPING_KINDS: [&str; 3] = ["collective", "era", "project"];
