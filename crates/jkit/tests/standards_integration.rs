use jkit::standards::config::ProjectInfo;
use jkit::standards::gates::{evaluate, GateOutcome, RuleFile};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn project_info_full_yaml_parses() {
    let info = ProjectInfo::from_yaml_file(&fixture("project-info-full.yaml"))
        .expect("parse full yaml");
    assert_eq!(info.project.name, "my-app");
    assert!(info.database.enabled);
    assert!(info.tenant.enabled);
    assert!(!info.i18n.enabled);
    assert!(info.redis.enabled);
    assert!(!info.spring_cloud.enabled);
    assert!(info.auth.toms.enabled);
}

#[test]
fn evaluate_full_fixture_returns_expected_outcomes() {
    let info = ProjectInfo::from_yaml_file(&fixture("project-info-full.yaml")).unwrap();
    let outcomes = evaluate(&info);

    let by_file: std::collections::HashMap<RuleFile, &GateOutcome> =
        outcomes.iter().map(|o| (o.file, o)).collect();

    assert!(by_file[&RuleFile::JavaCoding].applies);
    assert!(by_file[&RuleFile::Api].applies);
    assert!(by_file[&RuleFile::Exception].applies);
    assert!(by_file[&RuleFile::Environment].applies);
    assert!(by_file[&RuleFile::Database].applies);
    assert!(by_file[&RuleFile::Tenant].applies);
    assert!(by_file[&RuleFile::Redis].applies);
    assert!(by_file[&RuleFile::AuthToms].applies);
    assert!(!by_file[&RuleFile::I18n].applies);
    assert!(!by_file[&RuleFile::SpringCloud].applies);
}

#[test]
fn evaluate_outcomes_are_in_canonical_order() {
    let info = ProjectInfo::from_yaml_file(&fixture("project-info-full.yaml")).unwrap();
    let outcomes = evaluate(&info);
    let order: Vec<RuleFile> = outcomes.iter().map(|o| o.file).collect();
    assert_eq!(
        order,
        vec![
            RuleFile::JavaCoding,
            RuleFile::Api,
            RuleFile::Exception,
            RuleFile::Environment,
            RuleFile::Database,
            RuleFile::Tenant,
            RuleFile::I18n,
            RuleFile::Redis,
            RuleFile::SpringCloud,
            RuleFile::AuthToms,
        ]
    );
}
