use recomphub_ingest::candidate_triage::{self, CandidateEvidenceSnapshot, CatalogGameName};
use recomphub_shared::{
    Playability, TriageAssessment, TriageCategory, TriageCitation, TriageConfidence, TriageSource,
    WorkEvidence,
};

fn snapshot() -> CandidateEvidenceSnapshot {
    CandidateEvidenceSnapshot {
        repo_url: "https://github.com/example/project".into(),
        github_repo_id: Some(1),
        name: Some("project".into()),
        default_branch: Some("main".into()),
        available: true,
        description: None,
        topics: vec![],
        readme: Some("Example Game has a renderer implementation.".into()),
        root_entries: Some(vec![]),
        recent_commits: Some(vec![]),
        release: None,
        archived: Some(true),
        pushed_at: None,
        stars: Some(0),
    }
}
fn assessment() -> TriageAssessment {
    TriageAssessment {
        category: TriageCategory::GameProject,
        confidence: TriageConfidence::Strong,
        work_evidence: WorkEvidence::Implementation,
        summary: "A source implementation exists for Example Game.".into(),
        game_titles: vec!["Example Game".into()],
        kind: None,
        playability: Playability::Unknown,
        targets: vec![],
        warnings: vec![],
        citations: vec![
            TriageCitation {
                claim: "game".into(),
                source: TriageSource::Readme,
                quote: "Example Game".into(),
                source_url: "https://evil.example".into(),
            },
            TriageCitation {
                claim: "work".into(),
                source: TriageSource::Readme,
                quote: "renderer implementation".into(),
                source_url: "https://evil.example".into(),
            },
        ],
    }
}
#[test]
fn archived_zero_star_project_with_cited_work_remains_eligible() {
    let result = candidate_triage::validate_assessment(assessment(), &snapshot()).unwrap();
    assert_eq!(result.category, TriageCategory::GameProject);
    assert_eq!(result.playability, Playability::InProgress);
    assert!(
        result
            .citations
            .iter()
            .all(|citation| citation.source_url == "https://github.com/example/project#readme")
    );
}
#[test]
fn forged_quote_is_rejected_and_alias_ambiguity_is_preserved() {
    let mut result = assessment();
    result.citations[1].quote = "invented work".into();
    assert!(candidate_triage::validate_assessment(result, &snapshot()).is_err());
    let games = candidate_triage::match_games(
        &["Alias".into()],
        &[
            CatalogGameName {
                id: 2,
                name: "A".into(),
                alt_names: vec!["Alias".into()],
            },
            CatalogGameName {
                id: 1,
                name: "B".into(),
                alt_names: vec!["Alias".into()],
            },
        ],
    );
    assert_eq!(
        games
            .into_iter()
            .map(|game| game.game_id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}
