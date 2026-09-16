//! Common filter types shared between CalDAV and CardDAV
//!
//! This module contains filter structures used by both CalDAV (RFC 4791)
//! and CardDAV (RFC 6352) REPORT requests.

use http::StatusCode;
use xmltree::{Element, XMLNode};

use crate::DavResult;
use crate::errors::DavError;
use crate::xmltree_ext::ElementExt;

/// Text matching filter for property values
///
/// Used in both CalDAV and CardDAV for matching text content in properties.
#[derive(Debug, Clone, Default)]
pub struct TextMatch {
    /// The text to match against
    pub text: String,
    /// Collation to use for comparison (e.g., "i;ascii-casemap")
    pub collation: Option<String>,
    /// If true, the match condition is negated
    pub negate_condition: bool,
    /// Match type for CardDAV: "equals", "contains", "starts-with", "ends-with"
    /// CalDAV uses "contains" by default if not specified
    pub match_type: Option<String>,
}

impl TextMatch {
    /// Build a text-match filter from a `<text-match>` element.
    ///
    /// The element's first text node is the text to match; the
    /// `collation`, `negate-condition` and `match-type` attributes are
    /// copied verbatim.
    pub(crate) fn from_element(elem: &Element) -> Self {
        let text = elem
            .children
            .iter()
            .find_map(|child| match child {
                XMLNode::Text(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();

        TextMatch {
            text,
            collation: elem.attributes.get("collation").cloned(),
            negate_condition: elem
                .attributes
                .get("negate-condition")
                .map(|v| v == "yes")
                .unwrap_or(false),
            match_type: elem.attributes.get("match-type").cloned(),
        }
    }

    /// Whether `value` matches this filter's text under its collation and
    /// match type. `negate_condition` is not applied; see [`Self::matches_any`].
    ///
    /// `i;ascii-casemap`, `i;unicode-casemap` and an unspecified collation
    /// compare case-insensitively; any other collation compares exactly. The
    /// match type defaults to `contains`.
    pub fn matches(&self, value: &str) -> bool {
        let case_insensitive = self.collation.as_deref().is_none_or(|c| {
            c.eq_ignore_ascii_case("i;ascii-casemap") || c.eq_ignore_ascii_case("i;unicode-casemap")
        });
        let (haystack, needle) = if case_insensitive {
            (value.to_lowercase(), self.text.to_lowercase())
        } else {
            (value.to_string(), self.text.clone())
        };
        match self.match_type.as_deref() {
            Some("equals") => haystack == needle,
            Some("starts-with") => haystack.starts_with(&needle),
            Some("ends-with") => haystack.ends_with(&needle),
            _ => haystack.contains(&needle),
        }
    }

    /// Whether any of `values` matches, with `negate_condition` applied to
    /// that overall result rather than to each value (RFC 4791 9.7.5,
    /// RFC 6352 10.5.4): a negated filter matches only when none of the
    /// values match.
    pub fn matches_any<'a>(&self, values: impl IntoIterator<Item = &'a str>) -> bool {
        let any = values.into_iter().any(|v| self.matches(v));
        any != self.negate_condition
    }
}

/// Parameter filter for matching property parameters
///
/// Used in both CalDAV and CardDAV for filtering based on property parameters
/// (e.g., TYPE=HOME on a TEL property).
#[derive(Debug, Clone)]
pub struct ParameterFilter {
    /// Name of the parameter to filter on (e.g., "TYPE")
    pub name: String,
    /// If true, the parameter must NOT be defined
    pub is_not_defined: bool,
    /// Text match filter for the parameter value
    pub text_match: Option<TextMatch>,
}

impl ParameterFilter {
    /// Create a new parameter filter with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_not_defined: false,
            text_match: None,
        }
    }

    /// Build a parameter filter from a `<param-filter>` element.
    ///
    /// Fails with `400 Bad Request` when the mandatory `name` attribute is
    /// missing. Child elements other than `<is-not-defined>` and
    /// `<text-match>` are ignored.
    pub(crate) fn from_element(elem: &Element) -> DavResult<Self> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?;

        let mut filter = ParameterFilter::new(name);

        for child in elem.child_elems_iter() {
            match child.name.as_str() {
                "is-not-defined" => filter.is_not_defined = true,
                "text-match" => filter.text_match = Some(TextMatch::from_element(child)),
                _ => {}
            }
        }

        Ok(filter)
    }
}

