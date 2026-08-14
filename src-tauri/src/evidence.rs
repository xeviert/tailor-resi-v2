use crate::analysis::JobAnalysis;
use crate::tailoring::TailoringError;
use serde::{Deserialize, Serialize};
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SelectedEvidence {
    pub term: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreflightItem {
    pub term: String,
    pub kind: String,
    pub importance: u8,
    pub source: &'static str,
    pub proof_note: Option<String>,
    pub eligible_for_bullets: bool,
}

pub fn evidence_bank_path(root: &Path) -> PathBuf {
    root.join("resume").join("evidence-bank.json")
}

pub fn load_evidence_bank(root: &Path) -> Result<EvidenceBank, TailoringError> {
    let path = evidence_bank_path(root);
    if !path.exists() {
        return Ok(EvidenceBank { version: 1, entries: vec![] });
    }
    let text = std::fs::read_to_string(path).map_err(|error| TailoringError::Io(error.to_string()))?;
    let mut bank: EvidenceBank = serde_json::from_str(&text)
        .map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    if bank.version == 0 { bank.version = 1; }
    Ok(bank)
}

pub fn save_selected_evidence(root: &Path, selected: &[SelectedEvidence]) -> Result<EvidenceBank, TailoringError> {
    let mut bank = load_evidence_bank(root)?;
    for item in selected {
        let term = item.term.trim();
        if term.is_empty() { continue; }
        let proof_note = item.proof_note.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned);
        match bank.entries.iter_mut().find(|entry| normalize(&entry.term) == normalize(term)) {
            Some(entry) => {
                entry.kind = item.kind.clone();
                if proof_note.is_some() { entry.proof_note = proof_note; }
                entry.user_attested = true;
            }
            None => bank.entries.push(EvidenceEntry {
                term: term.to_string(), kind: item.kind.clone(), proof_note, user_attested: true,
            }),
        }
    }
    bank.version = 1;
    let path = evidence_bank_path(root);
    let text = serde_json::to_string_pretty(&bank).map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    std::fs::write(path, format!("{text}\n")).map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(bank)
}

pub fn remove_evidence(root: &Path, term: &str) -> Result<EvidenceBank, TailoringError> {
    let mut bank = load_evidence_bank(root)?;
    bank.entries.retain(|entry| normalize(&entry.term) != normalize(term));
    let path = evidence_bank_path(root);
    let text = serde_json::to_string_pretty(&bank).map_err(|error| TailoringError::InvalidJson(error.to_string()))?;
    std::fs::write(path, format!("{text}\n")).map_err(|error| TailoringError::Io(error.to_string()))?;
    Ok(bank)
}

pub fn preflight_items(analysis: &JobAnalysis, base_resume: &serde_json::Value, bank: &EvidenceBank) -> Vec<PreflightItem> {
    let base_text = base_resume.to_string().to_lowercase();
    let mut candidates: Vec<(String, String, u8)> = analysis.core_keywords.iter()
        .map(|signal| (signal.term.clone(), group_kind(&signal.category), signal.importance)).collect();
    candidates.extend(analysis.required_skills.iter().map(|term| (term.clone(), "technology".to_string(), 5)));
    candidates.extend(analysis.preferred_skills.iter().map(|term| (term.clone(), "technology".to_string(), 3)));
    candidates.extend(analysis.tools_and_platforms.iter().map(|term| (term.clone(), "technology".to_string(), 4)));
    candidates.extend(analysis.domain_terms.iter().map(|term| (term.clone(), "method_domain".to_string(), 3)));
    candidates.extend(analysis.responsibility_phrases.iter().map(|term| (term.clone(), "responsibility".to_string(), 4)));

    let mut items = Vec::new();
    for (term, kind, importance) in candidates {
        if term.trim().is_empty() || items.iter().any(|item: &PreflightItem| normalize(&item.term) == normalize(&term)) { continue; }
        let bank_entry = bank.entries.iter().find(|entry| normalize(&entry.term) == normalize(&term) && entry.user_attested);
        let source = if base_text.contains(&term.to_lowercase()) { "base_resume" } else if bank_entry.is_some() { "evidence_bank" } else { "needs_approval" };
        let proof_note = bank_entry.and_then(|entry| entry.proof_note.clone());
        items.push(PreflightItem { term, kind, importance, source, eligible_for_bullets: proof_note.is_some(), proof_note });
    }
    items.sort_by(|left, right| right.importance.cmp(&left.importance).then_with(|| left.term.cmp(&right.term)));
    items
}

pub fn selected_for_prompt(selected: &[SelectedEvidence]) -> Vec<EvidenceEntry> {
    selected.iter().filter_map(|item| {
        let term = item.term.trim();
        (!term.is_empty()).then(|| EvidenceEntry {
            term: term.to_string(), kind: item.kind.clone(),
            proof_note: item.proof_note.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned),
            user_attested: true,
        })
    }).collect()
}

fn group_kind(category: &str) -> String {
    let category = category.to_lowercase();
    if category.contains("responsib") { "responsibility".to_string() }
    else if category.contains("domain") || category.contains("method") { "method_domain".to_string() }
    else { "technology".to_string() }
}

fn normalize(value: &str) -> String {
    value.to_lowercase().chars().filter(|ch| ch.is_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::KeywordSignal;

    #[test]
    fn classifies_base_bank_and_unapproved_terms() {
        let analysis = JobAnalysis { role_target: String::new(), seniority: String::new(), core_keywords: vec![
            KeywordSignal { term: "React".into(), category: "technology".into(), importance: 5, evidence: String::new() },
            KeywordSignal { term: "Kubernetes".into(), category: "technology".into(), importance: 4, evidence: String::new() },
            KeywordSignal { term: "Team leadership".into(), category: "responsibility".into(), importance: 3, evidence: String::new() },
        ], required_skills: vec![], preferred_skills: vec![], tools_and_platforms: vec![], domain_terms: vec![], responsibility_phrases: vec![], achievement_angles: vec![], ats_phrase_bank: vec![], must_not_claim_without_evidence: vec![], summary: String::new() };
        let bank = EvidenceBank { version: 1, entries: vec![EvidenceEntry { term: "Kubernetes".into(), kind: "technology".into(), proof_note: None, user_attested: true }] };
        let items = preflight_items(&analysis, &serde_json::json!({"skills":"React"}), &bank);
        assert_eq!(items[0].source, "base_resume");
        assert_eq!(items[1].source, "evidence_bank");
        assert_eq!(items[2].source, "needs_approval");
    }
}
