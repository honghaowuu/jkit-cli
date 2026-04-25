use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::util::print_json;

/// Top-level user-authored target schema yaml shape.
#[derive(Debug, Deserialize)]
struct TargetSchema(Vec<TargetTable>);

#[derive(Debug, Deserialize, Clone)]
struct TargetTable {
    table: String,
    #[serde(default)]
    columns: Vec<TargetColumn>,
    #[serde(default)]
    foreign_keys: Vec<TargetFk>,
    #[serde(default)]
    indexes: Vec<TargetIndex>,
}

#[derive(Debug, Deserialize, Clone)]
struct TargetColumn {
    name: String,
    #[serde(rename = "type")]
    col_type: String,
    #[serde(default)]
    nullable: Option<bool>,
    #[serde(default)]
    primary_key: Option<bool>,
    #[serde(default)]
    default: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct TargetFk {
    column: String,
    references: String,
}

#[derive(Debug, Deserialize, Clone)]
struct TargetIndex {
    name: String,
    columns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DiffReport {
    db_type: String,
    db_reachable: bool,
    target_path: String,
    changes: Vec<serde_json::Value>,
    no_op: Vec<serde_json::Value>,
    warnings: Vec<String>,
    blocking_errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    db_unreachable_reason: Option<String>,
}

#[derive(Debug, Default)]
struct LiveSchema {
    /// table -> column-name -> (data_type, is_nullable)
    tables: BTreeMap<String, BTreeMap<String, LiveColumn>>,
    /// table -> set of column names that participate in the PK.
    primary_keys: BTreeMap<String, Vec<String>>,
    /// (table, column) -> referenced "table(col)" string, lower-cased.
    foreign_keys: HashMap<(String, String), String>,
    /// table -> set of index names.
    indexes: HashMap<String, Vec<String>>,
    /// table -> approximate row count (used to gate the backfill warning).
    row_counts: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
struct LiveColumn {
    data_type: String,
    is_nullable: bool,
}

pub fn run(run_dir: &Path, no_db: bool, pom_path: &Path, target_override: Option<&Path>) -> Result<()> {
    let cs_path = run_dir.join("change-summary.md");
    if !cs_path.exists() {
        anyhow::bail!("change-summary.md missing in {}", run_dir.display());
    }
    let target_path = target_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| run_dir.join("target-schema.yaml"));
    if !target_path.exists() {
        anyhow::bail!(
            "sql-migration must produce target-schema.yaml before invoking diff (looked at {})",
            target_path.display()
        );
    }

    let pom_text = fs::read_to_string(pom_path)
        .with_context(|| format!("reading {}", pom_path.display()))?;
    let db_type = detect_db_type(&pom_text)
        .ok_or_else(|| anyhow::anyhow!("unsupported or undetected DB type"))?;

    let target_text = fs::read_to_string(&target_path)
        .with_context(|| format!("reading {}", target_path.display()))?;
    let target: TargetSchema = serde_yaml::from_str(&target_text)
        .with_context(|| format!("parsing {}", target_path.display()))?;

    let database_url = resolve_database_url();
    let mut db_reachable = true;
    let mut unreachable_reason: Option<String> = None;
    let live: LiveSchema = match (database_url.as_deref(), no_db) {
        (Some(url), _) => match introspect(&db_type, url) {
            Ok(s) => s,
            Err(e) if no_db => {
                db_reachable = false;
                unreachable_reason = Some(e.to_string());
                LiveSchema::default()
            }
            Err(e) => anyhow::bail!("DB introspection failed: {}", e),
        },
        (None, true) => {
            db_reachable = false;
            unreachable_reason = Some("DATABASE_URL not found".to_string());
            LiveSchema::default()
        }
        (None, false) => anyhow::bail!("DATABASE_URL not found (use --no-db to skip)"),
    };

    let (changes, no_op, warnings) = compute_diff(&target, &live, db_reachable);