/// Collect the text of every `<href>` child of a multiget REPORT body.
pub(crate) fn hrefs_from(root: &Element) -> Vec<String> {
    root.child_elems_iter()
        .filter(|elem| elem.name == "href")
        .flat_map(|elem| {
            elem.children.iter().filter_map(|child| match child {
                XMLNode::Text(href) => Some(href.clone()),
                _ => None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Element {
        Element::parse(xml.as_bytes()).unwrap()
    }

    #[test]
    fn text_match_from_element_reads_text_and_attributes() {
        let tm = TextMatch::from_element(&parse(
            r#"<text-match collation="i;unicode-casemap" negate-condition="yes" match-type="equals">John</text-match>"#,
        ));
        assert_eq!(tm.text, "John");
        assert_eq!(tm.collation.as_deref(), Some("i;unicode-casemap"));
        assert!(tm.negate_condition);
        assert_eq!(tm.match_type.as_deref(), Some("equals"));

        let tm = TextMatch::from_element(&parse("<text-match/>"));
        assert_eq!(tm.text, "");
        assert!(tm.collation.is_none());
        assert!(!tm.negate_condition);
        assert!(tm.match_type.is_none());

        let tm = TextMatch::from_element(&parse(
            r#"<text-match negate-condition="no">x</text-match>"#,
        ));
        assert!(!tm.negate_condition);
    }

    #[test]
    fn param_filter_from_element() {
        let pf = ParameterFilter::from_element(&parse(
            r#"<param-filter name="TYPE"><text-match>WORK</text-match></param-filter>"#,
        ))
        .unwrap();
        assert_eq!(pf.name, "TYPE");
        assert!(!pf.is_not_defined);
        assert_eq!(pf.text_match.unwrap().text, "WORK");

        let pf = ParameterFilter::from_element(&parse(
            r#"<param-filter name="TYPE"><is-not-defined/><unknown/></param-filter>"#,
        ))
        .unwrap();
        assert!(pf.is_not_defined);
        assert!(pf.text_match.is_none());
    }

    #[test]
    fn param_filter_without_name_is_bad_request() {
        match ParameterFilter::from_element(&parse("<param-filter/>")) {
            Err(DavError::StatusClose(StatusCode::BAD_REQUEST)) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn hrefs_from_collects_only_href_children() {
        let root = parse(
            "<multiget><prop><getetag/></prop><href>/a.ics</href><other>/x</other><href>/b.ics</href><href/></multiget>",
        );
        assert_eq!(
            hrefs_from(&root),
            vec!["/a.ics".to_string(), "/b.ics".to_string()]
        );
        assert!(hrefs_from(&parse("<multiget/>")).is_empty());
    }

    fn tm(text: &str, match_type: Option<&str>, negate: bool) -> TextMatch {
        TextMatch {
            text: text.into(),
            collation: None,
            negate_condition: negate,
            match_type: match_type.map(Into::into),
        }
    }

    #[test]
    fn matches_applies_match_type_and_default_collation() {
        assert!(tm("Ann", None, false).matches("hannah"));
        assert!(tm("ann", Some("equals"), false).matches("ANN"));
        assert!(!tm("ann", Some("equals"), false).matches("hannah"));
        assert!(tm("han", Some("starts-with"), false).matches("Hannah"));
        assert!(!tm("nah", Some("starts-with"), false).matches("Hannah"));
        assert!(tm("nah", Some("ends-with"), false).matches("Hannah"));
        assert!(!tm("han", Some("ends-with"), false).matches("Hannah"));
    }

    #[test]
    fn matches_respects_collation() {
        let mut octet = tm("ann", Some("equals"), false);
        octet.collation = Some("i;octet".into());
        assert!(!octet.matches("ANN"));
        assert!(octet.matches("ann"));

        let mut unicode = tm("ann", Some("equals"), false);
        unicode.collation = Some("I;UNICODE-CASEMAP".into());
        assert!(unicode.matches("ANN"));
    }

    #[test]
    fn matches_ignores_negate_condition() {
        assert!(tm("a", Some("equals"), true).matches("a"));
    }

    #[test]
    fn matches_any_negates_over_the_whole_set() {
        let positive = tm("a", Some("equals"), false);
        assert!(positive.matches_any(["a", "b"]));
        assert!(!positive.matches_any(["b", "c"]));
        assert!(!positive.matches_any([]));

        let negated = tm("a", Some("equals"), true);
        assert!(!negated.matches_any(["a", "b"]));
        assert!(negated.matches_any(["b", "c"]));
        assert!(negated.matches_any([]));
    }
}
