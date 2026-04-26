use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::{tempdir, TempDir};

fn jkit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jkit"))
}

fn write(p: &Path, contents: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

/// Build a fixture project: two domains (invoice + payment), one cross-cutting
/// orphan entity (Audit), one entity in a sub-package of the invoice
/// controller (InvoiceLine), so we exercise package-prefix selection.
fn fixture_project() -> TempDir {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Minimal pom.xml — service_meta only needs to read it; smartdoc isn't
    // invoked here.
    write(
        &root.join("pom.xml"),
        r#"<?xml version="1.0"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>fixture</artifactId>
  <version>0.0.1</version>
</project>
"#,
    );

    // invoice domain — controller + two entities (one in same package, one in
    // sub-package to verify pkg-prefix matching).
    write(
        &root.join("src/main/java/com/example/invoice/InvoiceController.java"),
        r#"
package com.example.invoice;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/invoices")
public class InvoiceController {
    @GetMapping("/{id}")
    public Invoice getById(@PathVariable String id) { return null; }
}
"#,
    );
    write(
        &root.join("src/main/java/com/example/invoice/Invoice.java"),
        r#"
package com.example.invoice;
import javax.persistence.*;
import javax.validation.constraints.*;
import java.math.BigDecimal;
import java.util.List;

/**
 * Invoice issued to a customer for billable services.
 *
 * @author someone
 */
@Entity
public class Invoice {
    @Id
    @GeneratedValue
    private Long id;

    @NotNull
    @Column(precision = 19, scale = 2)
    private BigDecimal amount;

    @ManyToOne(fetch = FetchType.LAZY)
    private Customer customer;

    @OneToMany(mappedBy = "invoice")
    private List<InvoiceLine> lineItems;
}
"#,
    );
    write(
        &root.join("src/main/java/com/example/invoice/line/InvoiceLine.java"),
        r#"
package com.example.invoice.line;
import javax.persistence.*;

@Entity
public class InvoiceLine {
    @Id
    private Long id;
}
"#,
    );

    // payment domain — controller + one entity. Entity stays single in its
    // package; verifies the simple case.
    write(
        &root.join("src/main/java/com/example/payment/PaymentController.java"),
        r#"
package com.example.payment;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/payments")
public class PaymentController {
    @PostMapping
    public Payment create() { return null; }
}
"#,
    );
    write(
        &root.join("src/main/java/com/example/payment/Payment.java"),
        r#"
package com.example.payment;
import javax.persistence.*;

@Entity
public class Payment {
    @Id
    private Long id;
}
"#,
    );

    // Cross-cutting entity in a package no controller covers — should land
    // in `domain_model_excluded` for both slugs. The orphan warning fires
    // only when no slug claims it.
    write(
        &root.join("src/main/java/com/example/audit/Audit.java"),
        r#"
package com.example.audit;
import javax.persistence.*;

@Entity
public class Audit {
    @Id
    private Long id;
}
"#,
    );

    // Domain map: explicit two-slug split.
    write(
        &root.join(".jkit/migration-domain-map.json"),
        r#"{
  "schema_version": 1,
  "domains": [
    {"slug": "invoice", "controllers": ["com.example.invoice.InvoiceController"]},
    {"slug": "payment", "controllers": ["com.example.payment.PaymentController"]}
  ]
}
"#,
    );

    tmp
}

fn run_scaffold(root: &Path, extra_args: &[&str]) -> serde_json::Value {
    let out = Command::new(jkit_bin())
        .arg("migrate")
        .arg("scaffold-docs")
        .arg("--domain-map")
        .arg(".jkit/migration-domain-map.json")
        .args(extra_args)
        .current_dir(root)
        .output()
        .expect("running jkit migrate scaffold-docs");
    assert!(
        out.status.success(),
        "scaffold-docs failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    serde_json::from_str(&stdout).expect("stdout is JSON")
}

fn domain<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["domains"]
        .as_array()
        .expect("domains array")
        .iter()
        .find(|d| d["name"] == name)
        .unwrap_or_else(|| panic!("no domain {name} in report"))
}

#[test]
fn placeholder_when_flag_off() {
    let tmp = fixture_project();
    let report = run_scaffold(tmp.path(), &[]);

    let inv = domain(&report, "invoice");
    assert_eq!(inv["domain_model_source"], "placeholder");

    let body = fs::read_to_string(tmp.path().join("docs/domains/invoice/domain-model.md"))
        .unwrap();
    assert!(body.contains("Auto-scaffolded by `jkit migrate scaffold-docs`"));
    // No entity table in the placeholder.
    assert!(!body.contains("| Field | Type | Constraints |"));
}

