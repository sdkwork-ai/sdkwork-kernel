//! Live PostgreSQL contract tests for agent runtime session persistence.

use sdkwork_agent_database::{
    ActionKind, EventQuery, EventRepository, EventRow, MessageRepository, MessageRow,
    PermissionOperationRepository, PermissionOperationRow, PermissionOperationState,
    PermissionPayloadKind, PermissionRepository, PermissionRow, PostgresDatabase, RunRow, RunState,
    RuntimeSessionWrites, SessionRepository, SessionRow, StepRow, StepState, TaskRepository,
    TaskRow,
};

fn runtime_postgres_uri() -> String {
    assert!(
        sdkwork_database_config::workspace_database::workspace_postgres_env_is_configured(),
        "set SDKWORK_DATABASE_* to a disposable sdkwork_ai_test or sdkwork_ai_test_<run_id> PostgreSQL database"
    );
    let config = sdkwork_database_config::DatabaseConfig::from_env("agent_runtime")
        .expect("SDKWORK_DATABASE_* must resolve a valid workspace database profile");
    assert!(
        config.engine == sdkwork_database_config::DatabaseEngine::Postgres,
        "SDKWORK_DATABASE_ENGINE must be postgresql"
    );
    let schema = sdkwork_database_config::workspace_database::resolve_workspace_postgres_schema()
        .expect("SDKWORK_DATABASE_SCHEMA must match the workspace database");
    assert!(
        schema == "sdkwork_ai_test" || schema.starts_with("sdkwork_ai_test_"),
        "live PostgreSQL tests require sdkwork_ai_test or sdkwork_ai_test_<run_id>, got {schema}"
    );
    config.url
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_conditional_session_sync_rejects_stale_snapshot_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.conditional.pg.{}", uuid_like_suffix());
    let mut session = SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "provider".to_string(),
        state: "working".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: Some("hermes".to_string()),
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-07-15T00:00:00Z".to_string(),
        updated_at: Some("2026-07-15T00:02:00Z".to_string()),
        metadata_json: None,
    };
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: format!("evt.newer.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:02:00Z".to_string(),
            },
        )
        .expect("newer sync"));

    session.message_count = 23;
    session.owner_tenant_id = Some("tenant.updated".to_string());
    session.owner_user_ref = Some("user.updated".to_string());
    session.updated_at = Some("2026-07-15T00:03:00Z".to_string());
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: format!("evt.aggregate.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:03:00Z".to_string(),
            },
        )
        .expect("aggregate update"));
    let updated = db
        .load_session(&session_id)
        .expect("load updated")
        .expect("updated session");
    assert_eq!(updated.message_count, 23);
    assert_eq!(updated.owner_tenant_id.as_deref(), Some("tenant.updated"));

    let mut foreign = session.clone();
    foreign.provider_id = Some("codex".to_string());
    foreign.updated_at = Some("2026-07-15T00:04:00Z".to_string());
    assert!(!db
        .save_session_with_event_if_newer(
            &foreign,
            &EventRow {
                event_id: format!("evt.foreign.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:04:00Z".to_string(),
            },
        )
        .expect("provider conflict"));
    let ordinary_foreign_event_id = format!("evt.foreign.ordinary.{session_id}");
    assert!(matches!(
        db.save_session_with_event(
            &foreign,
            &EventRow {
                event_id: ordinary_foreign_event_id.clone(),
                session_id: Some(session_id.clone()),
                event_type: "session.updated".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:04:00Z".to_string(),
            },
        ),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));

    session.state = "paused".to_string();
    session.updated_at = Some("2026-07-15T00:01:00Z".to_string());
    let stale_event_id = format!("evt.stale.{session_id}");
    assert!(!db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: stale_event_id.clone(),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:01:00Z".to_string(),
            },
        )
        .expect("stale sync"));
    assert_eq!(
        db.load_session(&session_id)
            .expect("load")
            .expect("session")
            .state,
        "working"
    );
    assert!(db
        .load_events(
            &session_id,
            &EventQuery {
                limit: Some(20),
                ..EventQuery::default()
            },
        )
        .expect("events")
        .iter()
        .all(|event| event.event_id != stale_event_id));

    session.state = "closed".to_string();
    session.updated_at = Some("2026-07-15T00:05:00Z".to_string());
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: format!("evt.closed.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:05:00Z".to_string(),
            },
        )
        .expect("terminal write"));
    session.state = "active".to_string();
    session.updated_at = Some("2026-07-15T00:06:00Z".to_string());
    assert!(!db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: format!("evt.reopened.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:06:00Z".to_string(),
            },
        )
        .expect("terminal regression"));
    assert_eq!(
        db.load_session(&session_id)
            .expect("load terminal")
            .expect("terminal session")
            .state,
        "closed"
    );
    let ordinary_event_id = format!("evt.reopened.ordinary.{session_id}");
    assert!(matches!(
        db.save_session_with_event(
            &session,
            &EventRow {
                event_id: ordinary_event_id.clone(),
                session_id: Some(session_id.clone()),
                event_type: "session.updated".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:06:00Z".to_string(),
            },
        ),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(matches!(
        db.save_session(&session),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(matches!(
        db.update_session(&session),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(db
        .load_events(
            &session_id,
            &EventQuery {
                limit: Some(50),
                ..EventQuery::default()
            },
        )
        .expect("events after rejected ordinary write")
        .iter()
        .all(|event| {
            event.event_id != ordinary_event_id && event.event_id != ordinary_foreign_event_id
        }));
    assert!(matches!(
        db.append_message_with_event(
            &MessageRow {
                message_id: format!("msg.after-close.{session_id}"),
                session_id: session_id.clone(),
                role: "user".to_string(),
                content: "late".to_string(),
                created_at: "2026-07-15T00:06:01Z".to_string(),
                metadata_json: None,
            },
            &EventRow {
                event_id: format!("evt.after-close.{session_id}"),
                session_id: Some(session_id.clone()),
                event_type: "message.sent".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:06:01Z".to_string(),
            },
        ),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_session_message_roundtrip_when_uri_configured() {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.runtime.pg.{}", uuid_like_suffix());

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: Some("runtime contract".to_string()),
        model: None,
        cwd: None,
        provider_id: Some("hermes".to_string()),
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    db.save_message(&MessageRow {
        message_id: format!("msg.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "runtime postgres contract".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    })
    .expect("save message");

    let loaded = db.load_session(&session_id).expect("load").expect("found");
    assert_eq!(loaded.agent_id, "agent.runtime");

    let messages = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default(),
        )
        .expect("messages");
    assert_eq!(messages.len(), 1);

    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_event_identity_and_session_association_are_immutable_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.event.identity.pg.{}", uuid_like_suffix());
    let session = SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-07-15T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    };
    db.save_session(&session).expect("session");
    let original_event = EventRow {
        event_id: format!("evt.identity.{session_id}"),
        session_id: Some(session_id.clone()),
        event_type: "session.created".to_string(),
        severity: "info".to_string(),
        payload: Some("original".to_string()),
        created_at: "2026-07-15T00:00:00Z".to_string(),
    };
    db.save_event(&original_event).expect("event");
    db.save_event(&original_event).expect("idempotent event");

    let mut conflicting_event = original_event.clone();
    conflicting_event.payload = Some("changed".to_string());
    assert!(matches!(
        db.save_event(&conflicting_event),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));

    let message = MessageRow {
        message_id: format!("msg.identity.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "must roll back".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let mut mismatched_event = original_event.clone();
    mismatched_event.event_id = format!("evt.mismatch.{session_id}");
    mismatched_event.session_id = Some("session.other".to_string());
    assert!(matches!(
        db.append_message_with_event(&message, &mismatched_event),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(matches!(
        db.append_message_with_event(&message, &conflicting_event),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default()
        )
        .expect("messages")
        .is_empty());
    assert_eq!(
        db.load_session(&session_id)
            .expect("load")
            .expect("session")
            .message_count,
        0
    );
    let events = db
        .load_events(
            &session_id,
            &EventQuery {
                limit: Some(20),
                ..EventQuery::default()
            },
        )
        .expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload.as_deref(), Some("original"));
    db.delete_session_cascade(&session_id).expect("cleanup");
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_message_count_overflow_is_rejected_atomically_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.message-count.max.pg.{}", uuid_like_suffix());
    let session = SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: i64::MAX,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-07-15T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    };
    db.save_session(&session).expect("session");
    assert!(matches!(
        db.increment_session_message_count(&session_id),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));

    let message = MessageRow {
        message_id: format!("msg.message-count.max.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "overflow".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = EventRow {
        event_id: format!("evt.message-count.max.{session_id}"),
        session_id: Some(session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-07-15T00:00:01Z".to_string(),
    };
    assert!(matches!(
        db.append_message_with_event(&message, &event),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default()
        )
        .expect("messages")
        .is_empty());
    assert!(db
        .load_events(&session_id, &EventQuery::default())
        .expect("events")
        .is_empty());
    assert_eq!(
        db.load_session(&session_id)
            .expect("load")
            .expect("session")
            .message_count,
        i64::MAX
    );
    db.delete_session_cascade(&session_id).expect("cleanup");
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_update_session_preserves_messages_when_uri_configured() {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.runtime.pg.upsert.{}", uuid_like_suffix());

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    db.save_message(&MessageRow {
        message_id: format!("msg.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "must survive session update".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    })
    .expect("save message");

    let mut session = db.load_session(&session_id).expect("load").expect("found");
    session.state = "closed".to_string();
    session.message_count = 1;
    session.updated_at = Some("2026-06-23T00:00:02Z".to_string());
    db.update_session(&session).expect("update session");

    let messages = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default(),
        )
        .expect("messages");
    assert_eq!(messages.len(), 1);

    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_save_message_rejects_duplicate_message_id_with_different_content_when_uri_configured(
) {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.runtime.pg.save-conflict.{}", uuid_like_suffix());

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    let message = MessageRow {
        message_id: format!("msg.save-conflict.{}", uuid_like_suffix()),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "original payload".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    db.save_message(&message).expect("first save");

    let conflicting = MessageRow {
        content: "changed payload".to_string(),
        ..message.clone()
    };
    let error = db
        .save_message(&conflicting)
        .expect_err("duplicate message id with changed payload must fail");
    assert!(matches!(
        error,
        sdkwork_agent_database::DatabaseError::ConstraintViolation(_)
    ));

    let messages = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default(),
        )
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "original payload");

    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_save_message_rejects_duplicate_message_id_for_different_session_when_uri_configured(
) {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let first_session_id = format!("session.runtime.pg.save-conflict-a.{}", uuid_like_suffix());
    let second_session_id = format!("session.runtime.pg.save-conflict-b.{}", uuid_like_suffix());

    for session_id in [&first_session_id, &second_session_id] {
        db.save_session(&sdkwork_agent_database::SessionRow {
            session_id: session_id.clone(),
            agent_id: "agent.runtime".to_string(),
            kind: "main".to_string(),
            source: "contract-test".to_string(),
            state: "active".to_string(),
            title: None,
            model: None,
            cwd: None,
            provider_id: None,
            bridge_id: None,
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id: None,
            owner_user_ref: None,
            created_at: "2026-06-23T00:00:00Z".to_string(),
            updated_at: None,
            metadata_json: None,
        })
        .expect("save session");
    }

    let message = MessageRow {
        message_id: format!("msg.save-cross-session.{}", uuid_like_suffix()),
        session_id: first_session_id.clone(),
        role: "user".to_string(),
        content: "original payload".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    db.save_message(&message).expect("first save");

    let conflicting = MessageRow {
        session_id: second_session_id.clone(),
        ..message
    };
    let error = db
        .save_message(&conflicting)
        .expect_err("duplicate message id must not move across sessions");
    assert!(matches!(
        error,
        sdkwork_agent_database::DatabaseError::ConstraintViolation(_)
    ));

    assert_eq!(
        db.load_messages(
            &first_session_id,
            &sdkwork_agent_database::MessageQuery::default()
        )
        .expect("first messages")
        .len(),
        1
    );
    assert!(db
        .load_messages(
            &second_session_id,
            &sdkwork_agent_database::MessageQuery::default()
        )
        .expect("second messages")
        .is_empty());

    let _ = db.delete_session_cascade(&first_session_id);
    let _ = db.delete_session_cascade(&second_session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_permissions_roundtrip_when_uri_configured() {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let permission_id = format!("perm.runtime.pg.{}", uuid_like_suffix());

    db.save_permission(&PermissionRow {
        permission_request_id: permission_id.clone(),
        session_id: Some("session.runtime.pg".to_string()),
        category: "host.filesystem.read".to_string(),
        resource: "/workspace/README.md".to_string(),
        side_effect_level: "read_only".to_string(),
        reason: "contract test".to_string(),
        status: "pending".to_string(),
        owner_tenant_id: Some("tenant.contract".to_string()),
        owner_user_ref: Some("user.contract".to_string()),
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
    })
    .expect("save permission");

    let loaded = db
        .load_permission(&permission_id)
        .expect("load")
        .expect("found");
    assert_eq!(loaded.status, "pending");

    let listed = db
        .list_permissions(&sdkwork_agent_database::PermissionQuery {
            status: Some("pending".to_string()),
            limit: Some(20),
            offset: None,
            ..Default::default()
        })
        .expect("list");
    assert!(listed
        .iter()
        .any(|row| row.permission_request_id == permission_id));

    db.update_permission_status(&permission_id, "allow")
        .expect("allow permission");
    db.update_permission_status(&permission_id, "allow")
        .expect("same decision is idempotent");
    assert!(db.update_permission_status(&permission_id, "deny").is_err());
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_append_message_with_event_is_idempotent_when_uri_configured() {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.runtime.pg.idempotent.{}", uuid_like_suffix());

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    let message = MessageRow {
        message_id: format!("msg.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "retry-safe append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = sdkwork_agent_database::EventRow {
        event_id: format!("evt.{session_id}"),
        session_id: Some(session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:01Z".to_string(),
    };

    let first_count = db
        .append_message_with_event(&message, &event)
        .expect("first append");
    let retry_count = db
        .append_message_with_event(&message, &event)
        .expect("retry append");

    assert_eq!(first_count, 1);
    assert_eq!(retry_count, 1);
    let mut conflicting_event = event.clone();
    conflicting_event.payload = Some("conflict".to_string());
    assert!(matches!(
        db.append_message_with_event(&message, &conflicting_event),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert_eq!(db.message_count(&session_id).expect("message count"), 1);
    assert_eq!(
        db.load_session(&session_id)
            .expect("load")
            .expect("found")
            .message_count,
        1
    );
    assert_eq!(
        db.load_events(&session_id, &Default::default())
            .expect("events")
            .len(),
        1
    );

    let mut closed = db.load_session(&session_id).expect("load").expect("found");
    closed.state = "closed".to_string();
    db.update_session(&closed).expect("close session");
    assert_eq!(
        db.append_message_with_event(&message, &event)
            .expect("exact retry after close"),
        1
    );

    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_append_message_with_event_does_not_write_event_for_duplicate_message_id_when_uri_configured(
) {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.runtime.pg.duplicate-event.{}", uuid_like_suffix());

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    let message = MessageRow {
        message_id: format!("msg.duplicate-event.{}", uuid_like_suffix()),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "retry-safe append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let first_event = sdkwork_agent_database::EventRow {
        event_id: format!("evt.duplicate-event-a.{}", uuid_like_suffix()),
        session_id: Some(session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:01Z".to_string(),
    };
    let retry_event = sdkwork_agent_database::EventRow {
        event_id: format!("evt.duplicate-event-b.{}", uuid_like_suffix()),
        created_at: "2026-06-23T00:00:02Z".to_string(),
        ..first_event.clone()
    };

    db.append_message_with_event(&message, &first_event)
        .expect("first append");
    let retry_count = db
        .append_message_with_event(&message, &retry_event)
        .expect("retry append");

    let events = db
        .load_events(&session_id, &Default::default())
        .expect("events");
    assert_eq!(retry_count, 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, first_event.event_id);

    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_append_message_with_event_rejects_cross_session_duplicate_when_uri_configured() {
    let uri = runtime_postgres_uri();

    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let first_session_id = format!("session.runtime.pg.conflict-a.{}", uuid_like_suffix());
    let second_session_id = format!("session.runtime.pg.conflict-b.{}", uuid_like_suffix());

    for session_id in [&first_session_id, &second_session_id] {
        db.save_session(&sdkwork_agent_database::SessionRow {
            session_id: session_id.clone(),
            agent_id: "agent.runtime".to_string(),
            kind: "main".to_string(),
            source: "contract-test".to_string(),
            state: "active".to_string(),
            title: None,
            model: None,
            cwd: None,
            provider_id: None,
            bridge_id: None,
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id: None,
            owner_user_ref: None,
            created_at: "2026-06-23T00:00:00Z".to_string(),
            updated_at: None,
            metadata_json: None,
        })
        .expect("save session");
    }

    let message = MessageRow {
        message_id: format!("msg.conflict.{}", uuid_like_suffix()),
        session_id: first_session_id.clone(),
        role: "user".to_string(),
        content: "original append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = sdkwork_agent_database::EventRow {
        event_id: format!("evt.conflict.{}", uuid_like_suffix()),
        session_id: Some(first_session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:01Z".to_string(),
    };
    db.append_message_with_event(&message, &event)
        .expect("first append");

    let conflicting_message = MessageRow {
        session_id: second_session_id.clone(),
        content: "conflicting append".to_string(),
        ..message
    };
    let conflicting_event = sdkwork_agent_database::EventRow {
        event_id: format!("evt.conflict.{}", uuid_like_suffix()),
        session_id: Some(second_session_id.clone()),
        ..event
    };

    let error = db
        .append_message_with_event(&conflicting_message, &conflicting_event)
        .expect_err("duplicate message id must not move across sessions");
    assert!(matches!(
        error,
        sdkwork_agent_database::DatabaseError::ConstraintViolation(_)
    ));
    assert_eq!(db.message_count(&first_session_id).expect("first count"), 1);
    assert_eq!(
        db.message_count(&second_session_id).expect("second count"),
        0
    );
    assert!(db
        .load_events(&second_session_id, &Default::default())
        .expect("events")
        .is_empty());

    let _ = db.delete_session_cascade(&first_session_id);
    let _ = db.delete_session_cascade(&second_session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_task_ownership_terminal_and_cancel_contracts_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let suffix = uuid_like_suffix();
    let session_id = format!("session.task.pg.{suffix}");
    let other_session_id = format!("session.task.other.pg.{suffix}");
    for id in [&session_id, &other_session_id] {
        db.save_session(&SessionRow {
            session_id: id.clone(),
            agent_id: "agent.runtime".to_string(),
            kind: "main".to_string(),
            source: "contract-test".to_string(),
            state: "active".to_string(),
            title: None,
            model: None,
            cwd: None,
            provider_id: None,
            bridge_id: None,
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id: None,
            owner_user_ref: None,
            created_at: "2026-07-15T00:00:00Z".to_string(),
            updated_at: None,
            metadata_json: None,
        })
        .expect("session");
    }

    let task = TaskRow {
        task_id: format!("task.pg.{suffix}"),
        session_id: session_id.clone(),
        instruction: "run".to_string(),
        state: "Running".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        updated_at: None,
    };
    db.save_task(&task).expect("task");
    let foreign = TaskRow {
        session_id: other_session_id.clone(),
        ..task.clone()
    };
    assert!(db.save_task(&foreign).is_err());

    let cancel_event = EventRow {
        event_id: format!("evt.task.cancel.pg.{suffix}"),
        session_id: Some(session_id.clone()),
        event_type: "task.cancelled".to_string(),
        severity: "info".to_string(),
        payload: Some(task.task_id.clone()),
        created_at: "2026-07-15T00:00:02Z".to_string(),
    };
    let (cancelled, changed) = db
        .cancel_task_with_event(&task.task_id, "2026-07-15T00:00:02Z", &cancel_event)
        .expect("cancel");
    assert!(changed);
    assert_eq!(cancelled.state, "cancelled");
    assert!(
        !db.cancel_task_with_event(&task.task_id, "2026-07-15T00:00:03Z", &cancel_event)
            .expect("exact retry")
            .1
    );

    let mut conflicting_event = cancel_event.clone();
    conflicting_event.payload = Some("conflict".to_string());
    assert!(db
        .cancel_task_with_event(&task.task_id, "2026-07-15T00:00:03Z", &conflicting_event,)
        .is_err());
    let mut wrong_session_event = cancel_event.clone();
    wrong_session_event.event_id = format!("evt.task.cancel.wrong.pg.{suffix}");
    wrong_session_event.session_id = Some(other_session_id.clone());
    assert!(db
        .cancel_task_with_event(&task.task_id, "2026-07-15T00:00:03Z", &wrong_session_event,)
        .is_err());

    let reopened = TaskRow {
        state: "running".to_string(),
        ..cancelled
    };
    assert!(db.update_task(&reopened).is_err());
    let already_canceled = TaskRow {
        task_id: format!("task.already-canceled.pg.{suffix}"),
        session_id: session_id.clone(),
        instruction: "done".to_string(),
        state: "CANCELED".to_string(),
        created_at: "2026-07-15T00:00:02Z".to_string(),
        updated_at: None,
    };
    db.save_task(&already_canceled)
        .expect("already canceled task");
    assert!(
        !db.cancel_task_with_event(
            &already_canceled.task_id,
            "2026-07-15T00:00:03Z",
            &EventRow {
                event_id: format!("evt.task.already-canceled.pg.{suffix}"),
                session_id: Some(session_id.clone()),
                event_type: "task.cancelled".to_string(),
                severity: "info".to_string(),
                payload: Some(already_canceled.task_id.clone()),
                created_at: "2026-07-15T00:00:03Z".to_string(),
            },
        )
        .expect("already canceled")
        .1
    );
    assert_eq!(
        db.load_events(
            &session_id,
            &EventQuery {
                event_type: Some("task.cancelled".to_string()),
                limit: Some(20),
                ..Default::default()
            },
        )
        .expect("events")
        .len(),
        1
    );

    let mut closed = db
        .load_session(&session_id)
        .expect("load")
        .expect("session");
    closed.state = "closed".to_string();
    db.update_session(&closed).expect("close");
    let late_task = TaskRow {
        task_id: format!("task.late.pg.{suffix}"),
        session_id: session_id.clone(),
        instruction: "late".to_string(),
        state: "created".to_string(),
        created_at: "2026-07-15T00:00:04Z".to_string(),
        updated_at: None,
    };
    assert!(db
        .save_task_with_event(
            &late_task,
            &EventRow {
                event_id: format!("evt.task.late.pg.{suffix}"),
                session_id: Some(session_id.clone()),
                event_type: "task.created".to_string(),
                severity: "info".to_string(),
                payload: Some(late_task.task_id.clone()),
                created_at: "2026-07-15T00:00:04Z".to_string(),
            },
        )
        .is_err());
    assert!(db
        .load_task(&late_task.task_id)
        .expect("late lookup")
        .is_none());

    let _ = db.delete_session_cascade(&session_id);
    let _ = db.delete_session_cascade(&other_session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_completed_turn_retry_contract_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let session_id = format!("session.turn.retry.pg.{}", uuid_like_suffix());
    let mut session = SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-07-16T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    };
    db.save_session(&session).expect("session");
    let message = MessageRow {
        message_id: format!("msg.turn.retry.{session_id}"),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "hello".to_string(),
        created_at: "2026-07-16T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let events = vec![
        EventRow {
            event_id: format!("evt.turn.message.{session_id}"),
            session_id: Some(session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=user".to_string()),
            created_at: "2026-07-16T00:00:01Z".to_string(),
        },
        EventRow {
            event_id: format!("evt.turn.completed.{session_id}"),
            session_id: Some(session_id.clone()),
            event_type: "turn.completed".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-07-16T00:00:02Z".to_string(),
        },
    ];
    assert_eq!(
        db.append_message_turn_with_events(std::slice::from_ref(&message), &events)
            .expect("first turn"),
        1
    );
    let assistant = MessageRow {
        message_id: format!("msg.turn.partial-assistant.{session_id}"),
        session_id: session_id.clone(),
        role: "assistant".to_string(),
        content: "partial".to_string(),
        created_at: "2026-07-16T00:00:02Z".to_string(),
        metadata_json: None,
    };
    let partial_events = vec![
        events[0].clone(),
        EventRow {
            event_id: format!("evt.turn.partial-assistant.{session_id}"),
            session_id: Some(session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=assistant".to_string()),
            created_at: "2026-07-16T00:00:02Z".to_string(),
        },
        events[1].clone(),
    ];
    assert!(matches!(
        db.append_message_turn_with_events(&[message.clone(), assistant.clone()], &partial_events,),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert!(db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default()
        )
        .expect("messages")
        .iter()
        .all(|row| row.message_id != assistant.message_id));
    session.state = "closed".to_string();
    db.update_session(&session).expect("close");
    assert_eq!(
        db.append_message_turn_with_events(std::slice::from_ref(&message), &events)
            .expect("exact retry after close"),
        1
    );
    let mut conflicting_events = events;
    conflicting_events[1].payload = Some("conflict".to_string());
    assert!(matches!(
        db.append_message_turn_with_events(std::slice::from_ref(&message), &conflicting_events,),
        Err(sdkwork_agent_database::DatabaseError::ConstraintViolation(
            _
        ))
    ));
    assert_eq!(db.message_count(&session_id).expect("count"), 1);
    assert_eq!(
        db.load_events(&session_id, &EventQuery::default())
            .expect("events")
            .len(),
        2
    );
    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_permission_operation_claim_is_skip_locked_and_fenced_when_uri_configured() {
    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let suffix = uuid_like_suffix();
    let session_id = format!("session.permission.pg.{suffix}");
    let permission_id = format!("permission.pg.{suffix}");
    let task_id = format!("task.permission.pg.{suffix}");
    let run_id = format!("run.permission.pg.{suffix}");
    let step_id = format!("step.permission.pg.{suffix}");
    let now = sdkwork_agent_database::runtime_now_timestamp();
    db.save_session(&SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".into(),
        kind: "main".into(),
        source: "contract-test".into(),
        state: "active".into(),
        title: None,
        model: None,
        cwd: None,
        provider_id: None,
        bridge_id: None,
        token_usage_json: None,
        message_count: 0,
        owner_tenant_id: Some("tenant.test".into()),
        owner_user_ref: Some("user.test".into()),
        created_at: now.clone(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("session");
    let permission = PermissionRow {
        permission_request_id: permission_id.clone(),
        session_id: Some(session_id.clone()),
        category: "tool.invoke".into(),
        resource: "tool.protected".into(),
        side_effect_level: "side_effectful".into(),
        reason: "approval required".into(),
        status: "pending".into(),
        owner_tenant_id: Some("tenant.test".into()),
        owner_user_ref: Some("user.test".into()),
        created_at: now.clone(),
        updated_at: None,
    };
    let task = TaskRow {
        task_id: task_id.clone(),
        session_id: session_id.clone(),
        instruction: "execute approved tool".into(),
        state: "accepted".into(),
        created_at: now.clone(),
        updated_at: Some(now.clone()),
    };
    let run = RunRow {
        run_id: run_id.clone(),
        task_id: task_id.clone(),
        session_id: session_id.clone(),
        attempt: 1,
        state: RunState::AwaitingPermission,
        next_attempt_at: None,
        lease_owner: None,
        lease_expires_at: None,
        fencing_token: 0,
        cancel_requested_at: None,
        started_at: None,
        finished_at: None,
        error_kind: None,
        error_code: None,
        error_detail: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let step = StepRow {
        step_id: step_id.clone(),
        run_id: run_id.clone(),
        sequence_no: 0,
        action_kind: ActionKind::ToolCall,
        state: StepState::AwaitingPermission,
        provider_id: Some("provider.tool".into()),
        descriptor_revision: Some("1.0.0".into()),
        policy_revision: Some("1.0.0".into()),
        causation_step_id: None,
        idempotency_key_hash: None,
        result_json: None,
        error_kind: None,
        error_code: None,
        error_detail: None,
        started_at: None,
        finished_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let operation = PermissionOperationRow {
        permission_request_id: permission_id.clone(),
        run_id: run_id.clone(),
        step_id,
        tool_call_id: format!("tool-call.pg.{suffix}"),
        provider_id: "provider.tool".into(),
        descriptor_revision: "1.0.0".into(),
        policy_revision: "1.0.0".into(),
        payload_kind: PermissionPayloadKind::Ciphertext,
        payload_ref: "ciphertext".into(),
        payload_digest: "digest".into(),
        encryption_key_id: Some("key.v1".into()),
        state: PermissionOperationState::Pending,
        expires_at: "2099-01-01T00:00:00.000000000Z".into(),
        lease_owner: None,
        lease_expires_at: None,
        fencing_token: 0,
        result_json: None,
        error_kind: None,
        error_code: None,
        error_detail: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let requested = EventRow {
        event_id: format!("event.permission.requested.pg.{suffix}"),
        session_id: Some(session_id.clone()),
        event_type: "permission.requested".into(),
        severity: "warn".into(),
        payload: None,
        created_at: now.clone(),
    };
    db.create_permission_execution(&permission, &task, &run, &step, &operation, &requested)
        .expect("permission execution");
    db.decide_permission_operation(
        &permission_id,
        "allow",
        &now,
        &EventRow {
            event_id: format!("event.permission.allowed.pg.{suffix}"),
            event_type: "permission.allowed".into(),
            ..requested
        },
    )
    .expect("allow");

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = ["worker.one", "worker.two"].map(|worker| {
        let db = db.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            db.claim_permission_operation(
                worker,
                "2026-07-17T00:00:00.000000000Z",
                "2099-01-01T00:00:00.000000000Z",
            )
            .expect("claim")
        })
    });
    let claims = handles
        .into_iter()
        .map(|handle| handle.join().expect("claim thread"))
        .collect::<Vec<_>>();
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let claim = claims.into_iter().flatten().next().expect("one claim");
    assert_eq!(claim.operation.fencing_token, 1);
    db.complete_permission_operation(
        &claim,
        r#"{"output":"done"}"#,
        &sdkwork_agent_database::runtime_now_timestamp(),
        &EventRow {
            event_id: format!("event.permission.completed.pg.{suffix}"),
            session_id: Some(session_id.clone()),
            event_type: "permission.operation.completed".into(),
            severity: "info".into(),
            payload: None,
            created_at: sdkwork_agent_database::runtime_now_timestamp(),
        },
    )
    .expect("complete");
    let stored = db
        .load_permission_operation(&permission_id)
        .expect("load")
        .expect("operation");
    assert_eq!(stored.state, PermissionOperationState::Completed);
    assert!(stored.payload_ref.is_empty());
    let _ = db.delete_session_cascade(&session_id);
}

#[test]
#[ignore = "requires SDKWORK_DATABASE_URL and a disposable live PostgreSQL database"]
fn live_postgres_schema_status_reports_baseline_when_uri_configured() {
    use sdkwork_agent_database::RuntimeMaintenance;

    let uri = runtime_postgres_uri();
    let db = PostgresDatabase::connect_migrated(&uri).expect("postgres");
    let status = db.schema_status().expect("schema status");
    assert!(
        status.drift_free,
        "PostgreSQL readiness depends on schema_status: version={} expected={} drift_free=false",
        status.version, status.expected_version
    );
    assert_eq!(status.version, status.expected_version);
}

fn uuid_like_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos}")
}
