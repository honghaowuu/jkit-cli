use jkit::standards::config::ProjectInfo;
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
