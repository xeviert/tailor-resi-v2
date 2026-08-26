//! Deterministic ATS coverage scoring.
//!
//! The tailoring model also emits a self-assessed coverage number, but nothing constrains it:
//! the schema requires an integer in 0..=100 and the prompt never says how to derive one. That
//! made the headline metric — and the before/after delta on a re-tailor — a comparison of two
//! unrelated model guesses.
//!
//! This module measures the same thing against the document that was actually produced. It
//! takes the weighted term ledger the job analysis yields, checks each term against the resume
//! text, and reports what hit, what missed, and why.

use crate::analysis::JobAnalysis;
use crate::evidence::{analysis_candidates, equivalent_terms, token_set, PreflightItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AtsCoverage {
    /// 0..=100, weighted by how strongly the job post asked for each term.
    pub score: u8,
    /// Weight earned, counting partial matches proportionally.
    pub covered_weight: u32,
    pub total_weight: u32,
    /// Weight earned from text the tailoring layer is allowed to rewrite. This is the part of
    /// the score tailoring can actually move; the rest comes from facts that were always there.
    pub editable_covered_weight: u32,
    pub categories: Vec<CategoryCoverage>,
    pub terms: Vec<TermCoverage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CategoryCoverage {
    /// `required`, `core`, `tools`, `responsibilities`, `preferred`, or `domain`.
    pub group: String,
    /// Terms the document carries in full.
    pub covered: u32,
    /// Terms the document carries in part. Reported separately because a long requirement
    /// phrase is often mostly present, and a bare "0/8" would read as a total failure when
    /// the weighted bar beside it is half full.
    pub partial: u32,
    pub total: u32,
    pub covered_weight: u32,
    pub total_weight: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TermCoverage {
    pub term: String,
    pub kind: String,
    pub group: String,
    pub weight: u8,
    pub covered: bool,
    /// How much of the term the document carries, 0.0 to 1.0.
    ///
    /// The analysis does not only return single keywords; it also returns multi-word
    /// requirement phrases like "produire et maintenir des RFC / ADR". Scoring those all-or-
    /// nothing throws away real information — a resume naming ADR but not RFC has covered
    /// half of that requirement, and treating it as a total miss both understates the score
    /// and hides which half is missing.
    pub coverage_ratio: f32,
    /// The region carrying the most of this term, e.g. `experience.2.bullets.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_in: Option<String>,
    /// False when the only match is in locked text — a company name, a title, a date.
    pub in_editable_surface: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miss_reason: Option<MissReason>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissReason {
    /// Nothing in the base resume or the evidence bank supports this claim. Covering it needs
    /// the user to attest to it first.
    NoEvidence,
    /// The preflight resolved this term as already supported and it still did not reach the
    /// document. This is free, truthful coverage the tailoring pass left on the table.
    EvidenceNotPlaced,
}

/// One addressable piece of resume text.
struct Region {
    path: String,
    tokens: BTreeSet<String>,
    editable: bool,
}

/// Reads the never-tailored sections so their text counts toward coverage.
///
/// A real ATS parses the whole document, so education and the contact header are as visible to
/// it as a bullet is. These files carry a BOM, which `serde_json` will not accept.
pub fn load_locked_sections(
    root: &Path,
    language: &str,
) -> Result<Option<serde_json::Value>, String> {
    let path = root
        .join("resume")
        .join("content")
        .join(format!("locked.{language}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|error| error.to_string())
}

fn regions(content: &serde_json::Value, locked: Option<&serde_json::Value>) -> Vec<Region> {
    let mut regions = Vec::new();

    if let Some(text) = content["summary"].as_str() {
        regions.push(Region {
            path: "summary".to_string(),
            tokens: token_set(text),
            editable: true,
        });
    }

    for (job_index, job) in content["experience"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (bullet_index, bullet) in job["bullets"].as_array().into_iter().flatten().enumerate() {
            if let Some(text) = bullet.as_str() {
                regions.push(Region {
                    path: format!("experience.{job_index}.bullets.{bullet_index}"),
                    tokens: token_set(text),
                    editable: true,
                });
            }
        }
        for field in ["company", "location", "title", "dates"] {
            if let Some(text) = job[field].as_str() {
                regions.push(Region {
                    path: format!("experience.{job_index}.{field}"),
                    tokens: token_set(text),
                    editable: false,
                });
            }
        }
    }

    if let Some(skills) = content["skills"].as_object() {
        for (key, value) in skills {
            if let Some(text) = value.as_str() {
                regions.push(Region {
                    path: format!("skills.{key}"),
                    tokens: token_set(text),
                    editable: true,
                });
            }
        }
    }

    if let Some(locked) = locked.and_then(serde_json::Value::as_object) {
        for (key, value) in locked {
            for (index, line) in value.as_array().into_iter().flatten().enumerate() {
                if let Some(text) = line.as_str() {
                    regions.push(Region {
                        path: format!("{key}.{index}"),
                        tokens: token_set(text),
                        editable: false,
                    });
                }
            }
        }
    }

    regions
}

/// Every written form that should count as this term: the term itself plus the alternates the
/// analysis collected for it.
///
/// Without this, a resume saying "K8s" against a post saying "Kubernetes" scores as a miss even
/// though the two name the same thing.
fn matchable_forms<'a>(analysis: &'a JobAnalysis, term: &'a str) -> Vec<&'a str> {
    let mut forms = vec![term];
    for entry in &analysis.term_variants {
        if equivalent_terms(&entry.term, term) {
            forms.extend(
                entry
                    .variants
                    .iter()
                    .map(String::as_str)
                    .filter(|variant| !variant.trim().is_empty()),
            );
        }
    }
    forms
}

/// The share of a form's tokens present in `available`.
fn overlap_ratio(form_tokens: &BTreeSet<String>, available: &BTreeSet<String>) -> f32 {
    if form_tokens.is_empty() {
        return 0.0;
    }
    form_tokens.intersection(available).count() as f32 / form_tokens.len() as f32
}

/// How well a set of written forms is carried by a token pool, taking the best form.
fn best_ratio(forms: &[BTreeSet<String>], available: &BTreeSet<String>) -> f32 {
    forms
        .iter()
        .map(|form| overlap_ratio(form, available))
        .fold(0.0f32, f32::max)
}

/// `true` when the preflight already judged this term claimable without further attestation.
fn evidence_is_available(preflight: &[PreflightItem], term: &str) -> bool {
    preflight
        .iter()
        .any(|item| item.resolution == "auto_available" && equivalent_terms(&item.term, term))
}

pub fn score_ats_coverage(
    analysis: &JobAnalysis,
    content: &serde_json::Value,
    locked: Option<&serde_json::Value>,
    preflight: &[PreflightItem],
) -> AtsCoverage {
    let regions = regions(content, locked);
    let mut terms = Vec::new();
    let mut total_weight = 0u32;
    let mut covered_weight = 0u32;
    let mut editable_covered_weight = 0u32;

    // An applicant tracking system reads the document as one text, so a term is present if its
    // words are present anywhere in it. Requiring them all inside a single bullet is an
    // artefact of how the resume happens to be split up: "state management avancé" is genuinely
    // covered by a resume whose skills line says "state management" and whose tools line says
    // "debugging avancé", and scoring it as absent would misreport the document.
    let document_tokens = regions
        .iter()
        .flat_map(|region| region.tokens.iter().cloned())
        .collect::<BTreeSet<_>>();
    let editable_tokens = regions
        .iter()
        .filter(|region| region.editable)
        .flat_map(|region| region.tokens.iter().cloned())
        .collect::<BTreeSet<_>>();

    for candidate in analysis_candidates(analysis) {
        let weight = candidate.importance.max(1);
        total_weight += u32::from(weight);

        let forms = matchable_forms(analysis, &candidate.term)
            .into_iter()
            .map(token_set)
            .filter(|form| !form.is_empty())
            .collect::<Vec<_>>();

        let coverage_ratio = best_ratio(&forms, &document_tokens);
        let editable_ratio = best_ratio(&forms, &editable_tokens);
        let covered = coverage_ratio >= 1.0;

        covered_weight += weighted(weight, coverage_ratio);
        editable_covered_weight += weighted(weight, editable_ratio);

        // Report the single region carrying the most of the term, preferring an editable one:
        // when a term sits in both a bullet and a company name, the bullet is the tailoring work.
        let matched_in = (coverage_ratio > 0.0)
            .then(|| {
                regions
                    .iter()
                    .map(|region| (best_ratio(&forms, &region.tokens), region))
                    .filter(|(ratio, _)| *ratio > 0.0)
                    .max_by(|(left, left_region), (right, right_region)| {
                        left.total_cmp(right)
                            .then(left_region.editable.cmp(&right_region.editable))
                    })
                    .map(|(_, region)| region.path.clone())
            })
            .flatten();

        let miss_reason = (!covered).then(|| {
            if evidence_is_available(preflight, &candidate.term) {
                MissReason::EvidenceNotPlaced
            } else {
                MissReason::NoEvidence
            }
        });

        terms.push(TermCoverage {
            term: candidate.term,
            kind: candidate.kind,
            group: candidate.group.to_string(),
            weight,
            covered,
            coverage_ratio,
            matched_in,
            in_editable_surface: editable_ratio >= 1.0,
            miss_reason,
        });
    }

    // Misses first, heaviest first within that, so the report opens on whatever costs the most.
    terms.sort_by(|left, right| {
        left.covered
            .cmp(&right.covered)
            .then(right.weight.cmp(&left.weight))
            .then(left.coverage_ratio.total_cmp(&right.coverage_ratio))
            .then(left.term.cmp(&right.term))
    });

    AtsCoverage {
        score: percentage(covered_weight, total_weight),
        covered_weight,
        total_weight,
        editable_covered_weight,
        categories: categories(&terms),
        terms,
    }
}

/// Weight earned at a given coverage ratio, rounded to the nearest whole unit.
///
/// Working in whole units keeps the totals reportable as integers; the rounding error across a
/// ledger is well under a point of score.
fn weighted(weight: u8, ratio: f32) -> u32 {
    (f32::from(weight) * ratio).round() as u32
}

/// An empty ledger scores 0 rather than dividing by zero. There is nothing to cover, but
/// reporting a perfect score for an analysis that produced no terms would be worse.
fn percentage(covered: u32, total: u32) -> u8 {
    if total == 0 {
        return 0;
    }
    let score = (u64::from(covered) * 100 + u64::from(total) / 2) / u64::from(total);
    score.min(100) as u8
}

fn categories(terms: &[TermCoverage]) -> Vec<CategoryCoverage> {
    const ORDER: &[&str] = &[
        "required",
        "core",
        "tools",
        "responsibilities",
        "preferred",
        "domain",
    ];

    ORDER
        .iter()
        .filter_map(|group| {
            let members = terms
                .iter()
                .filter(|term| term.group == *group)
                .collect::<Vec<_>>();
            if members.is_empty() {
                return None;
            }
            Some(CategoryCoverage {
                group: (*group).to_string(),
                covered: members.iter().filter(|term| term.covered).count() as u32,
                partial: members
                    .iter()
                    .filter(|term| !term.covered && term.coverage_ratio > 0.0)
                    .count() as u32,
                total: members.len() as u32,
                covered_weight: members
                    .iter()
                    .map(|term| weighted(term.weight, term.coverage_ratio))
                    .sum(),
                total_weight: members.iter().map(|term| u32::from(term.weight)).sum(),
            })
        })
        .collect()
}

/// The terms the produced resume covers, and the ones it does not, as flat lists.
///
/// These replace the model's self-reported `covered_keywords` / `omitted_unsupported_keywords`,
/// which were never checked against the text the model actually wrote.
pub fn covered_and_omitted(coverage: &AtsCoverage) -> (Vec<String>, Vec<String>) {
    let covered = coverage
        .terms
        .iter()
        .filter(|term| term.covered)
        .map(|term| term.term.clone())
        .collect();
    let omitted = coverage
        .terms
        .iter()
        .filter(|term| !term.covered)
        .map(|term| term.term.clone())
        .collect();
    (covered, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::KeywordSignal;
    use crate::evidence::PreflightItem;

    fn analysis() -> JobAnalysis {
        JobAnalysis {
            role_target: "Lead Front Engineer".into(),
            seniority: "Senior".into(),
            core_keywords: vec![],
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

    fn resume(bullets: &[&str], skills: &str) -> serde_json::Value {
        serde_json::json!({
            "meta": { "language": "en", "type": "base", "template": "t.docx" },
            "experience": [{
                "company": "Kubernetes Consulting GmbH",
                "location": "Remote",
                "title": "Lead Front Engineer",
                "dates": "2024 - Present",
                "bullets": bullets
            }],
            "skills": { "frontend": skills }
        })
    }

    fn available(term: &str) -> PreflightItem {
        PreflightItem {
            term: term.into(),
            kind: "technology".into(),
            importance: 5,
            source: "evidence_bank",
            resolution: "auto_available",
            resolution_reason: "Previously confirmed in saved evidence",
            matched_term: Some(term.into()),
            proof_note: None,
            eligible_for_bullets: true,
            allow_model_role_placement: false,
        }
    }

    #[test]
    fn a_resume_naming_every_required_term_scores_full_marks() {
        let mut job = analysis();
        job.required_skills = vec!["React".into(), "TypeScript".into()];
        job.tools_and_platforms = vec!["Docker".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces in TypeScript."], "Docker"),
            None,
            &[],
        );

        assert_eq!(coverage.score, 100);
        assert_eq!(coverage.covered_weight, coverage.total_weight);
        assert!(coverage.terms.iter().all(|term| term.covered));
    }

    #[test]
    fn a_resume_naming_none_of_them_scores_zero() {
        let mut job = analysis();
        job.required_skills = vec!["Kotlin".into(), "Swift".into()];

        let coverage =
            score_ats_coverage(&job, &resume(&["Wrote documentation."], "Figma"), None, &[]);

        assert_eq!(coverage.score, 0);
        assert_eq!(coverage.covered_weight, 0);
        assert!(coverage.terms.iter().all(|term| !term.covered));
    }

    #[test]
    fn an_analysis_with_no_terms_scores_zero_rather_than_dividing_by_zero() {
        let coverage =
            score_ats_coverage(&analysis(), &resume(&["Anything."], "Anything"), None, &[]);

        assert_eq!(coverage.score, 0);
        assert_eq!(coverage.total_weight, 0);
        assert!(coverage.categories.is_empty());
    }

    #[test]
    fn a_missed_required_skill_costs_more_than_a_missed_domain_term() {
        let mut job = analysis();
        job.required_skills = vec!["Kotlin".into()];
        job.domain_terms = vec!["Fintech".into()];

        let required_missed = score_ats_coverage(
            &job,
            &resume(&["Worked across fintech products."], "Figma"),
            None,
            &[],
        );
        let domain_missed = score_ats_coverage(
            &job,
            &resume(&["Shipped Kotlin services."], "Figma"),
            None,
            &[],
        );

        // required = weight 5, domain = weight 3, so losing the required term must hurt more.
        assert!(
            required_missed.score < domain_missed.score,
            "missing required scored {}, missing domain scored {}",
            required_missed.score,
            domain_missed.score
        );
    }

    #[test]
    fn importance_from_the_analysis_drives_core_keyword_weight() {
        let mut job = analysis();
        job.core_keywords = vec![
            KeywordSignal {
                term: "Rust".into(),
                category: "technology".into(),
                importance: 5,
                evidence: "Named in the title".into(),
            },
            KeywordSignal {
                term: "Grafana".into(),
                category: "technology".into(),
                importance: 1,
                evidence: "Mentioned once".into(),
            },
        ];

        let kept_the_important_one = score_ats_coverage(
            &job,
            &resume(&["Shipped Rust services."], "Figma"),
            None,
            &[],
        );
        let kept_the_trivial_one = score_ats_coverage(
            &job,
            &resume(&["Maintained Grafana boards."], "Figma"),
            None,
            &[],
        );

        assert!(kept_the_important_one.score > kept_the_trivial_one.score);
    }

    #[test]
    fn punctuated_technology_names_survive_matching() {
        let mut job = analysis();
        job.required_skills = vec!["C++".into(), "Node.js".into(), "C#".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Wrote C++ and C# services."], "Node.js, Express"),
            None,
            &[],
        );

        assert_eq!(coverage.score, 100, "{:?}", coverage.terms);
    }

    #[test]
    fn a_miss_with_saved_evidence_is_reported_as_unplaced_not_unsupported() {
        let mut job = analysis();
        job.required_skills = vec!["GraphQL".into(), "Kotlin".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Wrote documentation."], "Figma"),
            None,
            &[available("GraphQL")],
        );

        let graphql = coverage
            .terms
            .iter()
            .find(|term| term.term == "GraphQL")
            .unwrap();
        let kotlin = coverage
            .terms
            .iter()
            .find(|term| term.term == "Kotlin")
            .unwrap();

        // GraphQL is claimable and still absent: free coverage the tailoring pass left behind.
        assert_eq!(graphql.miss_reason, Some(MissReason::EvidenceNotPlaced));
        assert_eq!(kotlin.miss_reason, Some(MissReason::NoEvidence));
    }

    #[test]
    fn a_covered_term_carries_no_miss_reason() {
        let mut job = analysis();
        job.required_skills = vec!["React".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces."], "Figma"),
            None,
            &[],
        );

        assert_eq!(coverage.terms[0].miss_reason, None);
        assert_eq!(
            coverage.terms[0].matched_in.as_deref(),
            Some("experience.0.bullets.0")
        );
    }

    #[test]
    fn a_term_found_only_in_a_company_name_counts_but_is_not_credited_to_tailoring() {
        let mut job = analysis();
        job.required_skills = vec!["Kubernetes".into()];

        let coverage =
            score_ats_coverage(&job, &resume(&["Wrote documentation."], "Figma"), None, &[]);

        let term = &coverage.terms[0];
        // A real ATS reads the employer name, so this is genuinely covered...
        assert!(term.covered);
        assert_eq!(term.matched_in.as_deref(), Some("experience.0.company"));
        // ...but tailoring did not put it there, so it must not read as tailoring work.
        assert!(!term.in_editable_surface);
        assert_eq!(coverage.editable_covered_weight, 0);
    }

    /// The summary is tailorable, so a term it carries is tailoring work, not locked text.
    #[test]
    fn a_term_found_only_in_the_summary_is_credited_to_tailoring() {
        let mut job = analysis();
        job.required_skills = vec!["GraphQL".into()];
        let mut content = resume(&["Wrote documentation."], "Figma");
        content["summary"] = serde_json::json!("Engineer building GraphQL platforms.");

        let coverage = score_ats_coverage(&job, &content, None, &[]);

        let term = &coverage.terms[0];
        assert!(term.covered);
        assert_eq!(term.matched_in.as_deref(), Some("summary"));
        assert!(term.in_editable_surface);
        assert!(coverage.editable_covered_weight > 0);
    }

    #[test]
    fn a_bullet_match_wins_over_an_incidental_locked_match() {
        let mut job = analysis();
        job.required_skills = vec!["Kubernetes".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Ran Kubernetes clusters in production."], "Figma"),
            None,
            &[],
        );

        assert_eq!(
            coverage.terms[0].matched_in.as_deref(),
            Some("experience.0.bullets.0")
        );
        assert!(coverage.terms[0].in_editable_surface);
    }

    #[test]
    fn locked_sections_contribute_to_coverage() {
        let mut job = analysis();
        job.required_skills = vec!["Business Administration".into()];
        let locked = serde_json::json!({
            "education": ["Bachelor of Business Administration - BBA"],
            "header": ["Xevier Turrubiartes"]
        });

        let without =
            score_ats_coverage(&job, &resume(&["Wrote documentation."], "Figma"), None, &[]);
        let with_locked = score_ats_coverage(
            &job,
            &resume(&["Wrote documentation."], "Figma"),
            Some(&locked),
            &[],
        );

        assert_eq!(without.score, 0);
        assert_eq!(with_locked.score, 100);
        assert_eq!(
            with_locked.terms[0].matched_in.as_deref(),
            Some("education.0")
        );
    }

    #[test]
    fn categories_report_each_group_separately() {
        let mut job = analysis();
        job.required_skills = vec!["React".into(), "Kotlin".into()];
        job.preferred_skills = vec!["Svelte".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces."], "Figma"),
            None,
            &[],
        );

        let required = coverage
            .categories
            .iter()
            .find(|category| category.group == "required")
            .unwrap();
        assert_eq!((required.covered, required.total), (1, 2));

        let preferred = coverage
            .categories
            .iter()
            .find(|category| category.group == "preferred")
            .unwrap();
        assert_eq!((preferred.covered, preferred.total), (0, 1));

        // Groups with no terms are left out rather than shown as an empty 0/0 bar.
        assert!(coverage
            .categories
            .iter()
            .all(|category| category.group != "domain"));
    }

    #[test]
    fn misses_are_listed_first_and_heaviest_first() {
        let mut job = analysis();
        job.required_skills = vec!["Kotlin".into()];
        job.domain_terms = vec!["Fintech".into()];
        job.tools_and_platforms = vec!["React".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces."], "Figma"),
            None,
            &[],
        );

        let order = coverage
            .terms
            .iter()
            .map(|term| term.term.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["Kotlin", "Fintech", "React"]);
    }

    #[test]
    fn covered_and_omitted_split_the_ledger_by_what_the_document_says() {
        let mut job = analysis();
        job.required_skills = vec!["React".into(), "Kotlin".into()];

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces."], "Figma"),
            None,
            &[],
        );
        let (covered, omitted) = covered_and_omitted(&coverage);

        assert_eq!(covered, vec!["React".to_string()]);
        assert_eq!(omitted, vec!["Kotlin".to_string()]);
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;
    use crate::analysis::TermVariants;

    fn job_wanting(term: &str, variants: &[&str]) -> JobAnalysis {
        JobAnalysis {
            role_target: "Platform Engineer".into(),
            seniority: "Senior".into(),
            core_keywords: vec![],
            required_skills: vec![term.into()],
            preferred_skills: vec![],
            tools_and_platforms: vec![],
            domain_terms: vec![],
            responsibility_phrases: vec![],
            achievement_angles: vec![],
            ats_phrase_bank: vec![],
            must_not_claim_without_evidence: vec![],
            term_variants: vec![TermVariants {
                term: term.into(),
                variants: variants.iter().map(|variant| (*variant).into()).collect(),
            }],
            summary: String::new(),
        }
    }

    fn resume_saying(bullet: &str) -> serde_json::Value {
        serde_json::json!({
            "meta": { "language": "en", "type": "base", "template": "t.docx" },
            "experience": [{
                "company": "Acme",
                "location": "Remote",
                "title": "Engineer",
                "dates": "2024 - Present",
                "bullets": [bullet]
            }],
            "skills": { "frontend": "Figma" }
        })
    }

    #[test]
    fn an_acronym_in_the_resume_covers_the_expansion_the_post_used() {
        let job = job_wanting("Kubernetes", &["K8s"]);

        let coverage = score_ats_coverage(&job, &resume_saying("Ran K8s clusters."), None, &[]);

        assert_eq!(coverage.score, 100, "{:?}", coverage.terms);
        assert!(coverage.terms[0].in_editable_surface);
    }

    #[test]
    fn the_expansion_in_the_resume_covers_the_acronym_the_post_used() {
        let job = job_wanting("CI/CD", &["continuous integration"]);

        let coverage = score_ats_coverage(
            &job,
            &resume_saying("Owned continuous integration for the platform."),
            None,
            &[],
        );

        assert_eq!(coverage.score, 100, "{:?}", coverage.terms);
    }

    #[test]
    fn an_unrelated_term_is_still_a_miss() {
        let job = job_wanting("Kubernetes", &["K8s"]);

        let coverage = score_ats_coverage(&job, &resume_saying("Ran Nomad clusters."), None, &[]);

        assert_eq!(coverage.score, 0, "{:?}", coverage.terms);
        assert_eq!(coverage.terms[0].miss_reason, Some(MissReason::NoEvidence));
    }

    #[test]
    fn variants_for_a_different_term_do_not_leak_across() {
        let mut job = job_wanting("Kubernetes", &["K8s"]);
        job.required_skills.push("Terraform".into());

        let coverage = score_ats_coverage(&job, &resume_saying("Ran K8s clusters."), None, &[]);

        let terraform = coverage
            .terms
            .iter()
            .find(|term| term.term == "Terraform")
            .unwrap();
        assert!(!terraform.covered);
    }

    #[test]
    fn a_blank_variant_is_ignored_rather_than_matching_everything() {
        let job = job_wanting("Kubernetes", &["", "   "]);

        let coverage = score_ats_coverage(&job, &resume_saying("Wrote documentation."), None, &[]);

        assert_eq!(coverage.score, 0, "{:?}", coverage.terms);
    }
}

#[cfg(test)]
mod partial_tests {
    use super::*;

    fn job_wanting(terms: &[&str]) -> JobAnalysis {
        JobAnalysis {
            role_target: "Lead Front Engineer".into(),
            seniority: "Senior".into(),
            core_keywords: vec![],
            required_skills: terms.iter().map(|term| (*term).into()).collect(),
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

    fn resume(bullets: &[&str], skills: &[(&str, &str)]) -> serde_json::Value {
        serde_json::json!({
            "meta": { "language": "en", "type": "base", "template": "t.docx" },
            "experience": [{
                "company": "Acme",
                "location": "Remote",
                "title": "Engineer",
                "dates": "2024 - Present",
                "bullets": bullets
            }],
            "skills": skills
                .iter()
                .map(|(key, value)| ((*key).to_string(), serde_json::json!(value)))
                .collect::<serde_json::Map<_, _>>()
        })
    }

    #[test]
    fn a_term_split_across_two_sections_still_counts_as_covered() {
        // "state management avancé" is genuinely in this resume; it just happens to straddle
        // two skills lines. An ATS reads the document as one text, so this must not be a miss.
        let job = job_wanting(&["state management avancé"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(
                &["Shipped features."],
                &[
                    ("frontend", "React, state management"),
                    ("tools", "debugging avancé"),
                ],
            ),
            None,
            &[],
        );

        assert!(coverage.terms[0].covered, "{:?}", coverage.terms);
        assert_eq!(coverage.score, 100);
    }

    #[test]
    fn a_partly_present_phrase_earns_part_of_its_weight() {
        let job = job_wanting(&["RFC and ADR"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Wrote features."], &[("tools", "Git, ADR")]),
            None,
            &[],
        );

        let term = &coverage.terms[0];
        assert!(!term.covered, "half a phrase is not a full match");
        assert!(
            (term.coverage_ratio - 0.5).abs() < f32::EPSILON,
            "{}",
            term.coverage_ratio
        );
        // Half the tokens present must beat zero and fall short of full.
        assert!(
            coverage.score > 0 && coverage.score < 100,
            "{}",
            coverage.score
        );
    }

    #[test]
    fn a_wholly_absent_phrase_earns_nothing() {
        let job = job_wanting(&["Kotlin and Swift"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Wrote features."], &[("tools", "Git")]),
            None,
            &[],
        );

        assert_eq!(coverage.terms[0].coverage_ratio, 0.0);
        assert_eq!(coverage.score, 0);
        assert!(coverage.terms[0].matched_in.is_none());
    }

    #[test]
    fn a_partial_match_points_at_the_section_carrying_the_most_of_it() {
        let job = job_wanting(&["automated visual regression testing"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(
                &["Handled regression triage."],
                &[("testing", "Playwright, automated visual testing")],
            ),
            None,
            &[],
        );

        assert_eq!(
            coverage.terms[0].matched_in.as_deref(),
            Some("skills.testing")
        );
    }

    #[test]
    fn categories_report_partial_matches_beside_full_ones() {
        let job = job_wanting(&["React", "RFC and ADR", "Kotlin"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Built React interfaces."], &[("tools", "Git, ADR")]),
            None,
            &[],
        );

        let required = &coverage.categories[0];
        assert_eq!(required.group, "required");
        assert_eq!(
            (required.covered, required.partial, required.total),
            (1, 1, 3)
        );
    }

    #[test]
    fn partial_credit_from_locked_text_is_not_credited_to_tailoring() {
        let job = job_wanting(&["Acme platform engineering"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(&["Wrote documentation."], &[("tools", "Git")]),
            None,
            &[],
        );

        // "Acme" comes from the employer name, so it earns coverage but no editable credit.
        assert!(coverage.terms[0].coverage_ratio > 0.0);
        assert_eq!(coverage.editable_covered_weight, 0);
        assert!(!coverage.terms[0].in_editable_surface);
    }

    #[test]
    fn french_inclusive_writing_matches_the_plain_word() {
        // French posts write "référent·e"; the resume writes "référent". Splitting on the middle
        // dot used to leave a stray "e" token and turn a real match into a miss.
        let job = job_wanting(&["Référent·e technique"]);

        let coverage = score_ats_coverage(
            &job,
            &resume(
                &["Référent technique d’une équipe de trois ingénieurs."],
                &[("tools", "Git")],
            ),
            None,
            &[],
        );

        assert!(coverage.terms[0].covered, "{:?}", coverage.terms);
        assert_eq!(coverage.score, 100);
    }
}