    let report = DiffReport {
        db_type,
        db_reachable,
        target_path: target_path.to_string_lossy().into_owned(),
        changes,
        no_op,
        warnings,
        blocking_errors: vec![],
        db_unreachable_reason: unreachable_reason,
    };
    print_json(&report)
}

/// Detect the DB type from the JDBC driver dependency.
pub fn detect_db_type(pom: &str) -> Option<String> {
    if pom.contains("<groupId>org.postgresql</groupId>")
        && pom.contains("<artifactId>postgresql</artifactId>")
    {
        return Some("postgresql".into());
    }
    if pom.contains("<artifactId>mysql-connector-java</artifactId>")
        || pom.contains("<artifactId>mysql-connector-j</artifactId>")
    {
        return Some("mysql".into());
    }
    if pom.contains("<artifactId>mariadb-java-client</artifactId>") {
        return Some("mariadb".into());
    }
    None
}

/// Resolve DATABASE_URL: process env first, then .env/local.env, then .env/local/*.env.
pub fn resolve_database_url() -> Option<String> {
    if let Ok(v) = std::env::var("DATABASE_URL") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    let single = Path::new(".env/local.env");
    if single.exists() {
        if let Some(v) = read_env_var(single, "DATABASE_URL") {
            return Some(v);
        }
    }
    let dir = Path::new(".env/local");
    if dir.is_dir() {
        let mut entries: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return None,
        };
        entries.sort_by_key(|e| e.file_name());
        let mut last: Option<String> = None;
        for entry in entries {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("env") {
                if let Some(v) = read_env_var(&p, "DATABASE_URL") {
                    last = Some(v);
                }
            }
        }
        return last;
    }
    None
}

fn read_env_var(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some(t) => t,
            None => continue,
        };
        if k.trim() == key {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            return Some(v.to_string());
        }
    }
    None
}

/// Open a connection and read schema state for tables we care about.
fn introspect(db_type: &str, url: &str) -> Result<LiveSchema> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime")?;
    runtime.block_on(introspect_async(db_type, url))
}

async fn introspect_async(db_type: &str, url: &str) -> Result<LiveSchema> {
    use sqlx::Row;
    let mut live = LiveSchema::default();
    match db_type {
        "postgresql" => {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(url)
                .await
                .context("connecting to postgres")?;

            let cols = sqlx::query(
                "SELECT table_name, column_name, data_type, is_nullable \
                 FROM information_schema.columns \
                 WHERE table_schema = current_schema()",
            )
            .fetch_all(&pool)
            .await
            .context("querying columns")?;
            for row in cols {
                let table: String = row.try_get("table_name")?;
                let column: String = row.try_get("column_name")?;
                let dt: String = row.try_get("data_type")?;
                let nullable: String = row.try_get("is_nullable")?;
                live.tables.entry(table).or_default().insert(
                    column,
                    LiveColumn {
                        data_type: dt,
                        is_nullable: nullable.eq_ignore_ascii_case("YES"),
                    },
                );
            }

            let pks = sqlx::query(
                "SELECT tc.table_name, kcu.column_name \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                  AND tc.table_schema = kcu.table_schema \
                 WHERE tc.constraint_type = 'PRIMARY KEY' \
                   AND tc.table_schema = current_schema() \
                 ORDER BY kcu.ordinal_position",
            )
            .fetch_all(&pool)
            .await
            .context("querying primary keys")?;
            for row in pks {
                let table: String = row.try_get("table_name")?;
                let col: String = row.try_get("column_name")?;
                live.primary_keys.entry(table).or_default().push(col);
            }

            let fks = sqlx::query(
                "SELECT tc.table_name, kcu.column_name, ccu.table_name AS ref_table, ccu.column_name AS ref_col \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                 JOIN information_schema.constraint_column_usage ccu \
                   ON tc.constraint_name = ccu.constraint_name \
                 WHERE tc.constraint_type = 'FOREIGN KEY' \
                   AND tc.table_schema = current_schema()",
            )
            .fetch_all(&pool)
            .await
            .context("querying foreign keys")?;
            for row in fks {
                let table: String = row.try_get("table_name")?;
                let col: String = row.try_get("column_name")?;
                let ref_table: String = row.try_get("ref_table")?;
                let ref_col: String = row.try_get("ref_col")?;
                live.foreign_keys
                    .insert((table, col), format!("{}({})", ref_table, ref_col).to_lowercase());
            }

            let idxs = sqlx::query(
                "SELECT tablename, indexname FROM pg_indexes WHERE schemaname = current_schema()",
            )
            .fetch_all(&pool)
            .await
            .context("querying indexes")?;
            for row in idxs {
                let table: String = row.try_get("tablename")?;
                let name: String = row.try_get("indexname")?;
                live.indexes.entry(table).or_default().push(name);
            }

            for (table, _) in live.tables.clone().iter() {
                let q = format!("SELECT count(*)::bigint AS n FROM \"{}\"", table.replace('"', ""));
                if let Ok(row) = sqlx::query(&q).fetch_one(&pool).await {
                    if let Ok(n) = row.try_get::<i64, _>("n") {
                        live.row_counts.insert(table.clone(), n);
                    }
                }
            }
        }
        "mysql" | "mariadb" => {
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(2)
                .connect(url)
                .await
                .context("connecting to mysql")?;

            let cols = sqlx::query(
                "SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE, IS_NULLABLE \
                 FROM information_schema.columns \
                 WHERE TABLE_SCHEMA = DATABASE()",
            )
            .fetch_all(&pool)
            .await
            .context("querying columns")?;
            for row in cols {
                let table: String = row.try_get("TABLE_NAME")?;
                let column: String = row.try_get("COLUMN_NAME")?;
                let dt: String = row.try_get("DATA_TYPE")?;
                let nullable: String = row.try_get("IS_NULLABLE")?;
                live.tables.entry(table).or_default().insert(
                    column,
                    LiveColumn {
                        data_type: dt,
                        is_nullable: nullable.eq_ignore_ascii_case("YES"),
                    },
                );
            }

            let pks = sqlx::query(
                "SELECT TABLE_NAME, COLUMN_NAME FROM information_schema.key_column_usage \
                 WHERE TABLE_SCHEMA = DATABASE() AND CONSTRAINT_NAME = 'PRIMARY' \
                 ORDER BY ORDINAL_POSITION",
            )
            .fetch_all(&pool)
            .await
            .context("querying primary keys")?;
            for row in pks {
                let table: String = row.try_get("TABLE_NAME")?;
                let col: String = row.try_get("COLUMN_NAME")?;
                live.primary_keys.entry(table).or_default().push(col);
            }
        }
        other => anyhow::bail!("unsupported DB type: {}", other),
    }
    Ok(live)
}

