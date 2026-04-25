use crate::standards::config::ProjectInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleFile {
    JavaCoding,
    Api,
    Exception,
    Environment,
    Database,
    Tenant,
    I18n,
    Redis,
    SpringCloud,
    AuthToms,
}

impl RuleFile {
    pub fn relative_path(&self) -> &'static str {
        match self {
            RuleFile::JavaCoding => "docs/standards/java-coding.md",
            RuleFile::Api => "docs/standards/api.md",
            RuleFile::Exception => "docs/standards/exception.md",
            RuleFile::Environment => "docs/standards/environment.md",
            RuleFile::Database => "docs/standards/database.md",
            RuleFile::Tenant => "docs/standards/tenant.md",
            RuleFile::I18n => "docs/standards/i18n.md",
            RuleFile::Redis => "docs/standards/redis.md",
            RuleFile::SpringCloud => "docs/standards/spring-cloud.md",
            RuleFile::AuthToms => "docs/standards/auth-toms.md",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            RuleFile::JavaCoding => "java-coding.md",
            RuleFile::Api => "api.md",
            RuleFile::Exception => "exception.md",
            RuleFile::Environment => "environment.md",
            RuleFile::Database => "database.md",
            RuleFile::Tenant => "tenant.md",
            RuleFile::I18n => "i18n.md",
            RuleFile::Redis => "redis.md",
            RuleFile::SpringCloud => "spring-cloud.md",
            RuleFile::AuthToms => "auth-toms.md",
        }
    }
}

#[derive(Debug)]
pub struct GateOutcome {
    pub file: RuleFile,
    pub applies: bool,
    pub reason: String,
}

const CANONICAL_ORDER: &[RuleFile] = &[
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
];

pub fn evaluate(info: &ProjectInfo) -> Vec<GateOutcome> {
    CANONICAL_ORDER
        .iter()
        .map(|&file| {
            let (applies, reason) = match file {
                RuleFile::JavaCoding
                | RuleFile::Api
                | RuleFile::Exception
                | RuleFile::Environment => (true, "always".to_string()),
                RuleFile::Database => (
                    info.database.enabled,
                    format!("database.enabled={}", info.database.enabled),
                ),
                RuleFile::Tenant => (
                    info.tenant.enabled,
                    format!("tenant.enabled={}", info.tenant.enabled),
                ),
                RuleFile::I18n => (
                    info.i18n.enabled,
                    format!("i18n.enabled={}", info.i18n.enabled),
                ),
                RuleFile::Redis => (
                    info.redis.enabled,
                    format!("redis.enabled={}", info.redis.enabled),
                ),
                RuleFile::SpringCloud => (
                    info.spring_cloud.enabled,
                    format!("spring-cloud.enabled={}", info.spring_cloud.enabled),
                ),
                RuleFile::AuthToms => (
                    info.auth.toms.enabled,
                    format!("auth.toms.enabled={}", info.auth.toms.enabled),
                ),
            };
            GateOutcome { file, applies, reason }
        })
        .collect()
}
