//! Selecting cases by the classification they declare.
//!
//! Filters are domain-neutral: they compare strings the case declares and know
//! nothing about what a project, suite, layer or tag means.

use std::collections::BTreeSet;

use crate::core::case::{Case, LoadedCase};

/// Criteria that select which cases run.
///
/// Within one field the values are alternatives; different fields must all
/// match. `skip_tag` always wins over the other fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// Projects to run. Empty means every project.
    pub project: Vec<String>,
    /// Suites to run. Empty means every suite.
    pub suite: Vec<String>,
    /// Layers to run. Empty means every layer.
    pub layer: Vec<String>,
    /// Tags to run. Empty means every tag.
    pub tag: Vec<String>,
    /// Tags that exclude a case, whatever the other fields say.
    pub skip_tag: Vec<String>,
}

impl Filters {
    /// Whether no filter was given, so every case runs.
    pub fn is_empty(&self) -> bool {
        self.project.is_empty()
            && self.suite.is_empty()
            && self.layer.is_empty()
            && self.tag.is_empty()
            && self.skip_tag.is_empty()
    }

    /// Whether the filters select `case`.
    pub fn matches(&self, case: &Case) -> bool {
        matches_one(&self.project, case.project.as_deref())
            && matches_one(&self.suite, case.suite.as_deref())
            && matches_one(&self.layer, case.layer.as_deref())
            && (self.tag.is_empty() || case.tags.iter().any(|tag| self.tag.contains(tag)))
            && !case.tags.iter().any(|tag| self.skip_tag.contains(tag))
    }

    /// The filters as command-line options, for an error message.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for value in &self.project {
            parts.push(format!("--project {}", quote(value)));
        }
        for value in &self.suite {
            parts.push(format!("--suite {}", quote(value)));
        }
        for value in &self.layer {
            parts.push(format!("--layer {}", quote(value)));
        }
        for value in &self.tag {
            parts.push(format!("--tag {}", quote(value)));
        }
        for value in &self.skip_tag {
            parts.push(format!("--skip-tag {}", quote(value)));
        }
        parts.join(" ")
    }
}

/// Whether `value` satisfies one field's alternatives.
///
/// An empty filter accepts everything; a case that does not declare the field
/// matches no filter for it, because there is nothing to compare.
fn matches_one(wanted: &[String], value: Option<&str>) -> bool {
    wanted.is_empty() || value.is_some_and(|value| wanted.iter().any(|want| want == value))
}

/// The classification values that the discovered cases actually use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Available {
    /// Every declared project, sorted.
    pub projects: BTreeSet<String>,
    /// Every declared suite, sorted.
    pub suites: BTreeSet<String>,
    /// Every declared layer, sorted.
    pub layers: BTreeSet<String>,
    /// Every declared tag, sorted.
    pub tags: BTreeSet<String>,
}

impl Available {
    /// Collects the classification of every loaded case.
    pub fn of(cases: &[LoadedCase]) -> Self {
        let mut available = Self::default();
        for loaded in cases {
            let case = &loaded.case;
            if let Some(project) = &case.project {
                available.projects.insert(project.clone());
            }
            if let Some(suite) = &case.suite {
                available.suites.insert(suite.clone());
            }
            if let Some(layer) = &case.layer {
                available.layers.insert(layer.clone());
            }
            for tag in &case.tags {
                available.tags.insert(tag.clone());
            }
        }
        available
    }
}

/// The message shown when the filters select no case.
///
/// It names the filters and every value that does exist, so a typo is visible
/// without running the cases one by one.
pub fn no_match_message(filters: &Filters, available: &Available) -> String {
    format!(
        "no case matched {}; the discovered cases use project: {}; suite: {}; layer: {}; tags: {}; check the spelling or remove a filter",
        filters.describe(),
        list(&available.projects),
        list(&available.suites),
        list(&available.layers),
        list(&available.tags),
    )
}

