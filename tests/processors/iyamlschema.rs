use std::fs;
use tempfile::TempDir;
use redb::{Database, TableDefinition};
use crate::common::run_rsconstruct_with_env;

const SCHEMA_URL: &str = "https://example.com/test_schema.json";

/// The schema content for tests. propertyOrdering is ["name", "age"] which
/// matches the YAML key order in test data (serde_json with preserve_order
/// feature preserves insertion order from YAML parsing).
const SCHEMA: &str = r#"{
    "$schema": "http://json-schema.org/draft-07/schema#",
    "type": "object",
    "properties": {
        "name": { "type": "string" },
        "age": { "type": "integer" }
    },
    "propertyOrdering": ["name", "age"],
    "required": ["name"]
}"#;

const WEBCACHE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("webcache_v2");

/// Pre-populate the webcache redb database so the processor can find the
/// schema without network access.
///
/// Entries carry a fetch timestamp and expire after `[cache]
/// webcache_ttl_secs`, so the seed is stamped "now" — an entry written with
/// a zero timestamp would read as decades old and be re-fetched over the
/// network, which is exactly what these tests must never do.
fn populate_webcache(project_path: &std::path::Path, url: &str, content: &str) {
    populate_webcache_aged(project_path, url, content, 0);
}

/// Seed the webcache with an entry that was fetched `age_secs` ago.
fn populate_webcache_aged(
    project_path: &std::path::Path,
    url: &str,
    content: &str,
    age_secs: u64,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let stored = serde_json::json!({
        "fetched_at_secs": now.saturating_sub(age_secs),
        "body": content,
    });
    let bytes = serde_json::to_vec(&stored).unwrap();

    let db_path = project_path.join(".rsconstruct/webcache.redb");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let db = Database::create(&db_path).unwrap();
    let write_txn = db.begin_write().unwrap();
    {
        let mut table = write_txn.open_table(WEBCACHE_TABLE).unwrap();
        table.insert(url, bytes.as_slice()).unwrap();
    }
    write_txn.commit().unwrap();
}

#[test]
fn iyamlschema_valid_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    populate_webcache(project_path, SCHEMA_URL, SCHEMA);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: 30\n", SCHEMA_URL),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build", "-v"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "Build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn iyamlschema_invalid_data_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    populate_webcache(project_path, SCHEMA_URL, SCHEMA);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    // "age" should be integer, not string
    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: not_a_number\n", SCHEMA_URL),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build should fail for invalid data");
}

#[test]
fn iyamlschema_wrong_ordering_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Schema expects ["age", "name"] but YAML has name before age
    let schema_wrong_order = r#"{
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "propertyOrdering": ["age", "name"],
        "required": ["name"]
    }"#;
    let wrong_url = "https://example.com/wrong_order_schema.json";
    populate_webcache(project_path, wrong_url, schema_wrong_order);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    // YAML key order is ["name", "age"] but schema expects ["age", "name"]
    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: 30\n", wrong_url),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build should fail for wrong key order");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("property ordering"), "Error should mention property ordering: {}", stderr);
}

#[test]
fn iyamlschema_no_schema_field_fails() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    fs::write(
        project_path.join("data.yaml"),
        "name: Alice\nage: 30\n",
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(!output.status.success(), "Build should fail when $schema is missing");
}

#[test]
fn iyamlschema_incremental_skip() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    populate_webcache(project_path, SCHEMA_URL, SCHEMA);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: 30\n", SCHEMA_URL),
    ).unwrap();

    // First build
    let output1 = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output1.status.success(),
        "First build should succeed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output1.stdout),
        String::from_utf8_lossy(&output1.stderr),
    );

    // Second build should skip
    let output2 = run_rsconstruct_with_env(project_path, &["build", "--verbose"], &[("NO_COLOR", "1")]);
    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("[iyamlschema] Skipping (unchanged):"),
        "Second build should skip: {}", stdout2,
    );
}

/// A cached schema older than `[cache] webcache_ttl_secs` must be re-fetched,
/// not served forever.
///
/// The webcache used to have no expiry at all: a URL fetched once was served
/// from disk indefinitely, so a schema that changed upstream was never picked
/// up. The URL here is unroutable, so a re-fetch attempt is observable as a
/// fetch failure — which is the point: the entry was NOT served from cache.
#[test]
fn expired_webcache_entry_is_refetched() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Two days old, against a one-hour TTL.
    populate_webcache_aged(project_path, SCHEMA_URL, SCHEMA, 2 * 24 * 60 * 60);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[cache]\nwebcache_ttl_secs = 3600\n\n[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: 30\n", SCHEMA_URL),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !output.status.success() && combined.contains("Failed to fetch schema"),
        "an expired entry must be re-fetched, not served from cache: {combined}",
    );
}

/// The same entry, still within the TTL, is served from cache without
/// touching the network.
#[test]
fn fresh_webcache_entry_is_served_from_cache() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // One hour old, against a one-week TTL.
    populate_webcache_aged(project_path, SCHEMA_URL, SCHEMA, 3600);

    fs::write(
        project_path.join("rsconstruct.toml"),
        "[cache]\nwebcache_ttl_secs = 604800\n\n[processor.iyamlschema]\nsrc_dirs = [\".\"]\n",
    ).unwrap();

    fs::write(
        project_path.join("data.yaml"),
        format!("$schema: \"{}\"\nname: Alice\nage: 30\n", SCHEMA_URL),
    ).unwrap();

    let output = run_rsconstruct_with_env(project_path, &["build"], &[("NO_COLOR", "1")]);
    assert!(
        output.status.success(),
        "a fresh entry must be served from cache: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
