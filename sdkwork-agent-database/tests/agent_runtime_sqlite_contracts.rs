//! SQLite contract tests for agent runtime persistence.

use sdkwork_agent_database::{
    AgentDatabase, DatabaseError, EventQuery, EventRepository, EventRow, MessageQuery,
    MessageRepository, MessageRow, PermissionQuery, PermissionRepository, PermissionRow, TaskRow,
    RuntimeMaintenance, RuntimeSessionWrites, SessionRepository, SessionRow, SqliteDatabase,
    TaskRepository,
};

fn migrated_sqlite() -> SqliteDatabase {
    SqliteDatabase::memory_migrated().expect("sqlite")
}

#[test]
fn sqlite_conditional_session_sync_rejects_stale_snapshot_and_event() {
    let db = migrated_sqlite();
    let mut session = scoped_session("session.conditional.sqlite", "tenant.1", "user.1");
    session.provider_id = Some("opencode".to_string());
    session.state = "working".to_string();
    session.updated_at = Some("2026-07-15T00:02:00Z".to_string());
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: "evt.sqlite.newer".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:02:00Z".to_string(),
            },
        )
        .expect("newer sync"));

    session.message_count = 17;
    session.owner_tenant_id = Some("tenant.updated".to_string());
    session.owner_user_ref = Some("user.updated".to_string());
    session.updated_at = Some("2026-07-15T00:03:00Z".to_string());
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: "evt.sqlite.aggregate-update".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:03:00Z".to_string(),
            },
        )
        .expect("aggregate update"));
    let updated = db
        .load_session(&session.session_id)
        .expect("load updated")
        .expect("updated session");
    assert_eq!(updated.message_count, 17);
    assert_eq!(updated.owner_tenant_id.as_deref(), Some("tenant.updated"));

    let mut foreign = session.clone();
    foreign.provider_id = Some("codex".to_string());
    foreign.updated_at = Some("2026-07-15T00:04:00Z".to_string());
    assert!(!db
        .save_session_with_event_if_newer(
            &foreign,
            &EventRow {
                event_id: "evt.sqlite.foreign-provider".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:04:00Z".to_string(),
            },
        )
        .expect("provider conflict"));
    assert!(matches!(
        db.save_session_with_event(
            &foreign,
            &EventRow {
                event_id: "evt.sqlite.foreign-provider-ordinary".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.updated".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:04:00Z".to_string(),
            },
        ),
        Err(DatabaseError::ConstraintViolation(_))
    ));

    session.state = "paused".to_string();
    session.updated_at = Some("2026-07-15T00:01:00Z".to_string());
    assert!(!db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: "evt.sqlite.stale".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:01:00Z".to_string(),
            },
        )
        .expect("stale sync"));
    session.state = "failed".to_string();
    session.updated_at = Some("2026-07-15T08:01:00+08:00".to_string());
    assert!(!db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: "evt.sqlite.offset-stale".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T01:00:00Z".to_string(),
            },
        )
        .expect("offset stale sync"));
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load")
            .expect("session")
            .state,
        "working"
    );
    assert!(db
        .load_events(
            &session.session_id,
            &EventQuery {
                limit: Some(20),
                ..EventQuery::default()
            },
        )
        .expect("events")
        .iter()
        .all(|event| {
            event.event_id != "evt.sqlite.stale" && event.event_id != "evt.sqlite.offset-stale"
        }));

    session.state = "closed".to_string();
    session.updated_at = Some("2026-07-15T00:05:00Z".to_string());
    assert!(db
        .save_session_with_event_if_newer(
            &session,
            &EventRow {
                event_id: "evt.sqlite.closed".to_string(),
                session_id: Some(session.session_id.clone()),
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
                event_id: "evt.sqlite.reopened".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.synchronized".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:06:00Z".to_string(),
            },
        )
        .expect("terminal regression"));
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load terminal")
            .expect("terminal session")
            .state,
        "closed"
    );
    let ordinary_event_id = "evt.sqlite.reopened-ordinary";
    assert!(matches!(
        db.save_session_with_event(
            &session,
            &EventRow {
                event_id: ordinary_event_id.to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "session.updated".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-15T00:06:00Z".to_string(),
            },
        ),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(matches!(
        db.save_session(&session),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(matches!(
        db.update_session(&session),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(db
        .load_events(&session.session_id, &EventQuery::default())
        .expect("events after rejected ordinary write")
        .iter()
        .all(|event| {
            event.event_id != ordinary_event_id
                && event.event_id != "evt.sqlite.foreign-provider-ordinary"
        }));
}

fn scoped_session(session_id: &str, tenant: &str, user: &str) -> SessionRow {
    SessionRow {
        session_id: session_id.to_string(),
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
        owner_tenant_id: Some(tenant.to_string()),
        owner_user_ref: Some(user.to_string()),
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    }
}

#[test]
fn sqlite_event_identity_and_session_association_are_immutable() {
    let db = migrated_sqlite();
    let session = scoped_session("session.event.identity.sqlite", "tenant.1", "user.1");
    db.save_session(&session).expect("session");
    let original_event = EventRow {
        event_id: "evt.identity.sqlite".to_string(),
        session_id: Some(session.session_id.clone()),
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
        Err(DatabaseError::ConstraintViolation(_))
    ));

    let message = MessageRow {
        message_id: "msg.identity.sqlite".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "must roll back".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let mut mismatched_event = original_event.clone();
    mismatched_event.event_id = "evt.mismatch.sqlite".to_string();
    mismatched_event.session_id = Some("session.other".to_string());
    assert!(matches!(
        db.append_message_with_event(&message, &mismatched_event),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(matches!(
        db.append_message_with_event(&message, &conflicting_event),
        Err(DatabaseError::ConstraintViolation(_))
    ));

    assert!(db
        .load_messages(&session.session_id, &MessageQuery::default())
        .expect("messages")
        .is_empty());
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load")
            .expect("session")
            .message_count,
        0
    );
    let events = db
        .load_events(&session.session_id, &EventQuery::default())
        .expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload.as_deref(), Some("original"));
}

#[test]
fn sqlite_message_count_overflow_is_rejected_atomically() {
    let db = migrated_sqlite();
    let mut session = scoped_session("session.message-count.max.sqlite", "tenant.1", "user.1");
    session.message_count = i64::MAX;
    db.save_session(&session).expect("session");
    assert!(matches!(
        db.increment_session_message_count(&session.session_id),
        Err(DatabaseError::ConstraintViolation(_))
    ));

    let message = MessageRow {
        message_id: "msg.message-count.max.sqlite".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "overflow".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = EventRow {
        event_id: "evt.message-count.max.sqlite".to_string(),
        session_id: Some(session.session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-07-15T00:00:01Z".to_string(),
    };
    assert!(matches!(
        db.append_message_with_event(&message, &event),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(db
        .load_messages(&session.session_id, &MessageQuery::default())
        .expect("messages")
        .is_empty());
    assert!(db
        .load_events(&session.session_id, &EventQuery::default())
        .expect("events")
        .is_empty());
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load")
            .expect("session")
            .message_count,
        i64::MAX
    );
}

#[test]
fn sqlite_terminal_session_rejects_atomic_message_append() {
    let db = migrated_sqlite();
    let mut session = scoped_session("session.terminal.sqlite", "tenant.1", "user.1");
    session.state = "failed".to_string();
    db.save_session(&session).expect("terminal session");
    let message = MessageRow {
        message_id: "msg.terminal.sqlite".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "late".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = EventRow {
        event_id: "evt.terminal.sqlite".to_string(),
        session_id: Some(session.session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-07-15T00:00:01Z".to_string(),
    };

    assert!(matches!(
        db.append_message_with_event(&message, &event),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(db
        .load_messages(&session.session_id, &MessageQuery::default())
        .expect("messages")
        .is_empty());
    assert!(db
        .load_events(&session.session_id, &EventQuery::default())
        .expect("events")
        .is_empty());
}

#[test]
fn sqlite_message_clear_event_is_atomic_observable_and_conflict_safe() {
    let db = migrated_sqlite();
    let session = scoped_session("session.clear.sqlite", "tenant.1", "user.1");
    db.save_session(&session).expect("session");
    let message = MessageRow {
        message_id: "msg.clear.sqlite.1".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "clear me".to_string(),
        created_at: "2026-07-15T00:00:01Z".to_string(),
        metadata_json: None,
    };
    db.append_message_with_event(
        &message,
        &EventRow {
            event_id: "evt.clear.sqlite.message.1".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-07-15T00:00:01Z".to_string(),
        },
    )
    .expect("message");
    let clear_event = EventRow {
        event_id: "evt.clear.sqlite.success".to_string(),
        session_id: Some(session.session_id.clone()),
        event_type: "session.updated".to_string(),
        severity: "info".to_string(),
        payload: Some("messages_cleared=true".to_string()),
        created_at: "2026-07-15T00:00:02Z".to_string(),
    };
    db.delete_messages_and_reset_count_with_event(
        &session.session_id,
        "2026-07-15T00:00:02Z",
        &clear_event,
    )
    .expect("clear");
    assert!(db
        .load_messages(&session.session_id, &MessageQuery::default())
        .expect("messages")
        .is_empty());
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load")
            .expect("session")
            .message_count,
        0
    );

    let second_message = MessageRow {
        message_id: "msg.clear.sqlite.2".to_string(),
        created_at: "2026-07-15T00:00:03Z".to_string(),
        ..message
    };
    db.append_message_with_event(
        &second_message,
        &EventRow {
            event_id: "evt.clear.sqlite.message.2".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-07-15T00:00:03Z".to_string(),
        },
    )
    .expect("second message");
    let collision = EventRow {
        event_id: "evt.clear.sqlite.collision".to_string(),
        session_id: Some(session.session_id.clone()),
        event_type: "session.updated".to_string(),
        severity: "info".to_string(),
        payload: Some("original".to_string()),
        created_at: "2026-07-15T00:00:04Z".to_string(),
    };
    db.save_event(&collision).expect("collision seed");
    let conflicting_clear = EventRow {
        payload: Some("messages_cleared=true".to_string()),
        ..collision
    };
    assert!(matches!(
        db.delete_messages_and_reset_count_with_event(
            &session.session_id,
            "2026-07-15T00:00:05Z",
            &conflicting_clear,
        ),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert_eq!(
        db.load_messages(&session.session_id, &MessageQuery::default())
            .expect("messages after rollback")
            .len(),
        1
    );
    assert_eq!(
        db.load_session(&session.session_id)
            .expect("load after rollback")
            .expect("session")
            .message_count,
        1
    );
}

#[test]
fn sqlite_permission_decision_transition_is_atomic_and_idempotent() {
    let db = migrated_sqlite();
    let permission_id = "permission.contract.transition";
    db.save_permission(&PermissionRow {
        permission_request_id: permission_id.to_string(),
        session_id: None,
        category: "host.filesystem.write".to_string(),
        resource: "/workspace/file".to_string(),
        side_effect_level: "side_effectful".to_string(),
        reason: "contract".to_string(),
        status: "pending".to_string(),
        owner_tenant_id: Some("tenant.contract".to_string()),
        owner_user_ref: Some("user.contract".to_string()),
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
    })
    .expect("save permission");

    db.update_permission_status(permission_id, "allow")
        .expect("allow permission");
    db.update_permission_status(permission_id, "allow")
        .expect("same decision is idempotent");
    assert!(db.update_permission_status(permission_id, "deny").is_err());
    assert!(db
        .update_permission_status(permission_id, "approved")
        .is_err());
    assert_eq!(
        db.load_permission(permission_id)
            .expect("load")
            .expect("permission")
            .status,
        "allow"
    );
}

#[test]
fn sqlite_session_message_roundtrip() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.1".to_string();

    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.clone(),
        agent_id: "agent.runtime".to_string(),
        kind: "main".to_string(),
        source: "contract-test".to_string(),
        state: "active".to_string(),
        title: Some("runtime contract".to_string()),
        model: None,
        cwd: None,
        provider_id: Some("rig".to_string()),
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
        content: "runtime sqlite contract".to_string(),
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

    db.delete_session_cascade(&session_id).expect("cascade");
    assert!(db.load_session(&session_id).expect("load").is_none());
}

#[test]
fn sqlite_cascade_delete_removes_permissions() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.perm".to_string();

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

    let permission_id = "perm.runtime.sqlite.1".to_string();
    db.save_permission(&PermissionRow {
        permission_request_id: permission_id.clone(),
        session_id: Some(session_id.clone()),
        category: "host.filesystem.read".to_string(),
        resource: "/workspace/README.md".to_string(),
        side_effect_level: "read_only".to_string(),
        reason: "contract test".to_string(),
        status: "pending".to_string(),
        owner_tenant_id: None,
        owner_user_ref: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
    })
    .expect("save permission");

    db.delete_session_cascade(&session_id).expect("cascade");
    assert!(db
        .load_permission(&permission_id)
        .expect("load permission")
        .is_none());
}

#[test]
fn sqlite_event_replay_after_event_id_is_strict() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.events".to_string();

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

    for (event_id, created_at) in [
        ("evt.1", "2026-06-23T00:00:01Z"),
        ("evt.2", "2026-06-23T00:00:02Z"),
        ("evt.3", "2026-06-23T00:00:03Z"),
    ] {
        db.save_event(&sdkwork_agent_database::EventRow {
            event_id: event_id.to_string(),
            session_id: Some(session_id.clone()),
            event_type: "test.event".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: created_at.to_string(),
        })
        .expect("save event");
    }

    let after_first = db
        .load_events(
            &session_id,
            &EventQuery {
                after_event_id: Some("evt.1".to_string()),
                limit: Some(10),
                ..EventQuery::default()
            },
        )
        .expect("after first");
    assert_eq!(after_first.len(), 2);
    assert_eq!(after_first[0].event_id, "evt.2");

    let missing_cursor = db
        .load_events(
            &session_id,
            &EventQuery {
                after_event_id: Some("evt.missing".to_string()),
                limit: Some(10),
                ..EventQuery::default()
            },
        )
        .expect("missing cursor");
    assert!(missing_cursor.is_empty());

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_message_list_after_message_id_is_strict() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.messages".to_string();

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

    for (message_id, created_at) in [
        ("msg.1", "2026-06-23T00:00:01Z"),
        ("msg.2", "2026-06-23T00:00:02Z"),
        ("msg.3", "2026-06-23T00:00:03Z"),
    ] {
        db.save_message(&MessageRow {
            message_id: message_id.to_string(),
            session_id: session_id.clone(),
            role: "user".to_string(),
            content: message_id.to_string(),
            created_at: created_at.to_string(),
            metadata_json: None,
        })
        .expect("save message");
    }

    let after_first = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery {
                after_message_id: Some("msg.1".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("after first");
    assert_eq!(after_first.len(), 2);
    assert_eq!(after_first[0].message_id, "msg.2");

    let missing_cursor = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery {
                after_message_id: Some("msg.missing".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("missing cursor");
    assert!(missing_cursor.is_empty());

    db.execute("DELETE FROM messages WHERE message_id = ?1", &[&"msg.1"])
        .expect("delete cursor message");
    let after_deleted_cursor = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery {
                after_message_id: Some("msg.1".to_string()),
                after_message_created_at: Some("2026-06-23T00:00:01Z".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("continue after deleted message cursor");
    assert_eq!(after_deleted_cursor.len(), 2);
    assert_eq!(after_deleted_cursor[0].message_id, "msg.2");

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_session_list_after_session_id_is_strict() {
    let db = migrated_sqlite();
    for (session_id, created_at) in [
        ("session.a", "2026-06-23T00:00:03Z"),
        ("session.b", "2026-06-23T00:00:02Z"),
        ("session.c", "2026-06-23T00:00:01Z"),
    ] {
        db.save_session(&sdkwork_agent_database::SessionRow {
            session_id: session_id.to_string(),
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
            created_at: created_at.to_string(),
            updated_at: Some(created_at.to_string()),
            metadata_json: None,
        })
        .expect("save session");
    }

    let after_first = db
        .list_sessions(&sdkwork_agent_database::SessionQuery {
            after_session_id: Some("session.a".to_string()),
            limit: Some(10),
            ..Default::default()
        })
        .expect("after first");
    assert_eq!(after_first.len(), 2);
    assert_eq!(after_first[0].session_id, "session.b");

    let missing_cursor = db
        .list_sessions(&sdkwork_agent_database::SessionQuery {
            after_session_id: Some("session.missing".to_string()),
            limit: Some(10),
            ..Default::default()
        })
        .expect("missing cursor");
    assert!(missing_cursor.is_empty());

    db.delete_session("session.a")
        .expect("delete cursor session");
    let after_deleted_cursor = db
        .list_sessions(&sdkwork_agent_database::SessionQuery {
            after_session_id: Some("session.a".to_string()),
            after_session_sort_at: Some("2026-06-23T00:00:03Z".to_string()),
            limit: Some(10),
            ..Default::default()
        })
        .expect("continue after deleted session cursor");
    assert_eq!(after_deleted_cursor.len(), 2);
    assert_eq!(after_deleted_cursor[0].session_id, "session.b");

    for session_id in ["session.a", "session.b", "session.c"] {
        db.delete_session_cascade(session_id).expect("cascade");
    }
}

#[test]
fn sqlite_task_list_after_task_id_is_strict() {
    let db = migrated_sqlite();
    let session_id = "session.tasks";
    db.save_session(&sdkwork_agent_database::SessionRow {
        session_id: session_id.to_string(),
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

    for (task_id, created_at) in [
        ("task.1", "2026-06-23T00:00:01Z"),
        ("task.2", "2026-06-23T00:00:02Z"),
        ("task.3", "2026-06-23T00:00:03Z"),
    ] {
        db.save_task(&sdkwork_agent_database::TaskRow {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            instruction: task_id.to_string(),
            state: "pending".to_string(),
            created_at: created_at.to_string(),
            updated_at: None,
        })
        .expect("save task");
    }

    let after_first = db
        .load_tasks(
            session_id,
            &sdkwork_agent_database::TaskQuery {
                after_task_id: Some("task.1".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("after first");
    assert_eq!(after_first.len(), 2);
    assert_eq!(after_first[0].task_id, "task.2");

    let missing_cursor = db
        .load_tasks(
            session_id,
            &sdkwork_agent_database::TaskQuery {
                after_task_id: Some("task.missing".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("missing cursor");
    assert!(missing_cursor.is_empty());

    db.delete_task("task.1").expect("delete cursor task");
    let after_deleted_cursor = db
        .load_tasks(
            session_id,
            &sdkwork_agent_database::TaskQuery {
                after_task_id: Some("task.1".to_string()),
                after_task_created_at: Some("2026-06-23T00:00:01Z".to_string()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("continue after deleted task cursor");
    assert_eq!(after_deleted_cursor.len(), 2);
    assert_eq!(after_deleted_cursor[0].task_id, "task.2");

    db.delete_session_cascade(session_id).expect("cascade");
}

#[test]
fn sqlite_update_session_preserves_messages() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.upsert".to_string();

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
        owner_tenant_id: Some("tenant.original".to_string()),
        owner_user_ref: Some("user.original".to_string()),
        created_at: "2026-06-23T00:00:00Z".to_string(),
        updated_at: None,
        metadata_json: None,
    })
    .expect("save session");

    let mut stale_session = db.load_session(&session_id).expect("load").expect("found");
    db.append_message_with_event(
        &MessageRow {
            message_id: "msg.preserve.1".to_string(),
            session_id: session_id.clone(),
            role: "user".to_string(),
            content: "must survive session update".to_string(),
            created_at: "2026-06-23T00:00:01Z".to_string(),
            metadata_json: None,
        },
        &EventRow {
            event_id: "evt.preserve.1".to_string(),
            session_id: Some(session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-06-23T00:00:01Z".to_string(),
        },
    )
    .expect("append message");

    stale_session.state = "closed".to_string();
    stale_session.owner_tenant_id = Some("tenant.overwrite".to_string());
    stale_session.owner_user_ref = Some("user.overwrite".to_string());
    stale_session.created_at = "2030-01-01T00:00:00Z".to_string();
    stale_session.updated_at = Some("2026-06-23T00:00:02Z".to_string());
    db.update_session(&stale_session).expect("update session");
    db.save_session_with_event(
        &stale_session,
        &EventRow {
            event_id: "evt.session.preserve".to_string(),
            session_id: Some(session_id.clone()),
            event_type: "session.closed".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-06-23T00:00:02Z".to_string(),
        },
    )
    .expect("transactional session update");

    let messages = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default(),
        )
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, "msg.preserve.1");
    let updated = db.load_session(&session_id).expect("load").expect("found");
    assert_eq!(updated.message_count, 1);
    assert_eq!(updated.owner_tenant_id.as_deref(), Some("tenant.original"));
    assert_eq!(updated.owner_user_ref.as_deref(), Some("user.original"));
    assert_eq!(updated.created_at, "2026-06-23T00:00:00Z");

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_save_message_rejects_duplicate_message_id_with_different_content() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.save-conflict".to_string();

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
        message_id: "msg.sqlite.save-conflict.1".to_string(),
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
    assert!(matches!(error, DatabaseError::ConstraintViolation(_)));

    let messages = db
        .load_messages(
            &session_id,
            &sdkwork_agent_database::MessageQuery::default(),
        )
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "original payload");

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_save_message_rejects_duplicate_message_id_for_different_session() {
    let db = migrated_sqlite();
    let first_session_id = "session.runtime.sqlite.save-conflict-a".to_string();
    let second_session_id = "session.runtime.sqlite.save-conflict-b".to_string();

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
        message_id: "msg.sqlite.save-cross-session.1".to_string(),
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
    assert!(matches!(error, DatabaseError::ConstraintViolation(_)));

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

    db.delete_session_cascade(&first_session_id)
        .expect("cascade");
    db.delete_session_cascade(&second_session_id)
        .expect("cascade");
}

#[test]
fn sqlite_append_message_with_event_is_idempotent_for_duplicate_message_id() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.idempotent-append".to_string();

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
        message_id: "msg.idempotent.1".to_string(),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "retry-safe append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = sdkwork_agent_database::EventRow {
        event_id: "evt.idempotent.1".to_string(),
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
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert_eq!(db.message_count(&session_id).expect("message count"), 1);
    let session = db.load_session(&session_id).expect("load").expect("found");
    assert_eq!(session.message_count, 1);
    assert_eq!(
        db.load_messages(&session_id, &Default::default())
            .expect("messages")
            .len(),
        1
    );
    assert_eq!(
        db.load_events(&session_id, &EventQuery::default())
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

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_append_message_with_event_does_not_write_event_for_duplicate_message_id() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.duplicate-event".to_string();

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
        message_id: "msg.sqlite.duplicate-event.1".to_string(),
        session_id: session_id.clone(),
        role: "user".to_string(),
        content: "retry-safe append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let first_event = sdkwork_agent_database::EventRow {
        event_id: "evt.sqlite.duplicate-event.1".to_string(),
        session_id: Some(session_id.clone()),
        event_type: "message.sent".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:01Z".to_string(),
    };
    let retry_event = sdkwork_agent_database::EventRow {
        event_id: "evt.sqlite.duplicate-event.2".to_string(),
        created_at: "2026-06-23T00:00:02Z".to_string(),
        ..first_event.clone()
    };

    db.append_message_with_event(&message, &first_event)
        .expect("first append");
    let retry_count = db
        .append_message_with_event(&message, &retry_event)
        .expect("retry append");

    let events = db
        .load_events(&session_id, &EventQuery::default())
        .expect("events");
    assert_eq!(retry_count, 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt.sqlite.duplicate-event.1");

    db.delete_session_cascade(&session_id).expect("cascade");
}

#[test]
fn sqlite_append_message_with_event_rejects_duplicate_message_id_for_different_session() {
    let db = migrated_sqlite();
    let first_session_id = "session.runtime.sqlite.conflict-a".to_string();
    let second_session_id = "session.runtime.sqlite.conflict-b".to_string();

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
        message_id: "msg.conflict.1".to_string(),
        session_id: first_session_id.clone(),
        role: "user".to_string(),
        content: "original append".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let event = sdkwork_agent_database::EventRow {
        event_id: "evt.conflict.1".to_string(),
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
        event_id: "evt.conflict.2".to_string(),
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
    assert_eq!(
        db.load_messages(&first_session_id, &Default::default())
            .expect("first messages")
            .len(),
        1
    );
    assert!(db
        .load_messages(&second_session_id, &Default::default())
        .expect("second messages")
        .is_empty());
    assert!(db
        .load_events(&second_session_id, &EventQuery::default())
        .expect("second events")
        .is_empty());

    db.delete_session_cascade(&first_session_id)
        .expect("cascade a");
    db.delete_session_cascade(&second_session_id)
        .expect("cascade b");
}

#[test]
fn sqlite_message_cursor_is_stable_for_equal_timestamps() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.equal-time";
    db.save_session(&scoped_session(session_id, "tenant.a", "user.a"))
        .expect("session");

    for message_id in ["msg.z", "msg.a"] {
        db.save_message(&MessageRow {
            message_id: message_id.to_string(),
            session_id: session_id.to_string(),
            role: "user".to_string(),
            content: message_id.to_string(),
            created_at: "2026-06-23T00:00:01Z".to_string(),
            metadata_json: None,
        })
        .expect("message");
    }

    let first = db
        .load_messages(
            session_id,
            &MessageQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .expect("first page");
    assert_eq!(first[0].message_id, "msg.a");

    let second = db
        .load_messages(
            session_id,
            &MessageQuery {
                after_message_id: Some(first[0].message_id.clone()),
                limit: Some(1),
                ..Default::default()
            },
        )
        .expect("second page");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].message_id, "msg.z");
}

#[test]
fn sqlite_recent_messages_returns_bounded_tail_in_chronological_order() {
    let db = migrated_sqlite();
    let session_id = "session.runtime.sqlite.recent";
    db.save_session(&scoped_session(session_id, "tenant.a", "user.a"))
        .expect("session");
    for index in 1..=6 {
        db.save_message(&MessageRow {
            message_id: format!("msg.{index}"),
            session_id: session_id.to_string(),
            role: "user".to_string(),
            content: index.to_string(),
            created_at: format!("2026-06-23T00:00:0{index}Z"),
            metadata_json: None,
        })
        .expect("message");
    }

    let recent = db
        .load_recent_messages(session_id, 3)
        .expect("recent messages");
    assert_eq!(
        recent
            .iter()
            .map(|row| row.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["msg.4", "msg.5", "msg.6"]
    );
    assert!(db.load_recent_messages(session_id, 0).is_err());
    assert!(db.load_recent_messages(session_id, 513).is_err());
}

#[test]
fn sqlite_event_and_permission_queries_enforce_tenant_user_scope() {
    let db = migrated_sqlite();
    for (session_id, tenant, user) in [
        ("session.scope.a", "tenant.a", "user.a"),
        ("session.scope.b", "tenant.b", "user.b"),
    ] {
        db.save_session(&scoped_session(session_id, tenant, user))
            .expect("session");
        db.save_event(&EventRow {
            event_id: format!("evt.{tenant}"),
            session_id: Some(session_id.to_string()),
            event_type: "scope.test".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-06-23T00:00:01Z".to_string(),
        })
        .expect("event");
        db.save_permission(&PermissionRow {
            permission_request_id: format!("perm.{tenant}"),
            session_id: Some(session_id.to_string()),
            category: "host.filesystem.read".to_string(),
            resource: "/workspace".to_string(),
            side_effect_level: "read_only".to_string(),
            reason: "scope test".to_string(),
            status: "pending".to_string(),
            owner_tenant_id: Some(tenant.to_string()),
            owner_user_ref: Some(user.to_string()),
            created_at: "2026-06-23T00:00:01Z".to_string(),
            updated_at: None,
        })
        .expect("permission");
    }
    db.save_event(&EventRow {
        event_id: "evt.global".to_string(),
        session_id: None,
        event_type: "scope.test".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:02Z".to_string(),
    })
    .expect("global event");

    let events = db
        .list_recent_events(&EventQuery {
            owner_tenant_id: Some("tenant.a".to_string()),
            owner_user_ref: Some("user.a".to_string()),
            limit: Some(20),
            ..Default::default()
        })
        .expect("scoped events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt.tenant.a");

    let permissions = db
        .list_permissions(&PermissionQuery {
            status: Some("pending".to_string()),
            owner_tenant_id: Some("tenant.a".to_string()),
            owner_user_ref: Some("user.a".to_string()),
            limit: Some(20),
            ..Default::default()
        })
        .expect("scoped permissions");
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0].permission_request_id, "perm.tenant.a");
}

#[test]
fn sqlite_session_and_task_event_writes_roll_back_together() {
    let db = migrated_sqlite();
    let session = scoped_session("session.atomic", "tenant.a", "user.a");
    let invalid_event = EventRow {
        event_id: "evt.atomic.invalid".to_string(),
        session_id: Some("session.missing".to_string()),
        event_type: "session.created".to_string(),
        severity: "info".to_string(),
        payload: None,
        created_at: "2026-06-23T00:00:00Z".to_string(),
    };
    assert!(db
        .save_session_with_event(&session, &invalid_event)
        .is_err());
    assert!(db
        .load_session(&session.session_id)
        .expect("session lookup")
        .is_none());

    db.save_session(&session).expect("parent session");
    let task = sdkwork_agent_database::TaskRow {
        task_id: "task.atomic".to_string(),
        session_id: session.session_id.clone(),
        instruction: "verify transaction".to_string(),
        state: "created".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        updated_at: None,
    };
    assert!(db.save_task_with_event(&task, &invalid_event).is_err());
    assert!(db.load_task(&task.task_id).expect("task lookup").is_none());
}

#[test]
fn sqlite_task_cancellation_is_atomic_idempotent_and_state_checked() {
    let db = migrated_sqlite();
    let session = scoped_session("session.task.cancel", "tenant.a", "user.a");
    db.save_session(&session).expect("parent session");
    let task = sdkwork_agent_database::TaskRow {
        task_id: "task.cancel.atomic".to_string(),
        session_id: session.session_id.clone(),
        instruction: "cancel atomically".to_string(),
        state: "running".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        updated_at: None,
    };
    db.save_task(&task).expect("task");
    let first_event = EventRow {
        event_id: "evt.task.cancel.first".to_string(),
        session_id: Some(session.session_id.clone()),
        event_type: "task.cancelled".to_string(),
        severity: "info".to_string(),
        payload: Some(task.task_id.clone()),
        created_at: "2026-06-23T00:00:02Z".to_string(),
    };
    let (cancelled, changed) = db
        .cancel_task_with_event(&task.task_id, "2026-06-23T00:00:02Z", &first_event)
        .expect("cancel task");
    assert!(changed);
    assert_eq!(cancelled.state, "cancelled");

    let retry_event = EventRow {
        event_id: "evt.task.cancel.retry".to_string(),
        ..first_event.clone()
    };
    let (_, changed) = db
        .cancel_task_with_event(&task.task_id, "2026-06-23T00:00:03Z", &retry_event)
        .expect("idempotent retry");
    assert!(!changed);
    let mut conflicting_event = first_event.clone();
    conflicting_event.payload = Some("conflict".to_string());
    assert!(matches!(
        db.cancel_task_with_event(&task.task_id, "2026-06-23T00:00:03Z", &conflicting_event,),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    let mut wrong_session_event = retry_event.clone();
    wrong_session_event.event_id = "evt.task.cancel.wrong-session".to_string();
    wrong_session_event.session_id = Some("session.task.other".to_string());
    assert!(matches!(
        db.cancel_task_with_event(&task.task_id, "2026-06-23T00:00:03Z", &wrong_session_event,),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    let events = db
        .load_events(
            &session.session_id,
            &EventQuery {
                event_type: Some("task.cancelled".to_string()),
                limit: Some(20),
                ..Default::default()
            },
        )
        .expect("events");
    assert_eq!(events.len(), 1);

    let already_canceled = sdkwork_agent_database::TaskRow {
        task_id: "task.cancel.already".to_string(),
        state: "CANCELED".to_string(),
        ..task.clone()
    };
    db.save_task(&already_canceled)
        .expect("already canceled task");
    let (_, changed) = db
        .cancel_task_with_event(
            &already_canceled.task_id,
            "2026-06-23T00:00:03Z",
            &EventRow {
                event_id: "evt.task.cancel.already".to_string(),
                payload: Some(already_canceled.task_id.clone()),
                ..retry_event
            },
        )
        .expect("already canceled");
    assert!(!changed);
    assert_eq!(
        db.load_events(
            &session.session_id,
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

    let mut reopened = cancelled.clone();
    reopened.state = "running".to_string();
    assert!(matches!(
        db.update_task(&reopened),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    let other_session = scoped_session("session.task.other", "tenant.a", "user.a");
    db.save_session(&other_session).expect("other session");
    let foreign = sdkwork_agent_database::TaskRow {
        session_id: other_session.session_id,
        state: "running".to_string(),
        ..cancelled.clone()
    };
    assert!(matches!(
        db.save_task(&foreign),
        Err(DatabaseError::ConstraintViolation(_))
    ));

    let completed = sdkwork_agent_database::TaskRow {
        task_id: "task.cancel.completed".to_string(),
        state: "completed".to_string(),
        ..task
    };
    db.save_task(&completed).expect("completed task");
    assert!(db
        .cancel_task_with_event(
            &completed.task_id,
            "2026-06-23T00:00:04Z",
            &EventRow {
                event_id: "evt.task.cancel.completed".to_string(),
                payload: Some(completed.task_id.clone()),
                ..first_event
            },
        )
        .is_err());

    let mut terminal_session = db
        .load_session(&session.session_id)
        .expect("load session")
        .expect("session");
    terminal_session.state = "closed".to_string();
    db.update_session(&terminal_session).expect("close session");
    let late_task = sdkwork_agent_database::TaskRow {
        task_id: "task.after-close".to_string(),
        session_id: session.session_id.clone(),
        instruction: "late".to_string(),
        state: "created".to_string(),
        created_at: "2026-06-23T00:00:05Z".to_string(),
        updated_at: None,
    };
    assert!(matches!(
        db.save_task_with_event(
            &late_task,
            &EventRow {
                event_id: "evt.task.after-close".to_string(),
                session_id: Some(session.session_id.clone()),
                event_type: "task.created".to_string(),
                severity: "info".to_string(),
                payload: Some(late_task.task_id.clone()),
                created_at: "2026-06-23T00:00:05Z".to_string(),
            },
        ),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(db
        .load_task(&late_task.task_id)
        .expect("late task lookup")
        .is_none());
}

#[test]
fn sqlite_completed_turn_rolls_back_when_a_later_message_conflicts() {
    let db = migrated_sqlite();
    let session = scoped_session("session.turn.atomic", "tenant.a", "user.a");
    db.save_session(&session).expect("parent session");

    let existing_assistant = MessageRow {
        message_id: "message.turn.conflict".to_string(),
        session_id: session.session_id.clone(),
        role: "assistant".to_string(),
        content: "original".to_string(),
        created_at: "2026-06-23T00:00:01Z".to_string(),
        metadata_json: None,
    };
    db.append_message_with_event(
        &existing_assistant,
        &EventRow {
            event_id: "event.turn.conflict.existing".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=assistant".to_string()),
            created_at: "2026-06-23T00:00:01Z".to_string(),
        },
    )
    .expect("existing assistant");

    let user = MessageRow {
        message_id: "message.turn.user".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "new user".to_string(),
        created_at: "2026-06-23T00:00:02Z".to_string(),
        metadata_json: None,
    };
    let conflicting_assistant = MessageRow {
        content: "changed assistant".to_string(),
        created_at: "2026-06-23T00:00:03Z".to_string(),
        ..existing_assistant.clone()
    };
    let events = vec![
        EventRow {
            event_id: "event.turn.conflict.user".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=user".to_string()),
            created_at: "2026-06-23T00:00:02Z".to_string(),
        },
        EventRow {
            event_id: "event.turn.conflict.assistant".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=assistant".to_string()),
            created_at: "2026-06-23T00:00:03Z".to_string(),
        },
        EventRow {
            event_id: "event.turn.conflict.completed".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "turn.completed".to_string(),
            severity: "info".to_string(),
            payload: Some("user_message_id=message.turn.user".to_string()),
            created_at: "2026-06-23T00:00:04Z".to_string(),
        },
    ];

    assert!(db
        .append_message_turn_with_events(&[user, conflicting_assistant], &events)
        .is_err());
    let messages = db
        .load_messages(&session.session_id, &MessageQuery::default())
        .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "original");
    assert_eq!(db.message_count(&session.session_id).expect("count"), 1);
    assert_eq!(
        db.load_events(&session.session_id, &EventQuery::default())
            .expect("events")
            .len(),
        1
    );
}

#[test]
fn sqlite_completed_turn_retry_is_idempotent_after_close_and_validates_event_identity() {
    let db = migrated_sqlite();
    let mut session = scoped_session("session.turn.retry.sqlite", "tenant.a", "user.a");
    db.save_session(&session).expect("session");
    let message = MessageRow {
        message_id: "msg.turn.retry.sqlite".to_string(),
        session_id: session.session_id.clone(),
        role: "user".to_string(),
        content: "hello".to_string(),
        created_at: "2026-07-16T00:00:01Z".to_string(),
        metadata_json: None,
    };
    let events = vec![
        EventRow {
            event_id: "evt.turn.message.sqlite".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=user".to_string()),
            created_at: "2026-07-16T00:00:01Z".to_string(),
        },
        EventRow {
            event_id: "evt.turn.completed.sqlite".to_string(),
            session_id: Some(session.session_id.clone()),
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
        message_id: "msg.turn.partial-assistant.sqlite".to_string(),
        session_id: session.session_id.clone(),
        role: "assistant".to_string(),
        content: "partial".to_string(),
        created_at: "2026-07-16T00:00:02Z".to_string(),
        metadata_json: None,
    };
    let partial_events = vec![
        events[0].clone(),
        EventRow {
            event_id: "evt.turn.partial-assistant.sqlite".to_string(),
            session_id: Some(session.session_id.clone()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some("role=assistant".to_string()),
            created_at: "2026-07-16T00:00:02Z".to_string(),
        },
        events[1].clone(),
    ];
    assert!(matches!(
        db.append_message_turn_with_events(&[message.clone(), assistant.clone()], &partial_events,),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert!(db
        .load_messages(&session.session_id, &MessageQuery::default())
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

    let mut conflicting_events = events.clone();
    conflicting_events[1].payload = Some("conflict".to_string());
    assert!(matches!(
        db.append_message_turn_with_events(std::slice::from_ref(&message), &conflicting_events,),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    let late_message = MessageRow {
        message_id: "msg.turn.late.sqlite".to_string(),
        ..message
    };
    let mut late_events = events;
    late_events[0].event_id = "evt.turn.late-message.sqlite".to_string();
    late_events[1].event_id = "evt.turn.late-completed.sqlite".to_string();
    assert!(matches!(
        db.append_message_turn_with_events(&[late_message], &late_events),
        Err(DatabaseError::ConstraintViolation(_))
    ));
    assert_eq!(db.message_count(&session.session_id).expect("count"), 1);
    assert_eq!(
        db.load_events(&session.session_id, &EventQuery::default())
            .expect("events")
            .len(),
        2
    );
}

#[test]
fn sqlite_concurrent_startup_migration_is_idempotent() {
    let path = std::env::temp_dir().join(format!(
        "sdkwork-agent-runtime-migration-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let path_text = path.to_string_lossy().into_owned();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let barrier = barrier.clone();
        let path_text = path_text.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            SqliteDatabase::open_migrated(&path_text)
        }));
    }
    for handle in handles {
        handle
            .join()
            .expect("migration thread")
            .expect("concurrent migration");
    }
    SqliteDatabase::open_migrated(&path_text).expect("repeat migration");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

#[test]
fn sqlite_retention_is_bounded_transactional_and_preserves_live_work() {
    let db = migrated_sqlite();
    let mut expired_session = scoped_session("session.retention.closed", "tenant.a", "user.a");
    expired_session.state = "closed".to_string();
    expired_session.created_at = "2020-01-01T00:00:00Z".to_string();
    expired_session.updated_at = Some("2020-01-02T00:00:00Z".to_string());
    expired_session.message_count = 1;
    db.save_session(&expired_session).expect("expired session");

    let mut live_session = scoped_session("session.retention.live", "tenant.a", "user.a");
    live_session.created_at = "2020-01-01T00:00:00Z".to_string();
    live_session.updated_at = Some("2026-01-01T00:00:00Z".to_string());
    live_session.message_count = 1;
    db.save_session(&live_session).expect("live session");

    for (message_id, session_id) in [
        (
            "message.retention.closed",
            expired_session.session_id.as_str(),
        ),
        ("message.retention.live", live_session.session_id.as_str()),
    ] {
        db.save_message(&MessageRow {
            message_id: message_id.to_string(),
            session_id: session_id.to_string(),
            role: "user".to_string(),
            content: "expired".to_string(),
            created_at: "2020-01-01T00:00:01Z".to_string(),
            metadata_json: None,
        })
        .expect("message");
        db.save_event(&EventRow {
            event_id: format!("event.{message_id}"),
            session_id: Some(session_id.to_string()),
            event_type: "retention.test".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2020-01-01T00:00:01Z".to_string(),
        })
        .expect("event");
    }

    for (task_id, session_id, state) in [
        (
            "task.retention.closed",
            expired_session.session_id.as_str(),
            "completed",
        ),
        (
            "task.retention.live",
            live_session.session_id.as_str(),
            "completed",
        ),
        (
            "task.retention.pending",
            live_session.session_id.as_str(),
            "pending",
        ),
    ] {
        db.save_task(&sdkwork_agent_database::TaskRow {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            instruction: "retention".to_string(),
            state: state.to_string(),
            created_at: "2020-01-01T00:00:01Z".to_string(),
            updated_at: None,
        })
        .expect("task");
    }

    for (permission_id, session_id, status) in [
        (
            "permission.retention.closed",
            expired_session.session_id.as_str(),
            "pending",
        ),
        (
            "permission.retention.live",
            live_session.session_id.as_str(),
            "denied",
        ),
        (
            "permission.retention.pending",
            live_session.session_id.as_str(),
            "pending",
        ),
    ] {
        db.save_permission(&PermissionRow {
            permission_request_id: permission_id.to_string(),
            session_id: Some(session_id.to_string()),
            category: "host.filesystem.read".to_string(),
            resource: "/workspace".to_string(),
            side_effect_level: "read_only".to_string(),
            reason: "retention".to_string(),
            status: status.to_string(),
            owner_tenant_id: Some("tenant.a".to_string()),
            owner_user_ref: Some("user.a".to_string()),
            created_at: "2020-01-01T00:00:01Z".to_string(),
            updated_at: None,
        })
        .expect("permission");
    }
    db.save_permission(&PermissionRow {
        permission_request_id: "permission.retention.global.pending".to_string(),
        session_id: None,
        category: "host.filesystem.read".to_string(),
        resource: "/workspace".to_string(),
        side_effect_level: "read_only".to_string(),
        reason: "retention".to_string(),
        status: "pending".to_string(),
        owner_tenant_id: Some("tenant.a".to_string()),
        owner_user_ref: Some("user.a".to_string()),
        created_at: "2020-01-01T00:00:01Z".to_string(),
        updated_at: None,
    })
    .expect("global pending permission");

    let counts = db
        .purge_expired("2021-01-01T00:00:00Z", 100)
        .expect("retention purge");
    assert_eq!(counts.sessions, 1);
    assert_eq!(counts.messages, 2);
    assert_eq!(counts.tasks, 2);
    assert_eq!(counts.events, 2);
    assert_eq!(counts.permissions, 2);
    assert!(db
        .load_session(&expired_session.session_id)
        .expect("expired lookup")
        .is_none());
    assert_eq!(
        db.load_session(&live_session.session_id)
            .expect("live lookup")
            .expect("live session")
            .message_count,
        0
    );
    assert!(db
        .load_task("task.retention.pending")
        .expect("pending task")
        .is_some());
    assert!(db
        .load_permission("permission.retention.pending")
        .expect("pending permission")
        .is_some());
    assert!(db
        .load_permission("permission.retention.global.pending")
        .expect("global pending permission")
        .is_some());

    let schema = db.schema_status().expect("schema status");
    assert!(schema.drift_free);
    assert_eq!(schema.version, schema.expected_version);
    db.run_maintenance().expect("sqlite maintenance");
}

#[test]
fn sqlite_task_cancellation_fences_claimed_permission_operation() {
    use sdkwork_agent_database::{
        ActionKind, PermissionOperationRepository, PermissionOperationRow, PermissionOperationState,
        PermissionPayloadKind, RunRow, RunState, RuntimeExecutionRepository, StepRow, StepState,
    };

    let db = migrated_sqlite();
    let session_id = "session.permission.cancel.sqlite";
    let task_id = "task.permission.cancel.sqlite";
    let run_id = "run.permission.cancel.sqlite";
    let step_id = "step.permission.cancel.sqlite";
    let permission_id = "permission.cancel.sqlite";
    db.save_session(&scoped_session(session_id, "tenant.1", "user.1"))
        .expect("session");

    let now = "2026-07-17T00:00:00Z";
    let permission = PermissionRow {
        permission_request_id: permission_id.to_string(),
        session_id: Some(session_id.to_string()),
        category: "tool.invoke".to_string(),
        resource: "tool.protected".to_string(),
        side_effect_level: "side_effectful".to_string(),
        reason: "approval required".to_string(),
        status: "pending".to_string(),
        owner_tenant_id: Some("tenant.1".to_string()),
        owner_user_ref: Some("user.1".to_string()),
        created_at: now.to_string(),
        updated_at: None,
    };
    let task = TaskRow {
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        instruction: "execute approved tool".to_string(),
        state: "accepted".to_string(),
        created_at: now.to_string(),
        updated_at: Some(now.to_string()),
    };
    let run = RunRow {
        run_id: run_id.to_string(),
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
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
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    let step = StepRow {
        step_id: step_id.to_string(),
        run_id: run_id.to_string(),
        sequence_no: 0,
        action_kind: ActionKind::ToolCall,
        state: StepState::AwaitingPermission,
        provider_id: Some("provider.tool".to_string()),
        descriptor_revision: Some("1.0.0".to_string()),
        policy_revision: Some("1.0.0".to_string()),
        causation_step_id: None,
        idempotency_key_hash: None,
        result_json: None,
        error_kind: None,
        error_code: None,
        error_detail: None,
        started_at: None,
        finished_at: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    let operation = PermissionOperationRow {
        permission_request_id: permission_id.to_string(),
        run_id: run_id.to_string(),
        step_id: step_id.to_string(),
        tool_call_id: "tool-call.cancel.sqlite".to_string(),
        provider_id: "provider.tool".to_string(),
        descriptor_revision: "1.0.0".to_string(),
        policy_revision: "1.0.0".to_string(),
        payload_kind: PermissionPayloadKind::Ciphertext,
        payload_ref: "ciphertext".to_string(),
        payload_digest: "digest".to_string(),
        encryption_key_id: Some("key.v1".to_string()),
        state: PermissionOperationState::Pending,
        expires_at: "2099-01-01T00:00:00Z".to_string(),
        lease_owner: None,
        lease_expires_at: None,
        fencing_token: 0,
        result_json: None,
        error_kind: None,
        error_code: None,
        error_detail: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    let requested = EventRow {
        event_id: "event.permission.requested.cancel.sqlite".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: "permission.requested".to_string(),
        severity: "warn".to_string(),
        payload: None,
        created_at: now.to_string(),
    };
    db.create_permission_execution(&permission, &task, &run, &step, &operation, &requested)
        .expect("permission execution");
    db.decide_permission_operation(
        permission_id,
        "allow",
        now,
        &EventRow {
            event_id: "event.permission.allowed.cancel.sqlite".to_string(),
            event_type: "permission.allowed".to_string(),
            ..requested.clone()
        },
    )
    .expect("allow");

    let claim = db
        .claim_permission_operation("worker.one", now, "2099-01-01T00:00:00Z")
        .expect("claim")
        .expect("one claim");

    let cancelled = db
        .request_task_cancellation(
            task_id,
            "2026-07-17T00:01:00Z",
            &EventRow {
                event_id: "event.task.cancelled.cancel.sqlite".to_string(),
                session_id: Some(session_id.to_string()),
                event_type: "task.cancelled".to_string(),
                severity: "info".to_string(),
                payload: None,
                created_at: "2026-07-17T00:01:00Z".to_string(),
            },
        )
        .expect("cancel")
        .0;
    assert_eq!(cancelled.state, "cancelled");

    // The stale claim must be fenced out: its fencing token no longer matches.
    let fenced = db.complete_permission_operation(
        &claim,
        r#"{"output":"resurrected"}"#,
        "2026-07-17T00:02:00Z",
        &EventRow {
            event_id: "event.permission.completed.cancel.sqlite".to_string(),
            session_id: Some(session_id.to_string()),
            event_type: "permission.operation.completed".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-07-17T00:02:00Z".to_string(),
        },
    );
    assert!(
        matches!(fenced, Err(DatabaseError::ConstraintViolation(_))),
        "stale permission claim must be fenced out after task cancellation, got {fenced:?}"
    );

    // The task must stay cancelled and the operation must be cancelled with a
    // crypto-erased payload.
    let stored_task = db.load_task(task_id).expect("load task").expect("task");
    assert_eq!(stored_task.state, "cancelled");
    let stored = db
        .load_permission_operation(permission_id)
        .expect("load")
        .expect("operation");
    assert_eq!(stored.state, PermissionOperationState::Cancelled);
    assert!(stored.payload_ref.is_empty());
    assert!(stored.payload_digest.is_empty());
    assert!(stored.encryption_key_id.is_none());
    let _ = db.delete_session_cascade(session_id);
}

#[test]
fn sqlite_request_task_cancellation_rejects_failed_task() {
    use sdkwork_agent_database::RuntimeExecutionRepository;

    let db = migrated_sqlite();
    let session_id = "session.cancel.failed.sqlite";
    let task_id = "task.cancel.failed.sqlite";
    db.save_session(&scoped_session(session_id, "tenant.1", "user.1"))
        .expect("session");
    db.save_task(&TaskRow {
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        instruction: "already failed".to_string(),
        state: "failed".to_string(),
        created_at: "2026-07-17T00:00:00Z".to_string(),
        updated_at: Some("2026-07-17T00:01:00Z".to_string()),
    })
    .expect("task");
    let result = db.request_task_cancellation(
        task_id,
        "2026-07-17T00:02:00Z",
        &EventRow {
            event_id: "event.task.cancelled.failed.sqlite".to_string(),
            session_id: Some(session_id.to_string()),
            event_type: "task.cancelled".to_string(),
            severity: "info".to_string(),
            payload: None,
            created_at: "2026-07-17T00:02:00Z".to_string(),
        },
    );
    assert!(

        matches!(result, Err(DatabaseError::ConstraintViolation(_))),
        "failed task must reject cancellation, got {result:?}"
    );
    let stored = db.load_task(task_id).expect("load").expect("task");
    assert_eq!(stored.state, "failed");
}
