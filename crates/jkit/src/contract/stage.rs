use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tera::{Context as TeraContext, Tera};

use super::service_meta;
use super::stage_status;
use crate::pom::prereqs::{self as pom_prereqs, PomReport};
use crate::pom::ProfileArg;
use crate::util::{atomic_write, print_json};

const PLUGIN_TPL: &str = include_str!("../../templates/contract/plugin.json.tera");
const SKILL_TPL: &str = include_str!("../../templates/contract/skill.md.tera");
const DOMAIN_TPL: &str = include_str!("../../templates/contract/domain.md.tera");
const SMART_DOC_TPL: &str = include_str!("../../templates/contract/smart-doc.json.tera");

#[derive(Debug, Deserialize)]
struct InterviewAnswers {
    description: String,
    use_when: Vec<String>,
    invariants: Vec<String>,
    keywords: Vec<String>,
    #[serde(default)]
    not_responsible_for: Vec<String>,
    #[serde(default)]
    sdk: Option<SdkAnswer>,
    #[serde(default)]
    authentication: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct SdkAnswer {
    present: bool,
    artifact_id: String,
    version: String,
}

#[derive(Serialize)]
struct StageReport {
    service: String,
    stage_dir: String,
    files_written: Vec<String>,
    pom_status: PomReport,
    smart_doc_action: &'static str,
    mvn_smart_doc: &'static str,
    gitignore_updated: bool,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    actions_pending: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocking_errors: Vec<String>,
}

pub fn run(
    service: &str,
    interview_path: &Path,
    domains: &[String],
    dry_run: bool,
    force: bool,
    pom_path: &Path,
    src_root: &Path,
) -> Result<()> {
    if domains.is_empty() {
        anyhow::bail!("--domains must list at least one slug");
    }

    let meta = service_meta::compute(pom_path, src_root)?;
    if meta.service_name != service {
        anyhow::bail!(
            "--service '{}' does not match detected service '{}' (source: {})",
            service,
            meta.service_name,
            meta.service_name_source
        );
    }

    // Validate every domain slug appears in controllers[].domain_slug
    let known_slugs: HashSet<&str> = meta
        .controllers
        .iter()
        .map(|c| c.domain_slug.as_str())
        .collect();
    for d in domains {
        if !known_slugs.contains(d.as_str()) {
            anyhow::bail!(
                "domain slug '{}' not found in controllers (known: {:?})",
                d,
                known_slugs
            );
        }
    }

    let answers_text = fs::read_to_string(interview_path)
        .with_context(|| format!("reading {}", interview_path.display()))?;
    let answers: InterviewAnswers = serde_json::from_str(&answers_text)
        .with_context(|| format!("parsing {}", interview_path.display()))?;

    let stage_dir: PathBuf = PathBuf::from(".jkit/contract-stage").join(&meta.service_name);
    if stage_dir.exists() && !force && !dry_run {
        anyhow::bail!(
            "stage dir {} already exists; pass --force to regenerate",
            stage_dir.display()
        );
    }

    let mut files_written: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut actions_pending: Vec<String> = Vec::new();
    let mut blocking_errors: Vec<String> = Vec::new();

    // 4. Install smart-doc plugin
    let pom_status = pom_prereqs::compute(ProfileArg::SmartDoc, !dry_run, pom_path)?;

    // 5. smart-doc.json (always under cwd)
    let sd_path = Path::new("smart-doc.json");
    let smart_doc_action: &'static str;
    let stage_reference_dir = stage_dir.join("reference");
    let openapi_json = stage_reference_dir.join("openapi.json");
    let openapi_yaml = stage_reference_dir.join("contract.yaml");

    if dry_run {
        smart_doc_action = if sd_path.exists() {
            "would_merge_existing"
        } else {
            "would_create"
        };
        actions_pending.push("install smart-doc plugin (if missing)".into());
        actions_pending.push("write smart-doc.json".into());
        actions_pending.push("run mvn smart-doc:openapi".into());
        actions_pending.push("convert openapi.json -> contract.yaml".into());
        actions_pending.push("instantiate plugin.json, SKILL.md, domains/*.md".into());
        actions_pending.push("update .gitignore".into());

        files_written.push(format!(
            "{}/.claude-plugin/plugin.json",
            stage_dir.display()
        ));
        files_written.push(format!(
            "{}/skills/{}/SKILL.md",
            stage_dir.display(),
            service
        ));
        for d in domains {
            files_written.push(format!("{}/domains/{}.md", stage_dir.display(), d));
        }
        files_written.push(format!("{}", openapi_yaml.display()));

        let report = StageReport {
            service: service.to_string(),
            stage_dir: stage_dir.display().to_string(),
            files_written,
            pom_status,
            smart_doc_action,
            mvn_smart_doc: "skipped",
            gitignore_updated: false,
            warnings,
            actions_pending,
            blocking_errors,
        };
        return print_json(&report);
    }

    fs::create_dir_all(&stage_reference_dir)?;
    let smart_doc_action_owned = write_smart_doc_json(sd_path, &openapi_json)?;
    smart_doc_action = match smart_doc_action_owned.as_str() {
        "merged_existing" => "merged_existing",
        "created" => "created",
        _ => "created",
    };

    // 6. mvn smart-doc:openapi
    let mvn_status = match Command::new("mvn")
        .args(["-q", "smart-doc:openapi"])
        .output()
    {
        Ok(o) if o.status.success() => "ok",
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            let combined = format!("{}\n{}", stdout, stderr);
            let tail: Vec<&str> = combined.lines().collect();
            let start = tail.len().saturating_sub(20);
            blocking_errors.push(format!(
                "mvn smart-doc:openapi failed:\n{}",
                tail[start..].join("\n")
            ));
            "failed"
        }
        Err(e) => {
            blocking_errors.push(format!("mvn invocation failed: {}", e));
            "failed"
        }
    };
    if mvn_status != "ok" {
        let report = StageReport {
            service: service.to_string(),
            stage_dir: stage_dir.display().to_string(),
            files_written,
            pom_status,
            smart_doc_action,
            mvn_smart_doc: mvn_status,
            gitignore_updated: false,
            warnings,
            actions_pending,
            blocking_errors,
        };
        print_json(&report)?;
        anyhow::bail!("smart-doc:openapi failed");
    }

