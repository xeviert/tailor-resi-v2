use crate::analysis::JobAnalysis;
use crate::tailoring::TailoringError;
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Write;
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
pub(crate) struct Candidate {
    pub(crate) term: String,
    pub(crate) kind: String,
    pub(crate) importance: u8,
    /// Which part of the analysis this term came from. `kind` says what sort of thing it is;
    /// `group` says how the job post asked for it, which is what a coverage report needs.
    pub(crate) group: &'static str,
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
    write_evidence_bank(root, &bank)?;
    Ok(bank)
}

pub fn remove_evidence(root: &Path, term: &str) -> Result<EvidenceBank, TailoringError> {
    let mut bank = load_evidence_bank(root)?;
    // Removal has to use the matcher `save_selected_evidence` upserts with. With exact
    // normalized equality here, an entry saved under a fuzzy match could not be deleted at all.
    bank.entries.retain(|entry| {
        !equivalent(&entry.term, term) && !placement_equivalent_terms(&entry.term, term)
    });
    write_evidence_bank(root, &bank)?;
    Ok(bank)
}

/// Writes the bank atomically. It is a single file holding every claim the user has ever
/// attested to, fully rewritten on each save, so a crash mid-write would take the whole
/// history with it.
fn write_evidence_bank(root: &Path, bank: &EvidenceBank) -> Result<(), TailoringError> {
    let text = serde_json::to_string_pretty(bank)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    AtomicFile::new(evidence_bank_path(root), AllowOverwrite)
        .write(|output| output.write_all(format!("{text}\n").as_bytes()))
        .map_err(|error| TailoringError::Io(error.to_string()))
}

/// The weighted, de-duplicated set of terms a job post asks for.
///
/// The evidence preflight and the ATS scorer both work from this list. Keeping it in one
/// place is what stops the two from disagreeing about what the job actually wants — the
/// preflight asking the user to confirm one set of terms while the score measures another.
pub(crate) fn analysis_candidates(analysis: &JobAnalysis) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    candidates.extend(analysis.core_keywords.iter().map(|signal| Candidate {
        term: signal.term.clone(),
        kind: inferred_kind(&signal.term, &group_kind(&signal.category)),
        importance: signal.importance,
        group: "core",
    }));
    candidates.extend(
        analysis
            .required_skills
            .iter()
            .map(|term| candidate(term, inferred_kind(term, "technology"), 5, "required")),
    );
    candidates.extend(
        analysis
            .preferred_skills
            .iter()
            .map(|term| candidate(term, inferred_kind(term, "technology"), 3, "preferred")),
    );
    candidates.extend(
        analysis
            .tools_and_platforms
            .iter()
            .map(|term| candidate(term, "technology", 4, "tools")),
    );
    candidates.extend(
        analysis
            .domain_terms
            .iter()
            .map(|term| candidate(term, "method_domain", 3, "domain")),
    );
    candidates.extend(
        analysis
            .responsibility_phrases
            .iter()
            .map(|term| candidate(term, "responsibility", 4, "responsibilities")),
    );

    consolidate_candidates(candidates)
}

