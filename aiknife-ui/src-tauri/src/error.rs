use std::{borrow::Cow, sync::Arc};

use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, UiError>;

/// Rust Error type that is specifically designed to bridge from lower-level Rust API error types
/// to something that can serialize to JSON and be used in the Typescript front-end.
#[derive(Error, Debug, strum::AsRefStr, strum::EnumDiscriminants)]
#[strum_discriminants(derive(Serialize, specta::Type), name(UiErrorType))]
pub enum UiError {
    // The underlying crates use anyhow, so this is the most common kind of error
    #[error("{0}")]
    Application(#[from] anyhow::Error),

    /// A tauri::Error, but wrapped in anyhow::Error because we need that representation to
    /// serialize
    #[error("Tauri error: {0}")]
    Tauri(anyhow::Error),

    #[error("some other error: {0}")]
    AnotherError(String),
}

impl UiError {
    /// If there is a longer, more detailed error message describing what went wrong, return it.
    fn error_details(&self) -> Option<String> {
        match self {
            UiError::Application(e) | UiError::Tauri(e) => Some(format!("{:?}", e)),
            UiError::AnotherError(_) => {
                // This is just a string so we don't have any more details
                None
            }
        }
    }
}

impl From<tauri::Error> for UiError {
    fn from(e: tauri::Error) -> Self {
        UiError::Tauri(e.into())
    }
}

impl Serialize for UiError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("UiError", 3)?;

        // Get the variant name using strum
        state.serialize_field("type", self.as_ref())?;

        // Get the error message
        state.serialize_field("message", &self.to_string())?;

        // Get detailed error chain
        state.serialize_field("details", &self.error_details())?;

        state.end()
    }
}

/// Fake struct that matches the shape of the JSON serialized form of [`UiError`].
///
/// Used to generate the `specta::Type` implementation that describes what UiError actually looks
/// like when serialized.  The `specta` crate is definitely *not* intended for hand-rolled impls of
/// `Type`, so this is just cleaner and simpler
#[allow(dead_code)]
#[derive(specta::Type)]
// force export to true to include this as an explicit named type in the generated typescript.
// otherwise it will be used inline as an anonymous type that that's not what we want.
#[specta(rename = "UiError", export = true)]
struct UiErrorDummy {
    pub r#type: UiErrorType,
    pub message: String,
    pub details: Option<String>,
}

impl specta::Type for UiError {
    fn inline(type_map: &mut specta::TypeMap, generics: specta::Generics<'_>) -> specta::DataType {
        UiErrorDummy::inline(type_map, generics)
    }
    fn reference(
        type_map: &mut specta::TypeMap,
        generics: &[specta::DataType],
    ) -> specta::datatype::reference::Reference {
        UiErrorDummy::reference(type_map, generics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn ui_error_serialization() {
        let root_error = std::fs::copy("/tmp/foo/fake/bullshit/noway/nonexistent", "nonexistent2");
        let next_error = root_error.context("i meant to do that");
        let next_error =
            next_error.context("that was never going to work!  what were you thinking");
        let e = UiError::Application(next_error.unwrap_err());
        let json = serde_json::to_string_pretty(&e).unwrap();
        expect_test::expect![[r#"
            {
              "type": "Application",
              "message": "that was never going to work!  what were you thinking",
              "details": "that was never going to work!  what were you thinking\n\nCaused by:\n    0: i meant to do that\n    1: No such file or directory (os error 2)"
            }"#]]
        .assert_eq(&json);
    }
}
