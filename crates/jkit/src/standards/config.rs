use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct ProjectInfo {
    pub project: Project,
    #[serde(default)]
    pub stack: Stack,
    #[serde(default)]
    pub database: Database,
    #[serde(default)]
    pub tenant: Toggle,
    #[serde(default)]
    pub i18n: I18n,
    #[serde(default)]
    pub redis: Toggle,
    #[serde(default, rename = "spring-cloud")]
    pub spring_cloud: SpringCloud,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub maven: Maven,
}

#[derive(Debug, Deserialize)]
pub struct Project {
    pub name: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default, rename = "server-port")]
    pub server_port: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Stack {
    #[serde(default)]
    pub java: Option<u32>,
    #[serde(default, rename = "spring-boot")]
    pub spring_boot: Option<String>,
    #[serde(default, rename = "mybatis-plus")]
    pub mybatis_plus: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Database {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "type")]
    pub db_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Toggle {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct I18n {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub languages: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SpringCloud {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub toms: TomsAuth,
}

#[derive(Debug, Default, Deserialize)]
pub struct TomsAuth {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "api-version")]
    pub api_version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Maven {
    #[serde(default)]
    pub repositories: Vec<MavenRepo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MavenRepo {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub snapshots: Option<bool>,
}

impl ProjectInfo {
    pub fn from_yaml_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let info: ProjectInfo = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {} as project-info.yaml", path.display()))?;
        Ok(info)
    }
}