fn list(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values
            .iter()
            .map(|value| format!("`{value}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn quote(value: &str) -> String {
    format!("\"{value}\"")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    fn case_with(
        project: Option<&str>,
        suite: Option<&str>,
        layer: Option<&str>,
        tags: &[&str],
    ) -> Case {
        Case {
            name: "case".to_string(),
            vars: BTreeMap::new(),
            project: project.map(str::to_string),
            suite: suite.map(str::to_string),
            layer: layer.map(str::to_string),
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            observe: Vec::new(),
            steps: Vec::new(),
        }
    }

    fn loaded(case: Case) -> LoadedCase {
        LoadedCase {
            path: PathBuf::from("case.yaml"),
            case,
        }
    }

    #[test]
    fn no_filters_select_every_case() {
        let filters = Filters::default();
        let case = case_with(None, None, None, &[]);

        assert!(filters.is_empty());
        assert!(filters.matches(&case));
    }

    #[test]
    fn values_of_one_field_are_alternatives() {
        let filters = Filters {
            layer: vec!["api".to_string(), "cli".to_string()],
            ..Filters::default()
        };

        assert!(filters.matches(&case_with(None, None, Some("api"), &[])));
        assert!(filters.matches(&case_with(None, None, Some("cli"), &[])));
        assert!(!filters.matches(&case_with(None, None, Some("ui"), &[])));
    }

    #[test]
    fn different_fields_must_all_match() {
        let filters = Filters {
            project: vec!["shop".to_string()],
            layer: vec!["api".to_string()],
            tag: vec!["smoke".to_string(), "fast".to_string()],
            ..Filters::default()
        };

        assert!(filters.matches(&case_with(Some("shop"), None, Some("api"), &["fast"])));
        assert!(!filters.matches(&case_with(Some("other"), None, Some("api"), &["fast"])));
        assert!(!filters.matches(&case_with(Some("shop"), None, Some("cli"), &["fast"])));
        assert!(!filters.matches(&case_with(Some("shop"), None, Some("api"), &["slow"])));
    }

    #[test]
    fn a_case_without_the_field_matches_no_filter_for_it() {
        let filters = Filters {
            project: vec!["shop".to_string()],
            ..Filters::default()
        };

        assert!(!filters.matches(&case_with(None, None, None, &[])));
    }

    #[test]
    fn a_tag_filter_needs_one_of_the_tags() {
        let filters = Filters {
            tag: vec!["smoke".to_string(), "fast".to_string()],
            ..Filters::default()
        };

        assert!(filters.matches(&case_with(None, None, None, &["slow", "smoke"])));
        assert!(!filters.matches(&case_with(None, None, None, &["slow"])));
        assert!(!filters.matches(&case_with(None, None, None, &[])));
    }

    #[test]
    fn a_skipped_tag_excludes_the_case_whatever_the_other_fields_say() {
        let filters = Filters {
            layer: vec!["api".to_string()],
            skip_tag: vec!["slow".to_string()],
            ..Filters::default()
        };

        assert!(filters.matches(&case_with(None, None, Some("api"), &["smoke"])));
        assert!(!filters.matches(&case_with(None, None, Some("api"), &["slow"])));
    }

    #[test]
    fn values_match_exactly_and_are_case_sensitive() {
        let filters = Filters {
            layer: vec!["api".to_string()],
            ..Filters::default()
        };

        assert!(!filters.matches(&case_with(None, None, Some("Api"), &[])));
        assert!(!filters.matches(&case_with(None, None, Some("api "), &[])));
    }

    #[test]
    fn available_collects_every_value_once_in_sorted_order() {
        let cases = vec![
            loaded(case_with(
                Some("shop"),
                Some("smoke"),
                Some("api"),
                &["smoke", "fast"],
            )),
            loaded(case_with(Some("billing"), None, Some("cli"), &["fast"])),
            loaded(case_with(None, None, None, &[])),
        ];

        let available = Available::of(&cases);

        assert_eq!(
            available.projects,
            BTreeSet::from(["billing".to_string(), "shop".to_string()])
        );
        assert_eq!(available.suites, BTreeSet::from(["smoke".to_string()]));
        assert_eq!(
            available.layers,
            BTreeSet::from(["api".to_string(), "cli".to_string()])
        );
        assert_eq!(
            available.tags,
            BTreeSet::from(["fast".to_string(), "smoke".to_string()])
        );
    }

    #[test]
    fn the_no_match_message_names_the_filters_and_the_available_values() {
        let filters = Filters {
            layer: vec!["api".to_string()],
            tag: vec!["smoke".to_string()],
            ..Filters::default()
        };
        let available = Available {
            projects: BTreeSet::from(["shop".to_string()]),
            suites: BTreeSet::new(),
            layers: BTreeSet::from(["cli".to_string()]),
            tags: BTreeSet::from(["fast".to_string()]),
        };

        let message = no_match_message(&filters, &available);

        assert!(message.contains("--layer \"api\""), "got: {message}");
        assert!(message.contains("--tag \"smoke\""), "got: {message}");
        assert!(message.contains("`shop`"), "got: {message}");
        assert!(message.contains("suite: none"), "got: {message}");
        assert!(message.contains("`cli`"), "got: {message}");
        assert!(message.contains("`fast`"), "got: {message}");
        assert!(message.contains("check the spelling"), "got: {message}");
    }

    #[test]
    fn a_description_lists_repeated_options_once_per_value() {
        let filters = Filters {
            project: vec!["shop".to_string(), "billing".to_string()],
            skip_tag: vec!["slow".to_string()],
            ..Filters::default()
        };

        assert_eq!(
            filters.describe(),
            "--project \"shop\" --project \"billing\" --skip-tag \"slow\""
        );
    }
}
