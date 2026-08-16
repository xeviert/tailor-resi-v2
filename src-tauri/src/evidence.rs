use crate::analysis::JobAnalysis;
use crate::tailoring::TailoringError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct EvidenceBank {
    pub version: u8,
    #[serde(default)]
    pub entries: Vec<EvidenceEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceEntry {
    pub term: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_note: Option<String>,
    pub user_attested: bool,
    #[serde(default)]
    pub allow_model_role_placement: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SelectedEvidence {
    pub term: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_note: Option<String>,
    #[serde(default)]
    pub allow_model_role_placement: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreflightItem {
    pub term: String,
    pub kind: String,
    pub importance: u8,
    pub source: &'static str,
    pub resolution: &'static str,
    pub resolution_reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_term: Option<String>,
    pub proof_note: Option<String>,
    pub eligible_for_bullets: bool,
    pub allow_model_role_placement: bool,
}

#[derive(Clone, Debug)]
struct Candidate {
    term: String,
    kind: String,
    importance: u8,
}

pub fn evidence_bank_path(root: &Path) -> PathBuf {
    root.join("resume").join("evidence-bank.json")
}

pub fn load_evidence_bank(root: &Path) -> Result<EvidenceBank, TailoringError> {
    let path = evidence_bank_path(root);
    if !path.exists() {
        return Ok(EvidenceBank {
            version: 2,
            entries: vec![],
        });
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| TailoringError::Io(error.to_string()))?;
    let mut bank: EvidenceBank = serde_json::from_str(&text)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    if bank.version < 2 {
        bank.version = 2;
    }
    Ok(bank)
}

pub fn save_selected_evidence(
    root: &Path,
    selected: &[SelectedEvidence],
) -> Result<EvidenceBank, TailoringError> {
    let mut bank = load_evidence_bank(root)?;
    for item in selected {
        let term = item.term.trim();
        if term.is_empty() {
            continue;
        }
        let proof_note = clean_proof(item.proof_note.as_deref());
        match bank.entries.iter_mut().find(|entry| {
            equivalent(&entry.term, term)
                || (item.allow_model_role_placement
                    && placement_equivalent_terms(&entry.term, term))
        }) {
            Some(entry) => {
                entry.kind = item.kind.clone();
                if proof_note.is_some() {
                    entry.proof_note = proof_note;
                }
                entry.user_attested = true;
                entry.allow_model_role_placement |= item.allow_model_role_placement;
            }
            None => bank.entries.push(EvidenceEntry {
                term: term.to_string(),
                kind: item.kind.clone(),
                proof_note,
                user_attested: true,
                allow_model_role_placement: item.allow_model_role_placement,
            }),
        }
    }
    bank.version = 2;
    let path = evidence_bank_path(root);
    let text = serde_json::to_string_pretty(&bank)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(bank)
}

pub fn remove_evidence(root: &Path, term: &str) -> Result<EvidenceBank, TailoringError> {
    let mut bank = load_evidence_bank(root)?;
    bank.entries
        .retain(|entry| normalize(&entry.term) != normalize(term));
    let path = evidence_bank_path(root);
    let text = serde_json::to_string_pretty(&bank)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(bank)
}

pub fn preflight_items(
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    bank: &EvidenceBank,
) -> Vec<PreflightItem> {
    let mut candidates = Vec::new();
    candidates.extend(analysis.core_keywords.iter().map(|signal| Candidate {
        term: signal.term.clone(),
        kind: inferred_kind(&signal.term, &group_kind(&signal.category)),
        importance: signal.importance,
    }));
    candidates.extend(
        analysis
            .required_skills
            .iter()
            .map(|term| candidate(term, inferred_kind(term, "technology"), 5)),
    );
    candidates.extend(
        analysis
            .preferred_skills
            .iter()
            .map(|term| candidate(term, inferred_kind(term, "technology"), 3)),
    );
    candidates.extend(
        analysis
            .tools_and_platforms
            .iter()
            .map(|term| candidate(term, "technology", 4)),
    );
    candidates.extend(
        analysis
            .domain_terms
            .iter()
            .map(|term| candidate(term, "method_domain", 3)),
    );
    candidates.extend(
        analysis
            .responsibility_phrases
            .iter()
            .map(|term| candidate(term, "responsibility", 4)),
    );

    let candidates = consolidate_candidates(candidates);
    let base_strings = json_strings(base_resume);
    let role_target = normalize(&analysis.role_target);
    let mut items = candidates
        .into_iter()
        .filter(|candidate| !candidate.term.trim().is_empty())
        .map(|candidate| {
            let base_match = base_strings
                .iter()
                .find(|value| text_supports(value, &candidate.term));
            let bank_entry = bank.entries.iter().find(|entry| {
                entry.user_attested
                    && (equivalent(&entry.term, &candidate.term)
                        || (entry.allow_model_role_placement
                            && placement_equivalent_terms(&entry.term, &candidate.term)))
            });
            let (source, resolution, resolution_reason, matched_term, proof_note) =
                if let Some(base_match) = base_match {
                    (
                        "base_resume",
                        "auto_available",
                        "Supported by the base resume",
                        Some(base_match.clone()),
                        None,
                    )
                } else if let Some(entry) = bank_entry {
                    (
                        "evidence_bank",
                        "auto_available",
                        "Previously confirmed in saved evidence",
                        Some(entry.term.clone()),
                        entry.proof_note.clone(),
                    )
                } else if requires_confirmation(&candidate, &role_target) {
                    (
                        "needs_approval",
                        "confirmation_required",
                        "Important factual claim not found in existing evidence",
                        None,
                        None,
                    )
                } else {
                    (
                        "needs_approval",
                        "auto_omitted",
                        "Low-value, generic, or unsupported signal",
                        None,
                        None,
                    )
                };
            let eligible_for_bullets = source == "base_resume"
                || proof_note.is_some()
                || bank_entry.is_some_and(|entry| entry.allow_model_role_placement);
            PreflightItem {
                term: candidate.term,
                kind: candidate.kind,
                importance: candidate.importance,
                source,
                resolution,
                resolution_reason,
                matched_term,
                proof_note,
                eligible_for_bullets,
                allow_model_role_placement: bank_entry
                    .is_some_and(|entry| entry.allow_model_role_placement),
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        resolution_rank(left.resolution)
            .cmp(&resolution_rank(right.resolution))
            .then_with(|| right.importance.cmp(&left.importance))
            .then_with(|| left.term.cmp(&right.term))
    });
    items
}

pub fn selected_for_prompt(selected: &[SelectedEvidence]) -> Vec<EvidenceEntry> {
    selected
        .iter()
        .filter_map(|item| {
            let term = item.term.trim();
            (!term.is_empty()).then(|| EvidenceEntry {
                term: term.to_string(),
                kind: item.kind.clone(),
                proof_note: clean_proof(item.proof_note.as_deref()),
                user_attested: true,
                allow_model_role_placement: item.allow_model_role_placement,
            })
        })
        .collect()
}

pub fn infer_selected_term_kind(analysis: &JobAnalysis, selected_term: &str) -> String {
    let selected_tokens = placement_token_set(selected_term);
    let mut candidates = Vec::new();
    candidates.extend(analysis.core_keywords.iter().map(|signal| Candidate {
        term: signal.term.clone(),
        kind: inferred_kind(&signal.term, &group_kind(&signal.category)),
        importance: signal.importance,
    }));
    candidates.extend(
        analysis
            .required_skills
            .iter()
            .chain(analysis.preferred_skills.iter())
            .chain(analysis.tools_and_platforms.iter())
            .map(|term| candidate(term, inferred_kind(term, "technology"), 0)),
    );
    candidates.extend(
        analysis
            .domain_terms
            .iter()
            .map(|term| candidate(term, "method_domain", 0)),
    );
    candidates.extend(
        analysis
            .responsibility_phrases
            .iter()
            .map(|term| candidate(term, "responsibility", 0)),
    );

    candidates
        .into_iter()
        .filter_map(|candidate| {
            let candidate_tokens = placement_token_set(&candidate.term);
            let overlap = selected_tokens.intersection(&candidate_tokens).count();
            (overlap > 0).then_some((overlap, candidate_tokens.len(), candidate.kind))
        })
        .max_by_key(|(overlap, token_count, _)| (*overlap, *token_count))
        .map(|(_, _, kind)| kind)
        .unwrap_or_else(|| inferred_kind(selected_term, "method_domain"))
}

pub(crate) fn equivalent_terms(left: &str, right: &str) -> bool {
    equivalent(left, right)
}

pub(crate) fn placement_term_is_covered(text: &str, term: &str) -> bool {
    let text_tokens = token_set(text);
    let term_tokens = placement_token_set(term);
    !term_tokens.is_empty() && term_tokens.is_subset(&text_tokens)
}

pub(crate) fn placement_equivalent_terms(left: &str, right: &str) -> bool {
    let left = placement_token_set(left);
    let right = placement_token_set(right);
    !left.is_empty() && left == right
}

fn placement_token_set(value: &str) -> BTreeSet<String> {
    const PLACEMENT_WRAPPERS: &[&str] = &[
        "practice",
        "practical",
        "experience",
        "experienced",
        "pratique",
        "expérience",
        "dans",
        "dan",
        "du",
        "de",
        "des",
        "la",
        "le",
        "les",
        "l",
        "d",
        "en",
        "au",
        "aux",
        "maîtrise",
    ];
    token_set(value)
        .into_iter()
        .filter(|token| !PLACEMENT_WRAPPERS.contains(&token.as_str()))
        .collect()
}

fn candidate(term: &str, kind: impl Into<String>, importance: u8) -> Candidate {
    Candidate {
        term: term.trim().to_string(),
        kind: kind.into(),
        importance,
    }
}

fn consolidate_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut consolidated: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if let Some(existing) = consolidated
            .iter_mut()
            .find(|existing| equivalent(&existing.term, &candidate.term))
        {
            existing.importance = existing.importance.max(candidate.importance);
            existing.kind = merge_kind(&existing.kind, &candidate.kind);
            if display_quality(&candidate.term) > display_quality(&existing.term) {
                existing.term = candidate.term;
            }
        } else {
            consolidated.push(candidate);
        }
    }
    consolidated
}

fn merge_kind(left: &str, right: &str) -> String {
    let rank = |kind: &str| match kind {
        "responsibility" => 3,
        "method_domain" => 2,
        _ => 1,
    };
    if rank(right) > rank(left) {
        right.to_string()
    } else {
        left.to_string()
    }
}

fn display_quality(term: &str) -> (bool, usize) {
    let words = term.split_whitespace().count();
    (words <= 6, usize::MAX - term.len().min(usize::MAX))
}

fn requires_confirmation(candidate: &Candidate, role_target: &str) -> bool {
    if candidate.importance < 4 || normalize(&candidate.term) == role_target {
        return false;
    }
    if is_generic_trait(&candidate.term) || is_job_title(&candidate.term) {
        return false;
    }
    match candidate.kind.as_str() {
        "technology" => true,
        "method_domain" => candidate.importance == 5,
        "responsibility" => {
            candidate.importance == 5 || is_specific_responsibility(&candidate.term)
        }
        _ => false,
    }
}

fn is_generic_trait(term: &str) -> bool {
    let value = term.to_lowercase();
    [
        "curiosity",
        "interest in",
        "getting things done",
        "track record",
        "strong communication",
        "ability to prioritize",
        "fast-paced",
        "high-stakes",
        "added-value",
    ]
    .iter()
    .any(|generic| value.contains(generic))
}

fn is_job_title(term: &str) -> bool {
    let value = term.trim().to_lowercase();
    let words = value.split_whitespace().count();
    words <= 6
        && [
            " engineer",
            " developer",
            " manager",
            " architect",
            " specialist",
        ]
        .iter()
        .any(|suffix| value.ends_with(suffix))
}

fn is_specific_responsibility(term: &str) -> bool {
    let value = term.to_lowercase();
    [
        "manage ",
        "lead ",
        "own ",
        "operate ",
        "deploy",
        "security",
        "compliance",
        "budget",
        "hire",
        "mentor",
        "direct reports",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn inferred_kind(term: &str, fallback: &str) -> String {
    let value = term.trim().to_lowercase();
    if [
        "build ",
        "design ",
        "manage ",
        "lead ",
        "own ",
        "collaborate ",
        "ensure ",
        "implement ",
        "contribute ",
        "participate ",
        "create ",
        "define ",
        "support ",
        "translate ",
        "review ",
        "prioritize ",
        "report ",
        "raise ",
        "bring ",
        "identify ",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
    {
        "responsibility".to_string()
    } else if [
        "ownership",
        "reliability",
        "maintainability",
        "scalability",
        "collaboration",
        "secure coding",
        "technical debt",
        "automated testing",
        "agile",
        "scrum",
        "kanban",
        "operational excellence",
        "stakeholder management",
        "system design",
        "architecture discussion",
        "ai fluency",
    ]
    .iter()
    .any(|marker| value.contains(marker))
    {
        "method_domain".to_string()
    } else {
        fallback.to_string()
    }
}

fn group_kind(category: &str) -> String {
    let category = category.to_lowercase();
    if category.contains("responsib") {
        "responsibility".to_string()
    } else if category.contains("domain") || category.contains("method") {
        "method_domain".to_string()
    } else {
        "technology".to_string()
    }
}

fn json_strings(value: &serde_json::Value) -> Vec<String> {
    let mut strings = Vec::new();
    collect_json_strings(value, &mut strings);
    strings
}

fn collect_json_strings(value: &serde_json::Value, strings: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => strings.push(value.clone()),
        serde_json::Value::Array(values) => {
            for value in values {
                collect_json_strings(value, strings);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_json_strings(value, strings);
            }
        }
        _ => {}
    }
}

fn text_supports(text: &str, term: &str) -> bool {
    let text_tokens = token_set(text);
    let term_tokens = token_set(term);
    !term_tokens.is_empty() && term_tokens.is_subset(&text_tokens)
}

fn equivalent(left: &str, right: &str) -> bool {
    if normalize(left) == normalize(right) {
        return true;
    }
    let left = token_set(left);
    let right = token_set(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let overlap = left.intersection(&right).count();
    let smaller = left.len().min(right.len());
    smaller >= 2 && overlap == smaller && smaller * 4 >= left.len().max(right.len()) * 3
}

fn token_set(value: &str) -> BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "the",
        "of",
        "to",
        "for",
        "with",
        "in",
        "on",
        "or",
        "your",
        "strong",
        "proven",
        "significant",
        "experience",
        "experienced",
        "knowledge",
        "ability",
        "skills",
        "skill",
        "current",
        "modern",
        "professional",
        "written",
        "spoken",
        "fluency",
        "fluent",
        "practices",
        "principles",
        "behind",
        "such",
        "as",
    ];
    value
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric() && ch != '+' && ch != '#' && ch != '.')
        .map(|word| word.trim_matches('.'))
        .filter(|word| !word.is_empty() && !STOP_WORDS.contains(word))
        .map(|word| word.trim_end_matches('s').to_string())
        .collect()
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn clean_proof(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolution_rank(resolution: &str) -> u8 {
    match resolution {
        "confirmation_required" => 0,
        "auto_available" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::KeywordSignal;

    fn analysis(signals: Vec<KeywordSignal>) -> JobAnalysis {
        JobAnalysis {
            role_target: "Rust Engineer".into(),
            seniority: String::new(),
            core_keywords: signals,
            required_skills: vec![],
            preferred_skills: vec![],
            tools_and_platforms: vec![],
            domain_terms: vec![],
            responsibility_phrases: vec![],
            achievement_angles: vec![],
            ats_phrase_bank: vec![],
            must_not_claim_without_evidence: vec![],
            summary: String::new(),
        }
    }

    fn signal(term: &str, category: &str, importance: u8) -> KeywordSignal {
        KeywordSignal {
            term: term.into(),
            category: category.into(),
            importance,
            evidence: String::new(),
        }
    }

    #[test]
    fn resolves_base_bank_confirmation_and_omission() {
        let analysis = analysis(vec![
            signal("React", "technology", 5),
            signal("Kubernetes", "technology", 4),
            signal("Terraform", "technology", 5),
            signal("Curiosity about AI", "technology", 4),
        ]);
        let bank = EvidenceBank {
            version: 1,
            entries: vec![EvidenceEntry {
                term: "Kubernetes".into(),
                kind: "technology".into(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: false,
            }],
        };
        let items = preflight_items(
            &analysis,
            &serde_json::json!({"skills":{"frontend":"React, TypeScript"}}),
            &bank,
        );

        assert_eq!(
            items
                .iter()
                .find(|item| item.term == "React")
                .unwrap()
                .resolution,
            "auto_available"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.term == "Kubernetes")
                .unwrap()
                .source,
            "evidence_bank"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.term == "Terraform")
                .unwrap()
                .resolution,
            "confirmation_required"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.term == "Curiosity about AI")
                .unwrap()
                .resolution,
            "auto_omitted"
        );
    }

    #[test]
    fn consolidates_redundant_phrases_and_keeps_highest_importance() {
        let mut analysis = analysis(vec![signal("English fluency", "technology", 3)]);
        analysis.required_skills = vec!["Fluent written and spoken English".into()];

        let items = preflight_items(&analysis, &serde_json::json!({}), &EvidenceBank::default());

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].importance, 5);
    }

    #[test]
    fn required_responsibility_is_not_mislabeled_as_technology() {
        let mut analysis = analysis(vec![]);
        analysis.required_skills = vec!["Lead cross-team delivery".into()];

        let items = preflight_items(&analysis, &serde_json::json!({}), &EvidenceBank::default());

        assert_eq!(items[0].kind, "responsibility");
        assert_eq!(items[0].resolution, "confirmation_required");
    }

    #[test]
    fn low_priority_unsupported_signal_is_omitted_without_a_question() {
        let analysis = analysis(vec![signal("GraphQL", "technology", 3)]);
        let items = preflight_items(&analysis, &serde_json::json!({}), &EvidenceBank::default());
        assert_eq!(items[0].resolution, "auto_omitted");
    }

    #[test]
    fn abstract_capability_is_not_treated_as_a_technology_question() {
        let analysis = analysis(vec![signal(
            "Clear ownership and reliability",
            "technology",
            4,
        )]);
        let items = preflight_items(&analysis, &serde_json::json!({}), &EvidenceBank::default());

        assert_eq!(items[0].kind, "method_domain");
        assert_eq!(items[0].resolution, "auto_omitted");
    }

    #[test]
    fn saved_responsibility_needs_a_proof_note_for_bullet_use() {
        let analysis = analysis(vec![signal(
            "Own production deployment",
            "responsibility",
            5,
        )]);
        let bank = EvidenceBank {
            version: 1,
            entries: vec![EvidenceEntry {
                term: "Production deployment ownership".into(),
                kind: "responsibility".into(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: false,
            }],
        };
        let items = preflight_items(&analysis, &serde_json::json!({}), &bank);
        assert!(!items[0].eligible_for_bullets);
    }

    #[test]
    fn saving_an_equivalent_attestation_updates_instead_of_growing_the_bank() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-evidence-{suffix}"));
        std::fs::create_dir_all(root.join("resume")).unwrap();
        std::fs::write(
            evidence_bank_path(&root),
            serde_json::to_string(&EvidenceBank {
                version: 1,
                entries: vec![EvidenceEntry {
                    term: "English fluency".into(),
                    kind: "technology".into(),
                    proof_note: None,
                    user_attested: true,
                    allow_model_role_placement: false,
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let bank = save_selected_evidence(
            &root,
            &[SelectedEvidence {
                term: "Fluent written and spoken English".into(),
                kind: "technology".into(),
                proof_note: Some("Used professionally".into()),
                allow_model_role_placement: false,
            }],
        )
        .unwrap();

        assert_eq!(bank.entries.len(), 1);
        assert_eq!(bank.version, 2);
        assert!(!bank.entries[0].allow_model_role_placement);
        assert_eq!(bank.entries[0].term, "English fluency");
        assert_eq!(
            bank.entries[0].proof_note.as_deref(),
            Some("Used professionally")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_placement_authorization_makes_saved_evidence_bullet_eligible() {
        let analysis = analysis(vec![signal("Angular", "technology", 5)]);
        let bank = EvidenceBank {
            version: 2,
            entries: vec![EvidenceEntry {
                term: "Angular".into(),
                kind: "technology".into(),
                proof_note: None,
                user_attested: true,
                allow_model_role_placement: true,
            }],
        };

        let items = preflight_items(&analysis, &serde_json::json!({}), &bank);

        assert!(items[0].eligible_for_bullets);
        assert!(items[0].allow_model_role_placement);
    }

    #[test]
    fn placement_coverage_ignores_experience_wrapper_words() {
        assert!(placement_term_is_covered(
            "Développé des interfaces Angular pour des applications métier.",
            "Angular dans l’expérience"
        ));
        assert!(placement_term_is_covered(
            "Applied Domain-Driven Design to modular backend services.",
            "pratique du Domain-Driven Design dans l’expérience"
        ));
    }

    #[test]
    fn selected_experience_phrase_inherits_the_underlying_skill_kind() {
        let mut analysis = analysis(vec![]);
        analysis.required_skills = vec!["Angular".into()];

        assert_eq!(
            infer_selected_term_kind(&analysis, "Angular dans l’expérience"),
            "technology"
        );
    }

    #[test]
    fn placement_attestation_updates_the_underlying_saved_capability() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-placement-evidence-{suffix}"));
        std::fs::create_dir_all(root.join("resume")).unwrap();
        std::fs::write(
            evidence_bank_path(&root),
            serde_json::to_string(&EvidenceBank {
                version: 1,
                entries: vec![EvidenceEntry {
                    term: "Angular".into(),
                    kind: "technology".into(),
                    proof_note: None,
                    user_attested: true,
                    allow_model_role_placement: false,
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let bank = save_selected_evidence(
            &root,
            &[SelectedEvidence {
                term: "Angular dans l’expérience".into(),
                kind: "technology".into(),
                proof_note: None,
                allow_model_role_placement: true,
            }],
        )
        .unwrap();

        assert_eq!(bank.entries.len(), 1);
        assert_eq!(bank.entries[0].term, "Angular");
        assert!(bank.entries[0].allow_model_role_placement);
        std::fs::remove_dir_all(root).unwrap();
    }
}
