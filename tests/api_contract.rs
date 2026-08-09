#[test]
fn migrations_define_idempotency_and_encrypted_token_storage() {
    let migration = include_str!("../migrations/0001_initial.sql");
    assert!(migration.contains("token_envelope text NOT NULL"));
    assert!(migration.contains("UNIQUE (user_id, idempotency_key)"));
    assert!(!migration.contains("access_token"));
    assert!(!migration.contains("refresh_token"));
}
