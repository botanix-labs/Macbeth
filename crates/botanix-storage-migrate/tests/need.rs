use botanix_storage_migrate::is_migration_needed;
use eyre::Result;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_is_migration_needed_both_paths_nonexistent() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(!result, "Migration should not be needed when both paths don't exist");

    Ok(())
}

#[test]
fn test_is_migration_needed_reth_empty_botanix_nonexistent() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create empty reth directory
    fs::create_dir(&reth_path)?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(!result, "Migration should not be needed when reth db is empty");

    Ok(())
}

#[test]
fn test_is_migration_needed_reth_has_content_botanix_nonexistent() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create reth directory with content
    fs::create_dir(&reth_path)?;
    fs::write(reth_path.join("some_file.db"), "content")?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(
        result,
        "Migration should be needed when reth db has content and botanix db doesn't exist"
    );

    Ok(())
}

#[test]
fn test_is_migration_needed_reth_has_content_botanix_empty() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create reth directory with content
    fs::create_dir(&reth_path)?;
    fs::write(reth_path.join("some_file.db"), "content")?;

    // Create empty botanix directory
    fs::create_dir(&botanix_path)?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(result, "Migration should be needed when reth db has content and botanix db is empty");

    Ok(())
}

#[test]
fn test_is_migration_needed_reth_has_content_botanix_has_content() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create reth directory with content
    fs::create_dir(&reth_path)?;
    fs::write(reth_path.join("reth_file.db"), "reth content")?;

    // Create botanix directory with content
    fs::create_dir(&botanix_path)?;
    fs::write(botanix_path.join("botanix_file.db"), "botanix content")?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(!result, "Migration should not be needed when both databases have content");

    Ok(())
}

#[test]
fn test_is_migration_needed_reth_empty_botanix_has_content() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create empty reth directory
    fs::create_dir(&reth_path)?;

    // Create botanix directory with content
    fs::create_dir(&botanix_path)?;
    fs::write(botanix_path.join("botanix_file.db"), "botanix content")?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(!result, "Migration should not be needed when reth db is empty");

    Ok(())
}

#[test]
fn test_is_migration_needed_both_paths_empty_directories() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let reth_path = temp_dir.path().join("reth_db");
    let botanix_path = temp_dir.path().join("botanix_db");

    // Create both directories but leave them empty
    fs::create_dir(&reth_path)?;
    fs::create_dir(&botanix_path)?;

    let result = is_migration_needed(&reth_path, &botanix_path)?;
    assert!(!result, "Migration should not be needed when both directories are empty");

    Ok(())
}
