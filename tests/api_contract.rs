#[test]
fn migrations_define_idempotency_and_encrypted_token_storage() {
    let migration = include_str!("../migrations/0001_initial.sql");
    assert!(migration.contains("token_envelope text NOT NULL"));
    assert!(migration.contains("UNIQUE (user_id, idempotency_key)"));
    assert!(!migration.contains("access_token"));
    assert!(!migration.contains("refresh_token"));
}

#[test]
fn merged_router_preserves_scoped_event_listing_and_updates() {
    let main = include_str!("../src/main.rs");
    let store = include_str!("../src/store.rs");
    let events = include_str!("../src/events.rs");

    assert!(main.contains("get(events::list).post(events::create)"));
    assert!(main.contains(".route(\"/v1/ws\", get(events::websocket))"));
    assert!(main.contains("AllowOrigin::exact(web_origin)"));
    assert!(!main.contains("allow_origin(Any)"));
    assert!(store.contains("WHERE owner_id = $1 ORDER BY created_at DESC LIMIT 100"));
    assert!(events.contains("event.owner_id == user_id"));
}
