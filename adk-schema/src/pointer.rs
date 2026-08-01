//! JSON Pointer construction and escaping, per RFC 6901.

use std::borrow::Cow;

/// Escapes a reference token: `~` becomes `~0` and `/` becomes `~1`.
///
/// Borrows when the token contains neither, which is the common case.
pub(crate) fn escape_token(token: &str) -> Cow<'_, str> {
    if token.contains(['~', '/']) {
        Cow::Owned(token.replace('~', "~0").replace('/', "~1"))
    } else {
        Cow::Borrowed(token)
    }
}

/// Reverses [`escape_token`]. `~1` is replaced before `~0`, so an escaped `~1`
/// does not become a separator.
pub(crate) fn unescape_token(token: &str) -> Cow<'_, str> {
    if token.contains('~') {
        Cow::Owned(token.replace("~1", "/").replace("~0", "~"))
    } else {
        Cow::Borrowed(token)
    }
}

/// Appends an escaped token to a parent pointer.
///
/// An empty parent yields `/token`, the pointer to a root member, so the root
/// needs no special case.
pub(crate) fn child(parent: &str, token: &str) -> String {
    format!("{parent}/{}", escape_token(token))
}

/// The last reference token of a pointer, unescaped.
pub(crate) fn last_token(pointer: &str) -> String {
    unescape_token(pointer.rsplit('/').next().unwrap_or_default()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    mod escape_token {
        use super::*;

        #[test]
        fn borrows_a_token_needing_no_escape() {
            assert!(matches!(escape_token("plain"), Cow::Borrowed("plain")));
        }

        #[test]
        fn escapes_a_separator() {
            assert_eq!(escape_token("a/b"), "a~1b");
        }

        #[test]
        fn escapes_a_tilde() {
            assert_eq!(escape_token("a~b"), "a~0b");
        }
    }

    mod round_trip {
        use super::*;

        /// `~1` must survive escaping: a naive unescape order turns it into a
        /// separator.
        #[test]
        fn a_literal_tilde_one_survives() {
            let original = "a~1b";

            assert_eq!(unescape_token(&escape_token(original)), original);
        }

        #[test]
        fn a_separator_survives() {
            let original = "a/b";

            assert_eq!(unescape_token(&escape_token(original)), original);
        }
    }

    mod child {
        use super::*;

        #[test]
        fn an_empty_parent_yields_a_root_member() {
            assert_eq!(child("", "properties"), "/properties");
        }

        #[test]
        fn a_nested_parent_appends() {
            assert_eq!(child("/properties", "name"), "/properties/name");
        }
    }

    mod last_token {
        use super::*;

        #[test]
        fn returns_the_unescaped_name() {
            assert_eq!(last_token("/properties/a~1b"), "a/b");
        }
    }
}