    // 7. JSON -> YAML
    if openapi_json.exists() {
        let json_text = fs::read_to_string(&openapi_json)?;
        let v: serde_json::Value = serde_json::from_str(&json_text)
            .context("parsing openapi.json")?;
        let yaml_text = serde_yaml::to_string(&v).context("converting openapi.json to YAML")?;
        atomic_write(&openapi_yaml, yaml_text.as_bytes())?;
        let _ = fs::remove_file(&openapi_json);
        files_written.push(openapi_yaml.display().to_string());
    } else {
        warnings.push(format!(
            "smart-doc did not produce {} — skipping YAML conversion",
            openapi_json.display()
        ));
    }

    // 8. Templates
    let mut tera = Tera::default();
    tera.add_raw_template("plugin.json", PLUGIN_TPL)?;
    tera.add_raw_template("skill.md", SKILL_TPL)?;
    tera.add_raw_template("domain.md", DOMAIN_TPL)?;

    let mut ctx = TeraContext::new();
    ctx.insert("service_name", &meta.service_name);
    ctx.insert("version", &meta.version);
    ctx.insert("description", &answers.description);
    ctx.insert("use_when", &answers.use_when);
    ctx.insert("invariants", &answers.invariants);
    ctx.insert("not_responsible_for", &answers.not_responsible_for);
    ctx.insert("keywords", &answers.keywords);
    ctx.insert("domains", &domains);
    ctx.insert(
        "authentication",
        answers.authentication.as_deref().unwrap_or("not specified"),
    );
    ctx.insert(
        "sdk",
        &answers
            .sdk
            .clone()
            .or_else(|| meta.sdk.as_ref().map(|s| SdkAnswer {
                present: s.present,
                artifact_id: s.artifact_id.clone(),
                version: s.version.clone(),
            })),
    );