/// Compare target schema vs live, return (changes, no_op, warnings).
fn compute_diff(
    target: &TargetSchema,
    live: &LiveSchema,
    db_reachable: bool,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, Vec<String>) {
    use serde_json::json;
    let mut changes: Vec<serde_json::Value> = Vec::new();
    let mut no_op: Vec<serde_json::Value> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for table in &target.0 {
        let live_table = live.tables.get(&table.table);
        if live_table.is_none() {
            // create_table
            let cols: Vec<serde_json::Value> = table
                .columns
                .iter()
                .map(|c| {
                    let mut o = serde_json::Map::new();
                    o.insert("name".into(), json!(c.name));
                    o.insert("type".into(), json!(c.col_type));
                    o.insert("nullable".into(), json!(c.nullable.unwrap_or(true)));
                    if let Some(d) = &c.default {
                        o.insert("default".into(), json!(d));
                    }
                    if c.primary_key.unwrap_or(false) {
                        o.insert("primary_key".into(), json!(true));
                    }
                    serde_json::Value::Object(o)
                })
                .collect();
            let pk: Vec<&str> = table
                .columns
                .iter()
                .filter(|c| c.primary_key.unwrap_or(false))
                .map(|c| c.name.as_str())
                .collect();
            let mut entry = serde_json::Map::new();
            entry.insert("kind".into(), json!("create_table"));
            entry.insert("name".into(), json!(table.table));
            entry.insert("columns".into(), json!(cols));
            entry.insert("primary_key".into(), json!(pk));
            changes.push(serde_json::Value::Object(entry));

            // FKs and indexes for newly-created tables go in too.
            for fk in &table.foreign_keys {
                changes.push(json!({
                    "kind": "add_foreign_key",
                    "table": table.table,
                    "column": fk.column,
                    "references": fk.references,
                }));
            }
            for idx in &table.indexes {
                changes.push(json!({
                    "kind": "add_index",
                    "table": table.table,
                    "name": idx.name,
                    "columns": idx.columns,
                }));
            }
            continue;
        }
        let live_table = live_table.unwrap();
        let nonempty = live.row_counts.get(&table.table).copied().unwrap_or(0) > 0;

        for col in &table.columns {
            match live_table.get(&col.name) {
                None => {
                    let nullable = col.nullable.unwrap_or(true);
                    let mut entry = serde_json::Map::new();
                    entry.insert("kind".into(), json!("add_column"));
                    entry.insert("table".into(), json!(table.table));
                    entry.insert("column".into(), json!(col.name));
                    entry.insert("type".into(), json!(col.col_type));
                    entry.insert("nullable".into(), json!(nullable));
                    if let Some(d) = &col.default {
                        entry.insert("default".into(), json!(d));
                    }
                    if let Some(fk) = table
                        .foreign_keys
                        .iter()
                        .find(|f| f.column == col.name)
                    {
                        entry.insert("foreign_key".into(), json!(fk.references));
                    }
                    changes.push(serde_json::Value::Object(entry));

                    if !nullable && col.default.is_none() && db_reachable && nonempty {
                        warnings.push(format!(
                            "ADD COLUMN {}.{} NOT NULL — backfill required (target table is non-empty)",
                            table.table, col.name
                        ));
                    }
                }
                Some(lc) => {
                    let target_nullable = col.nullable.unwrap_or(true);
                    let type_match = type_equivalent(&lc.data_type, &col.col_type);
                    let null_match = lc.is_nullable == target_nullable;
                    if type_match && null_match {
                        no_op.push(json!({
                            "kind": "column_present",
                            "table": table.table,
                            "column": col.name,
                        }));
                    } else {
                        changes.push(json!({
                            "kind": "alter_column",
                            "table": table.table,
                            "column": col.name,
                            "from": {"type": lc.data_type, "nullable": lc.is_nullable},
                            "to": {"type": col.col_type, "nullable": target_nullable},
                        }));
                        warnings.push(format!(
                            "ALTER COLUMN {}.{} ({} -> {}) — existing data may not satisfy constraint",
                            table.table, col.name, lc.data_type, col.col_type
                        ));
                    }
                }
            }
        }

        for fk in &table.foreign_keys {
            let key = (table.table.clone(), fk.column.clone());
            let want = fk.references.to_lowercase();
            if live.foreign_keys.get(&key).map(|s| s.as_str()) != Some(want.as_str()) {
                if live_table.contains_key(&fk.column) {
                    changes.push(serde_json::json!({
                        "kind": "add_foreign_key",
                        "table": table.table,
                        "column": fk.column,
                        "references": fk.references,
                    }));
                }
            }
        }

        for idx in &table.indexes {
            let exists = live
                .indexes
                .get(&table.table)
                .map(|v| v.iter().any(|n| n == &idx.name))
                .unwrap_or(false);
            if !exists {
                changes.push(serde_json::json!({
                    "kind": "add_index",
                    "table": table.table,
                    "name": idx.name,
                    "columns": idx.columns,
                }));
            }
        }
    }

    (changes, no_op, warnings)
}