#[test]
fn drafts_entities_when_flag_on() {
    let tmp = fixture_project();
    let report = run_scaffold(tmp.path(), &["--draft-domain-model"]);

    let inv = domain(&report, "invoice");
    assert_eq!(inv["domain_model_source"], "tree-sitter");

    let body = fs::read_to_string(tmp.path().join("docs/domains/invoice/domain-model.md"))
        .unwrap();
    // Marker for re-run detection.
    assert!(
        body.contains("Auto-drafted by `jkit migrate scaffold-docs --draft-domain-model`"),
        "expected auto-draft marker; got:\n{body}"
    );
    // Class-level Javadoc captured.
    assert!(body.contains("Invoice issued to a customer for billable services."));
    // FQN heading.
    assert!(body.contains("### Invoice (`com.example.invoice.Invoice`)"));
    // Field table.
    assert!(body.contains("| `id` | `Long` | `@Id`, `@GeneratedValue` |"));
    assert!(body.contains("`@NotNull`"));
    assert!(body.contains("`@Column(precision = 19, scale = 2)`"));
    // Relationships in their own section, not the field table.
    assert!(body.contains("**Relationships:**"));
    assert!(body.contains("`@ManyToOne(fetch = FetchType.LAZY)`"));
    assert!(body.contains("`@OneToMany(mappedBy = \"invoice\")`"));
    // Sub-package entity included via package proximity.
    assert!(body.contains("InvoiceLine"));

    let pay = domain(&report, "payment");
    assert_eq!(pay["domain_model_source"], "tree-sitter");
    let pay_body = fs::read_to_string(tmp.path().join("docs/domains/payment/domain-model.md"))
        .unwrap();
    assert!(pay_body.contains("### Payment (`com.example.payment.Payment`)"));
    // No entity headings for other domains' entities.
    assert!(!pay_body.contains("### Invoice"));
    assert!(!pay_body.contains("### InvoiceLine"));
}

#[test]
fn excludes_orphan_and_warns() {
    let tmp = fixture_project();
    let report = run_scaffold(tmp.path(), &["--draft-domain-model"]);

    // Audit isn't in any controller's package and isn't mentioned in any
    // impl-logic — it's an orphan. Payment is claimed by the payment slug,
    // so it must NOT show up in invoice's excluded list (would be noise).
    let inv = domain(&report, "invoice");
    let excluded: Vec<&str> = inv["domain_model_excluded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        excluded.contains(&"Audit"),
        "expected Audit in excluded for invoice; got {excluded:?}"
    );
    assert!(
        !excluded.contains(&"Payment"),
        "Payment is claimed by payment slug, must not appear in invoice excluded; got {excluded:?}"
    );

    // Orphan warning surfaced once at the top level.
    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("Audit") && w.contains("not attributable")),
        "expected orphan warning for Audit; got {warnings:?}"
    );
}

#[test]
fn rerun_overwrites_auto_drafted_file() {
    let tmp = fixture_project();
    let _ = run_scaffold(tmp.path(), &["--draft-domain-model"]);
    let path = tmp.path().join("docs/domains/invoice/domain-model.md");
    let first = fs::read_to_string(&path).unwrap();

    // Add a new entity to a controller's package and re-run; the file should
    // pick it up.
    write(
        &tmp.path().join("src/main/java/com/example/invoice/Tax.java"),
        r#"
package com.example.invoice;
import javax.persistence.*;

@Entity
public class Tax {
    @Id
    private Long id;
}
"#,
    );

    let report = run_scaffold(tmp.path(), &["--draft-domain-model"]);
    let inv = domain(&report, "invoice");
    assert_eq!(inv["domain_model_source"], "tree-sitter");
    let second = fs::read_to_string(&path).unwrap();
    assert!(second.contains("### Tax"), "Tax should be in re-drafted file");
    assert_ne!(first, second, "re-run should overwrite the auto-drafted file");
}

#[test]
fn rerun_skips_human_edited_file() {
    let tmp = fixture_project();
    let _ = run_scaffold(tmp.path(), &["--draft-domain-model"]);
    let path = tmp.path().join("docs/domains/invoice/domain-model.md");

    // Hand-edit: strip the auto-drafted marker.
    let edited = "# invoice domain model\n\nHand-written notes — do not regenerate.\n";
    fs::write(&path, edited).unwrap();

    let report = run_scaffold(tmp.path(), &["--draft-domain-model"]);
    let inv = domain(&report, "invoice");
    assert_eq!(inv["domain_model_source"], "skipped");
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, edited, "human-edited file must not be touched");

    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("invoice") && w.contains("human edits")),
        "expected human-edits warning; got {warnings:?}"
    );
}