pub fn preflight_items(
    analysis: &JobAnalysis,
    base_resume: &serde_json::Value,
    bank: &EvidenceBank,
) -> Vec<PreflightItem> {
    let candidates = analysis_candidates(analysis);
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

/// Assembles the evidence entries handed to the tailoring model.
///
/// Two sources are merged: entries the preflight already resolved from the saved bank, and
/// the selections the user just confirmed in this session. Both tailoring entry points must
/// use this, otherwise a first run sees only the session's selections while a re-tailor sees
/// the whole bank, and the two runs are not comparable.
pub fn approved_evidence_for(
    preflight: &[PreflightItem],
    bank: &EvidenceBank,
    selected: &[SelectedEvidence],
) -> Vec<EvidenceEntry> {
    let mut approved = preflight
        .iter()
        .filter(|item| item.source == "evidence_bank")
        .filter_map(|item| item.matched_term.as_deref())
        .filter_map(|matched| {
            bank.entries
                .iter()
                .find(|entry| equivalent_terms(&entry.term, matched))
                .cloned()
        })
        .collect::<Vec<EvidenceEntry>>();

    for entry in selected_for_prompt(selected) {
        if !approved
            .iter()
            .any(|existing| equivalent_terms(&existing.term, &entry.term))
        {
            approved.push(entry);
        }
    }
    approved
}

/// Resolves already-attested bank entries for terms the caller names directly, for cases where
/// the term is known but no `SelectedEvidence` record was built for it.
pub fn append_banked_terms(
    approved: &mut Vec<EvidenceEntry>,
    bank: &EvidenceBank,
    terms: &[String],
) {
    for term in terms {
        if let Some(entry) = bank.entries.iter().find(|entry| {
            equivalent_terms(&entry.term, term) || placement_equivalent_terms(&entry.term, term)
        }) {
            if !approved
                .iter()
                .any(|existing| equivalent_terms(&existing.term, &entry.term))
            {
                approved.push(entry.clone());
            }
        }
    }
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
    analysis_candidates(analysis)
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

fn candidate(
    term: &str,
    kind: impl Into<String>,
    importance: u8,
    group: &'static str,
) -> Candidate {
    Candidate {
        term: term.trim().to_string(),
        kind: kind.into(),
        importance,
        group,
    }
}

fn consolidate_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut consolidated: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if let Some(existing) = consolidated
            .iter_mut()
            .find(|existing| equivalent(&existing.term, &candidate.term))
        {
            if candidate.importance > existing.importance {
                existing.group = candidate.group;
            }
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

/// Soft-skill boilerplate that reads as a requirement but carries no ATS keyword value.
///
/// French entries matter as much as English ones: the app ships an FR resume path and French
/// posts run through this same filter, so an English-only list lets FR boilerplate through to
/// the confirmation queue and buries the signals that are actually worth the user's attention.
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
        "curiosité",
        "esprit d'équipe",
        "esprit d’équipe",
        "autonomie",
        "rigueur",
        "force de proposition",
        "aisance relationnelle",
        "sens du service",
        "intérêt pour",
        "environnement dynamique",
        "valeur ajoutée",
    ]
    .iter()
    .any(|generic| value.contains(generic))
}

fn is_job_title(term: &str) -> bool {
    let value = term.trim().to_lowercase();
    let words = value.split_whitespace().count();
    words <= 6
        && ([
            " engineer",
            " developer",
            " manager",
            " architect",
            " specialist",
            " ingénieur",
            " développeur",
            " développeuse",
            " architecte",
            " responsable",
            " consultant",
        ]
        .iter()
        .any(|suffix| value.ends_with(suffix))
            // French titles lead with the role word rather than trailing it.
            || [
                "ingénieur ",
                "développeur ",
                "développeuse ",
                "architecte ",
                "chef de projet",
                "responsable ",
            ]
            .iter()
            .any(|prefix| value.starts_with(prefix)))
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
        "gérer",
        "diriger",
        "piloter",
        "encadrer",
        "déployer",
        "sécurité",
        "conformité",
        "recruter",
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
        "construire ",
        "concevoir ",
        "développer ",
        "gérer ",
        "diriger ",
        "piloter ",
        "encadrer ",
        "assurer ",
        "mettre en ",
        "participer ",
        "contribuer ",
        "collaborer ",
        "définir ",
        "créer ",
        "maintenir ",
        "accompagner ",
        "améliorer ",
        "identifier ",
        "déployer ",
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
        "fiabilité",
        "maintenabilité",
        "évolutivité",
        "scalabilité",
        "dette technique",
        "tests automatisés",
        "revue de code",
        "conception système",
        "méthodologie agile",
        "accessibilité",
        "qualité de code",
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

pub(crate) fn text_supports(text: &str, term: &str) -> bool {
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

/// Splits text into the comparable token set used by every matcher in this module.
///
/// The stop list is bilingual on purpose. `text_supports` demands that *every* token of a term
/// appear in the text, so an unfiltered French article is enough to make a real match fail —
/// "gestion de projet" would never match a bullet that says "gestion projet".
pub(crate) fn token_set(value: &str) -> BTreeSet<String> {
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
        // French
        "de",
        "des",
        "du",
        "le",
        "la",
        "les",
        "un",
        "une",
        "et",
        "ou",
        "au",
        "aux",
        "pour",
        "avec",
        "dans",
        "sur",
        "vos",
        "votre",
        "solide",
        "bonne",
        "bonnes",
        "connaissance",
        "connaissances",
        "maîtrise",
        "capacité",
        "compétence",
        "compétences",
        "expérience",
        "expériences",
        "professionnel",
        "professionnelle",
    ];
    strip_inclusive_suffixes(&value.to_lowercase())
        .split(|ch: char| !ch.is_alphanumeric() && ch != '+' && ch != '#' && ch != '.')
        .map(|word| word.trim_matches('.'))
        .filter(|word| !word.is_empty() && !STOP_WORDS.contains(word))
        .map(|word| word.trim_end_matches('s').to_string())
        .collect()
}

/// Removes French inclusive-writing suffixes so the base word survives tokenization.
///
/// French job posts routinely write "référent·e", "ingénieur·e·s", "développeur·euse". Splitting
/// on the middle dot leaves a stray one- or two-letter token and makes the term fail to match a
/// resume that simply writes "référent". Only a short alphabetic run after the dot counts as a
/// suffix, so a middle dot used as a genuine separator still splits tokens as before.
fn strip_inclusive_suffixes(value: &str) -> String {
    const SEPARATORS: [char; 3] = ['·', '‧', '•'];
    const MAX_SUFFIX: usize = 5;

    if !value.contains(SEPARATORS) {
        return value.to_string();
    }

    let characters = value.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if !SEPARATORS.contains(&character) {
            out.push(character);
            index += 1;
            continue;
        }
        let suffix = characters[index + 1..]
            .iter()
            .take_while(|next| next.is_alphabetic())
            .count();
        if suffix > 0 && suffix <= MAX_SUFFIX {
            index += 1 + suffix;
        } else {
            out.push(' ');
            index += 1;
        }
    }
    out
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
            term_variants: vec![],
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

    fn bank_with(entries: Vec<EvidenceEntry>) -> EvidenceBank {
        EvidenceBank {
            version: 2,
            entries,
        }
    }

    fn banked(term: &str) -> EvidenceEntry {
        EvidenceEntry {
            term: term.into(),
            kind: "technology".into(),
            proof_note: None,
            user_attested: true,
            allow_model_role_placement: false,
        }
    }

    fn banked_item(term: &str) -> PreflightItem {
        PreflightItem {
            term: term.into(),
            kind: "technology".into(),
            importance: 5,
            source: "evidence_bank",
            resolution: "auto_available",
            resolution_reason: "Previously confirmed in saved evidence",
            matched_term: Some(term.into()),
            proof_note: None,
            eligible_for_bullets: false,
            allow_model_role_placement: false,
        }
    }

    fn selection(term: &str) -> SelectedEvidence {
        SelectedEvidence {
            term: term.into(),
            kind: "technology".into(),
            proof_note: None,
            allow_model_role_placement: false,
        }
    }

    #[test]
    fn approved_evidence_merges_banked_entries_with_this_sessions_selections() {
        let bank = bank_with(vec![banked("Kubernetes"), banked("Terraform")]);
        let preflight = vec![banked_item("Kubernetes")];

        let approved = approved_evidence_for(&preflight, &bank, &[selection("gRPC")]);
        let terms = approved
            .iter()
            .map(|entry| entry.term.as_str())
            .collect::<Vec<_>>();

        // The banked term is the regression this guards. A first tailoring run used to pass
        // only the session's selections, so previously attested evidence never reached the
        // model and only a re-tailor saw the full bank.
        assert!(terms.contains(&"Kubernetes"), "{terms:?}");
        assert!(terms.contains(&"gRPC"), "{terms:?}");
        assert!(
            !terms.contains(&"Terraform"),
            "bank entries this job did not match stay out: {terms:?}"
        );
    }

    #[test]
    fn approved_evidence_does_not_duplicate_a_term_present_in_both_sources() {
        let bank = bank_with(vec![banked("Kubernetes")]);
        let preflight = vec![banked_item("Kubernetes")];
        let approved = approved_evidence_for(&preflight, &bank, &[selection("Kubernetes")]);
        assert_eq!(approved.len(), 1);
    }

    #[test]
    fn append_banked_terms_resolves_terms_named_without_a_selection_record() {
        let bank = bank_with(vec![banked("Angular")]);
        let mut approved = Vec::new();
        append_banked_terms(&mut approved, &bank, &["Angular".to_string()]);
        assert_eq!(approved.len(), 1);
        append_banked_terms(&mut approved, &bank, &["Angular".to_string()]);
        assert_eq!(approved.len(), 1, "a repeated term must not be added twice");
    }

    #[test]
    fn french_boilerplate_is_filtered_like_its_english_equivalent() {
        assert!(is_generic_trait("Curiosité intellectuelle"));
        assert!(is_generic_trait("Esprit d’équipe"));
        assert!(is_generic_trait("Autonomie et rigueur"));
        assert!(is_job_title("Ingénieur logiciel"));
        assert!(is_job_title("Architecte technique"));
        assert!(is_job_title("Chef de projet technique"));
        assert!(!is_generic_trait("Kubernetes"));
        assert!(!is_job_title("Kubernetes"));
    }

    #[test]
    fn french_responsibility_verbs_are_classified_as_responsibilities() {
        assert_eq!(
            inferred_kind("Développer des interfaces accessibles", "technology"),
            "responsibility"
        );
        assert_eq!(
            inferred_kind("Encadrer une équipe de développeurs", "technology"),
            "responsibility"
        );
        assert_eq!(inferred_kind("PostgreSQL", "technology"), "technology");
    }

    #[test]
    fn french_articles_do_not_block_a_genuine_match() {
        // `text_supports` demands every token of the term, so an unfiltered "de" would sink
        // this match even though the bullet plainly supports the claim.
        assert!(text_supports(
            "Gestion projet et animation d’atelier",
            "gestion de projet"
        ));
    }

    #[test]
    fn a_fuzzily_saved_entry_can_still_be_removed() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("resume-evidence-remove-{suffix}"));
        std::fs::create_dir_all(root.join("resume")).unwrap();
        save_selected_evidence(
            &root,
            &[SelectedEvidence {
                term: "Angular".into(),
                kind: "technology".into(),
                proof_note: None,
                allow_model_role_placement: true,
            }],
        )
        .unwrap();

        // The UI offers the term as the preflight labelled it, which is not always the exact
        // string that landed in the bank. Exact-equality removal could not delete this.
        let bank = remove_evidence(&root, "Angular dans l’expérience").unwrap();

        assert!(bank.entries.is_empty(), "{:?}", bank.entries);
        std::fs::remove_dir_all(root).unwrap();
    }
}