    let plugin_path = stage_dir.join(".claude-plugin/plugin.json");
    fs::create_dir_all(plugin_path.parent().unwrap())?;
    let plugin_out = tera.render("plugin.json", &ctx)?;
    atomic_write(&plugin_path, plugin_out.as_bytes())?;
    files_written.push(plugin_path.display().to_string());

    let skill_path = stage_dir
        .join("skills")
        .join(&meta.service_name)
        .join("SKILL.md");
    fs::create_dir_all(skill_path.parent().unwrap())?;
    let skill_out = tera.render("skill.md", &ctx)?;
    atomic_write(&skill_path, skill_out.as_bytes())?;
    files_written.push(skill_path.display().to_string());

    let domains_dir = stage_dir.join("domains");
    fs::create_dir_all(&domains_dir)?;
    for d in domains {
        let controller = match meta.controllers.iter().find(|c| &c.domain_slug == d) {
            Some(c) => c,
            None => continue,
        };
        let mut dctx = TeraContext::new();
        dctx.insert("domain_slug", d);
        dctx.insert("controller_class", &controller.class);
        dctx.insert("controller_file", &controller.file);
        dctx.insert("methods", &controller.methods);
        let domain_out = tera.render("domain.md", &dctx)?;
        let dpath = domains_dir.join(format!("{}.md", d));
        atomic_write(&dpath, domain_out.as_bytes())?;
        files_written.push(dpath.display().to_string());
    }

    // 9. .gitignore
    let gitignore_updated = ensure_gitignore_entry(".jkit/contract-stage/")?;

    // 10. Write per-file SHA256 manifest so the next `stage-status` call
    // can detect hand-edits before the human approves another --force run.
    let manifest = stage_status::build_manifest(&files_written)?;
    let manifest_path = stage_dir.join(".manifest.json");
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    atomic_write(&manifest_path, &manifest_json)?;

    let report = StageReport {
        service: service.to_string(),
        stage_dir: stage_dir.display().to_string(),
        files_written,
        pom_status,
        smart_doc_action,
        mvn_smart_doc: "ok",
        gitignore_updated,
        warnings,
        actions_pending,
        blocking_errors,
    };
    print_json(&report)
}

/// Write or merge `smart-doc.json`. Returns "created" or "merged_existing".
fn write_smart_doc_json(path: &Path, openapi_json_path: &Path) -> Result<String> {
    let mut tera = Tera::default();
    tera.add_raw_template("smart-doc.json", SMART_DOC_TPL)?;
    let mut ctx = TeraContext::new();
    ctx.insert("out_path", &openapi_json_path.parent().unwrap().display().to_string());
    let rendered = tera.render("smart-doc.json", &ctx)?;
    let new_value: serde_json::Value = serde_json::from_str(&rendered)
        .context("rendered smart-doc.json template is invalid JSON")?;

    if !path.exists() {
        atomic_write(path, rendered.as_bytes())?;
        return Ok("created".to_string());
    }

    let existing_text = fs::read_to_string(path)?;
    let mut existing: serde_json::Value = serde_json::from_str(&existing_text)
        .context("existing smart-doc.json is malformed")?;
    if let (Some(eo), Some(no)) = (existing.as_object_mut(), new_value.as_object()) {
        // Overwrite only outPath, openApiAllInOne, sourceCodePaths
        for key in ["outPath", "openApiAllInOne", "sourceCodePaths"] {
            if let Some(v) = no.get(key) {
                eo.insert(key.to_string(), v.clone());
            }
        }
    }
    let merged = serde_json::to_string_pretty(&existing)?;
    atomic_write(path, merged.as_bytes())?;
    Ok("merged_existing".to_string())
}

fn ensure_gitignore_entry(entry: &str) -> Result<bool> {
    let path = Path::new(".gitignore");
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(false);
    }
    let mut new = existing;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(entry);
    new.push('\n');
    atomic_write(path, new.as_bytes())?;
    Ok(true)
}