/// Loose type comparison: case-insensitive, with simple aliasing for common
/// Postgres types so that `varchar(32)` matches `character varying`.
fn type_equivalent(live: &str, target: &str) -> bool {
    let l = live.to_ascii_lowercase();
    let t = target.to_ascii_lowercase();
    if l == t {
        return true;
    }
    let aliases = [
        ("character varying", "varchar"),
        ("timestamp with time zone", "timestamptz"),
        ("timestamp without time zone", "timestamp"),
        ("integer", "int"),
        ("integer", "int4"),
        ("bigint", "int8"),
        ("boolean", "bool"),
    ];
    let l_norm = l.split('(').next().unwrap_or(&l).trim();
    let t_norm = t.split('(').next().unwrap_or(&t).trim();
    if l_norm == t_norm {
        return true;
    }
    aliases
        .iter()
        .any(|(a, b)| (l_norm == *a && t_norm == *b) || (l_norm == *b && t_norm == *a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_postgres() {
        let pom = r#"<project>
            <dependencies>
                <dependency>
                    <groupId>org.postgresql</groupId>
                    <artifactId>postgresql</artifactId>
                </dependency>
            </dependencies>
        </project>"#;
        assert_eq!(detect_db_type(pom).as_deref(), Some("postgresql"));
    }

    #[test]
    fn detects_mysql() {
        let pom = r#"<project><dependencies><dependency><groupId>com.mysql</groupId><artifactId>mysql-connector-j</artifactId></dependency></dependencies></project>"#;
        assert_eq!(detect_db_type(pom).as_deref(), Some("mysql"));
    }

    #[test]
    fn type_equiv_aliases() {
        assert!(type_equivalent("character varying", "varchar(32)"));
        assert!(type_equivalent("timestamp with time zone", "timestamptz"));
        assert!(type_equivalent("integer", "int"));
        assert!(!type_equivalent("text", "varchar"));
    }

    #[test]
    fn unknown_db_type_returns_none() {
        let pom = r#"<project><dependencies/></project>"#;
        assert!(detect_db_type(pom).is_none());
    }
}
