use crate::common::*;
use api::chatbot::identity::{
    self, ChatSender, PAIRING_RATE_LIMIT, ROLE_ADMIN, ROLE_CONTRIBUTOR, ROLE_VIEWER, Resolution,
};
use api::repositories::chat_identities;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TRANSPORT: &str = "google_chat";
const DISCORD: &str = "discord";

// ============================================================================
// Fixtures
// ============================================================================

struct Bound {
    identity_id: Uuid,
    user_id: Uuid,
}

async fn bind(
    server: &TestServer,
    email: &str,
    role: &str,
    address: Option<&str>,
    external_id: Option<&str>,
    max_role: &str,
) -> Bound {
    bind_on(
        server,
        TRANSPORT,
        email,
        role,
        address,
        external_id,
        max_role,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn bind_on(
    server: &TestServer,
    transport: &str,
    email: &str,
    role: &str,
    address: Option<&str>,
    external_id: Option<&str>,
    max_role: &str,
) -> Bound {
    let user_id = create_user_with_role(server, email, role).await;
    let row = chat_identities::create(
        &server.pool,
        &chat_identities::NewChatIdentity {
            transport,
            address,
            external_id,
            user_id,
            max_role,
            enabled: true,
        },
    )
    .await
    .expect("identity created");
    Bound {
        identity_id: row.id,
        user_id,
    }
}

fn sender<'a>(external_id: Option<&'a str>, email: Option<&'a str>) -> ChatSender<'a> {
    ChatSender {
        transport: TRANSPORT,
        external_id,
        email,
        display_name: Some("Her"),
        space: Some("spaces/AAA"),
        thread: Some("spaces/AAA/threads/T1"),
    }
}

/// A Discord sender: a numeric user id and, always, no email.
fn discord_sender(external_id: &str) -> ChatSender<'_> {
    ChatSender {
        transport: DISCORD,
        external_id: Some(external_id),
        email: None,
        display_name: Some("Her"),
        space: Some("1122334455"),
        thread: None,
    }
}

fn principal(resolution: Resolution) -> api::chatbot::identity::ChatPrincipal {
    match resolution {
        Resolution::Principal(p) => p,
        other => panic!("expected a principal, got {other:?}"),
    }
}

// ============================================================================
// Resolver
// ============================================================================

#[tokio::test]
async fn resolves_by_address_case_insensitively_and_learns_the_external_id() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("Her@Example.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let resolved = identity::resolve(
        &server.pool,
        &sender(Some("users/1234"), Some("HER@example.COM")),
    )
    .await
    .expect("resolve succeeds");
    let p = principal(resolved);
    assert_eq!(p.identity_id, bound.identity_id);
    assert_eq!(p.auth.user_id, bound.user_id);
    assert_eq!(p.auth.role, ROLE_CONTRIBUTOR);

    let row = chat_identities::get(&server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert_eq!(
        row.external_id.as_deref(),
        Some("users/1234"),
        "first contact must learn the transport's stable id"
    );
    assert!(
        row.last_seen_at.is_some(),
        "last_seen_at is how an admin tells a working binding from a typo"
    );
}

#[tokio::test]
async fn resolves_by_external_id_when_the_address_has_changed() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("old@example.com"),
        Some("users/1234"),
        ROLE_CONTRIBUTOR,
    )
    .await;

    let resolved = identity::resolve(
        &server.pool,
        &sender(Some("users/1234"), Some("brand-new@example.com")),
    )
    .await
    .expect("resolve succeeds");
    assert_eq!(principal(resolved).identity_id, bound.identity_id);
}

/// Discord never sends an email, so an admin binding someone ahead of time has nothing
/// to type but the numeric user id. Before this resolved, the binding looked correct on
/// the settings page and the bot still answered "you're not set up to use this yet".
#[tokio::test]
async fn a_discord_binding_by_user_id_resolves_and_learns_it() {
    let server = TestServer::start_api_only().await;
    let bound = bind_on(
        &server,
        DISCORD,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("461291837402193920"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let resolved = identity::resolve(&server.pool, &discord_sender("461291837402193920"))
        .await
        .expect("resolve succeeds");
    let p = principal(resolved);
    assert_eq!(p.identity_id, bound.identity_id);
    assert_eq!(p.auth.user_id, bound.user_id);

    let row = chat_identities::get(&server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert_eq!(
        row.external_id.as_deref(),
        Some("461291837402193920"),
        "the id typed as an address must be promoted to the stable key on first contact"
    );
}

/// The looser address match must not reach across transports: two people can hold the
/// same-looking handle on Discord and Google Chat and they are not the same binding.
#[tokio::test]
async fn an_id_typed_as_an_address_does_not_match_another_transport() {
    let server = TestServer::start_api_only().await;
    bind_on(
        &server,
        DISCORD,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("461291837402193920"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let resolved = identity::resolve(&server.pool, &sender(Some("461291837402193920"), None))
        .await
        .expect("resolve succeeds");
    assert!(
        !matches!(resolved, Resolution::Principal(_)),
        "a google_chat sender must not bind to a discord identity"
    );
}

#[tokio::test]
async fn a_learned_external_id_is_not_overwritten() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("her@example.com"),
        Some("users/original"),
        ROLE_CONTRIBUTOR,
    )
    .await;

    identity::resolve(
        &server.pool,
        &sender(Some("users/impostor"), Some("her@example.com")),
    )
    .await
    .expect("resolve succeeds");

    let row = chat_identities::get(&server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert_eq!(row.external_id.as_deref(), Some("users/original"));
}

#[tokio::test]
async fn effective_role_is_the_lower_of_the_user_and_the_ceiling() {
    let server = TestServer::start_api_only().await;

    let cases = [
        (ROLE_VIEWER, ROLE_VIEWER, ROLE_VIEWER),
        (ROLE_VIEWER, ROLE_CONTRIBUTOR, ROLE_VIEWER),
        (ROLE_VIEWER, ROLE_ADMIN, ROLE_VIEWER),
        (ROLE_CONTRIBUTOR, ROLE_VIEWER, ROLE_VIEWER),
        (ROLE_CONTRIBUTOR, ROLE_CONTRIBUTOR, ROLE_CONTRIBUTOR),
        (ROLE_CONTRIBUTOR, ROLE_ADMIN, ROLE_CONTRIBUTOR),
        (ROLE_ADMIN, ROLE_VIEWER, ROLE_VIEWER),
        (ROLE_ADMIN, ROLE_CONTRIBUTOR, ROLE_CONTRIBUTOR),
        (ROLE_ADMIN, ROLE_ADMIN, ROLE_ADMIN),
    ];

    for (i, (user_role, max_role, expected)) in cases.into_iter().enumerate() {
        let address = format!("case{i}@example.com");
        bind(
            &server,
            &format!("case{i}@test.com"),
            user_role,
            Some(&address),
            None,
            max_role,
        )
        .await;

        let resolved = identity::resolve(&server.pool, &sender(None, Some(&address)))
            .await
            .expect("resolve succeeds");
        assert_eq!(
            principal(resolved).auth.role,
            expected,
            "user={user_role} max={max_role}"
        );
    }
}

#[tokio::test]
async fn demoting_in_the_users_page_lowers_the_chat_role_immediately() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_ADMIN,
        Some("her@example.com"),
        None,
        ROLE_ADMIN,
    )
    .await;

    let before = principal(
        identity::resolve(&server.pool, &sender(None, Some("her@example.com")))
            .await
            .expect("resolve succeeds"),
    );
    assert_eq!(before.auth.role, ROLE_ADMIN);

    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", bound.user_id),
            &json!({ "role": "viewer" }),
        )
        .await
        .assert_ok();

    let after = principal(
        identity::resolve(&server.pool, &sender(None, Some("her@example.com")))
            .await
            .expect("resolve succeeds"),
    );
    assert_eq!(
        after.auth.role, ROLE_VIEWER,
        "users.role is the only source of truth and is joined at request time"
    );
}

#[tokio::test]
async fn an_unmapped_sender_with_an_email_is_refused_without_a_pairing_code() {
    let server = TestServer::start_api_only().await;

    let resolved = identity::resolve(
        &server.pool,
        &sender(Some("users/stranger"), Some("stranger@example.com")),
    )
    .await
    .expect("resolve succeeds");
    assert!(
        matches!(resolved, Resolution::Unknown),
        "an admin can bind an address they can see; no code needed"
    );

    let pending = chat_identities::list_pending_pairing_codes(&server.pool)
        .await
        .expect("list");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn a_disabled_binding_is_refused_and_never_pairs() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("her@example.com"),
        Some("users/1234"),
        ROLE_CONTRIBUTOR,
    )
    .await;
    chat_identities::update(
        &server.pool,
        bound.identity_id,
        &chat_identities::ChatIdentityPatch {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .expect("disabled");

    let resolved = identity::resolve(&server.pool, &sender(Some("users/1234"), None))
        .await
        .expect("resolve succeeds");
    assert!(matches!(resolved, Resolution::Disabled));
    assert!(
        chat_identities::list_pending_pairing_codes(&server.pool)
            .await
            .expect("list")
            .is_empty()
    );
}

// ============================================================================
// Pairing
// ============================================================================

fn challenge(resolution: Resolution) -> identity::PairingChallenge {
    match resolution {
        Resolution::Pairing(c) => c,
        other => panic!("expected a pairing challenge, got {other:?}"),
    }
}

#[tokio::test]
async fn a_sender_with_no_email_gets_a_code_and_the_same_one_on_a_retry() {
    let server = TestServer::start_api_only().await;

    let first = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    assert!(!first.reissued);
    assert_eq!(first.code.len(), 6);

    let second = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    assert!(second.reissued);
    assert_eq!(
        second.code, first.code,
        "a live code is reused so one stranger cannot stack up approvals"
    );

    let pending = chat_identities::list_pending_pairing_codes(&server.pool)
        .await
        .expect("list");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].external_id, "users/gmail");
    assert_eq!(pending[0].display_name.as_deref(), Some("Her"));
}

#[tokio::test]
async fn repeated_unknown_senders_are_rate_limited_into_silence() {
    let server = TestServer::start_api_only().await;

    for attempt in 0..PAIRING_RATE_LIMIT {
        let issued = challenge(
            identity::resolve(&server.pool, &sender(Some("users/spam"), None))
                .await
                .expect("resolve succeeds"),
        );
        // Expire it so the next message mints a new one rather than reusing this.
        expire_code(&server, &issued.code).await;
        assert!(!issued.reissued, "attempt {attempt} should mint a new code");
    }

    let resolved = identity::resolve(&server.pool, &sender(Some("users/spam"), None))
        .await
        .expect("resolve succeeds");
    assert!(
        matches!(resolved, Resolution::Silent),
        "past the allowance the bot says nothing at all"
    );
}

async fn expire_code(server: &TestServer, code: &str) {
    sqlx::query(
        "UPDATE chat_pairing_codes SET expires_at = NOW() - INTERVAL '1 minute' WHERE code = $1",
    )
    .bind(code)
    .execute(&server.pool)
    .await
    .expect("expired");
}

#[tokio::test]
async fn an_expired_code_cannot_be_approved() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;

    let issued = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    let pairing_id = pending_id(&server, &issued.code).await;
    expire_code(&server, &issued.code).await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/chat/pairings/{pairing_id}/approve"),
            &json!({ "user_id": user_id }),
        )
        .await
        .assert_status(StatusCode::GONE);
}

async fn pending_id(server: &TestServer, code: &str) -> Uuid {
    let pending = chat_identities::list_pending_pairing_codes(&server.pool)
        .await
        .expect("list");
    pending
        .into_iter()
        .find(|p| p.code == code)
        .expect("code is pending")
        .id
}

#[tokio::test]
async fn approving_a_code_binds_the_sender_and_the_next_message_resolves() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;

    let issued = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    let pairing_id = pending_id(&server, &issued.code).await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/chat/pairings/{pairing_id}/approve"),
            &json!({ "user_id": user_id }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let resolved = identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
        .await
        .expect("resolve succeeds");
    let p = principal(resolved);
    assert_eq!(p.auth.user_id, user_id);
    assert_eq!(p.auth.role, ROLE_CONTRIBUTOR);

    assert!(
        chat_identities::list_pending_pairing_codes(&server.pool)
            .await
            .expect("list")
            .is_empty(),
        "an approved code stops being pending"
    );
}

#[tokio::test]
async fn a_code_is_single_use() {
    let server = TestServer::start_api_only().await;
    let first_user = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;
    let second_user = create_user_with_role(&server, "him@test.com", ROLE_CONTRIBUTOR).await;

    let issued = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    let pairing_id = pending_id(&server, &issued.code).await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/chat/pairings/{pairing_id}/approve"),
            &json!({ "user_id": first_user }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/chat/pairings/{pairing_id}/approve"),
            &json!({ "user_id": second_user }),
        )
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn approving_onto_an_existing_pending_row_writes_its_external_id() {
    let server = TestServer::start_api_only().await;
    let bound = bind(
        &server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("her@gmail.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let issued = challenge(
        identity::resolve(&server.pool, &sender(Some("users/gmail"), None))
            .await
            .expect("resolve succeeds"),
    );
    let pairing_id = pending_id(&server, &issued.code).await;

    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/chat/pairings/{pairing_id}/approve"),
            &json!({ "identity_id": bound.identity_id }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let row = chat_identities::get(&server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert_eq!(row.external_id.as_deref(), Some("users/gmail"));
    assert_eq!(row.address.as_deref(), Some("her@gmail.com"));
}

// ============================================================================
// Admin CRUD
// ============================================================================

#[tokio::test]
async fn admin_crud_round_trip() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_ADMIN).await;
    let admin = server.admin_client();

    let created = admin
        .post_json(
            "/api/v1/admin/chat/identities",
            &json!({
                "transport": TRANSPORT,
                "user_id": user_id,
                "address": "  Her@Example.com  ",
            }),
        )
        .await;
    created.assert_status(StatusCode::CREATED);
    let identity_id: Uuid = created.json();

    let listed: Value = admin.get("/api/v1/admin/chat/identities").await.json();
    let row = &listed["identities"][0];
    assert_eq!(row["address"], "Her@Example.com", "the address is trimmed");
    assert_eq!(
        row["max_role"], ROLE_CONTRIBUTOR,
        "the ceiling defaults low"
    );
    assert_eq!(
        row["effective_role"], ROLE_CONTRIBUTOR,
        "an admin user is capped by the default ceiling"
    );
    assert_eq!(row["status"], "awaiting_first_message");
    assert!(row["last_seen_at"].is_null());
    assert!(
        listed["transports"]
            .as_array()
            .expect("transports listed")
            .iter()
            .any(|t| t["id"] == TRANSPORT),
        "the settings page needs the config schema"
    );

    admin
        .patch_json(
            &format!("/api/v1/admin/chat/identities/{identity_id}"),
            &json!({ "max_role": ROLE_ADMIN, "enabled": false }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let listed: Value = admin.get("/api/v1/admin/chat/identities").await.json();
    assert_eq!(listed["identities"][0]["effective_role"], ROLE_ADMIN);
    assert_eq!(listed["identities"][0]["status"], "disabled");

    admin
        .delete(&format!("/api/v1/admin/chat/identities/{identity_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let listed: Value = admin.get("/api/v1/admin/chat/identities").await.json();
    assert!(listed["identities"].as_array().expect("array").is_empty());
}

#[tokio::test]
async fn admin_crud_validation() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;
    let admin = server.admin_client();

    let cases: [(Value, StatusCode); 4] = [
        (
            json!({ "transport": "carrier_pigeon", "user_id": user_id, "address": "a@b.com" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "transport": TRANSPORT, "user_id": user_id }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "transport": TRANSPORT, "user_id": user_id, "address": "a@b.com", "max_role": "wizard" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "transport": TRANSPORT, "user_id": Uuid::now_v7(), "address": "a@b.com" }),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, expected) in cases {
        admin
            .post_json("/api/v1/admin/chat/identities", &body)
            .await
            .assert_status(expected);
    }

    admin
        .post_json(
            "/api/v1/admin/chat/identities",
            &json!({ "transport": TRANSPORT, "user_id": user_id, "address": "dup@b.com" }),
        )
        .await
        .assert_status(StatusCode::CREATED);
    admin
        .post_json(
            "/api/v1/admin/chat/identities",
            &json!({ "transport": TRANSPORT, "user_id": user_id, "address": "DUP@b.com" }),
        )
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn chat_identity_routes_are_admin_only() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;
    // Role checks run inside the handler, so a malformed body 422s before the 403.
    // Every body below is valid.
    let valid = json!({ "transport": TRANSPORT, "user_id": user_id, "address": "a@b.com" });

    let anon = TestClient::new(&server.api_url);
    anon.get("/api/v1/admin/chat/identities")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.post_json("/api/v1/admin/chat/identities", &valid)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    for role in [ROLE_VIEWER, ROLE_CONTRIBUTOR] {
        let client = create_user_client(&server, &format!("{role}@denied.test"), role).await;
        client
            .get("/api/v1/admin/chat/identities")
            .await
            .assert_status(StatusCode::FORBIDDEN);
        client
            .post_json("/api/v1/admin/chat/identities", &valid)
            .await
            .assert_status(StatusCode::FORBIDDEN);
        client
            .patch_json(
                &format!("/api/v1/admin/chat/identities/{}", Uuid::now_v7()),
                &json!({ "enabled": false }),
            )
            .await
            .assert_status(StatusCode::FORBIDDEN);
        client
            .delete(&format!("/api/v1/admin/chat/identities/{}", Uuid::now_v7()))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        client
            .post_json(
                &format!("/api/v1/admin/chat/pairings/{}/approve", Uuid::now_v7()),
                &json!({ "user_id": user_id }),
            )
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
}

// ============================================================================
// Webhook — configuration and unauthenticated rejections
// ============================================================================

#[tokio::test]
async fn webhook_rejects_what_it_cannot_authenticate() {
    let bare = TestServer::start_api_only().await;
    let anon = TestClient::new(&bare.api_url);

    anon.post_json(
        "/api/v1/chat/carrier_pigeon/webhook",
        &json!({ "type": "MESSAGE" }),
    )
    .await
    .assert_status(StatusCode::NOT_FOUND);

    // No Google Chat agent configured yet: the transport has no credentials at all.
    anon.post_json(
        "/api/v1/chat/google_chat/webhook",
        &json!({ "type": "MESSAGE" }),
    )
    .await
    .assert_status(StatusCode::SERVICE_UNAVAILABLE);

    let fixture = webhook_fixture().await;
    let configured = TestClient::new(&fixture.server.api_url);

    configured
        .post_json(
            "/api/v1/chat/google_chat/webhook",
            &json!({ "type": "MESSAGE" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    TestClient::new(&fixture.server.api_url)
        .with_auth("not-a-jwt")
        .post_json(
            "/api/v1/chat/google_chat/webhook",
            &json!({ "type": "MESSAGE" }),
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// ============================================================================
// Attribution
// ============================================================================

#[tokio::test]
async fn record_activity_stores_the_user_when_it_has_one() {
    let server = TestServer::start_api_only().await;
    let user_id = create_user_with_role(&server, "her@test.com", ROLE_CONTRIBUTOR).await;

    api::routes::library::helpers::record_activity(
        &server.pool,
        "tested",
        None,
        None,
        "with a user",
        None,
        None,
        None,
        Some(user_id),
    )
    .await;
    api::routes::library::helpers::record_activity(
        &server.pool,
        "tested",
        None,
        None,
        "without a user",
        None,
        None,
        None,
        None,
    )
    .await;

    let rows: Vec<(String, Option<Uuid>)> =
        sqlx::query_as("SELECT title, user_id FROM activity_log ORDER BY title")
            .fetch_all(&server.pool)
            .await
            .expect("query");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("with a user".to_string(), Some(user_id)));
    assert_eq!(rows[1], ("without a user".to_string(), None));
}

#[tokio::test]
async fn wanting_a_book_attributes_the_attempt_to_the_caller() {
    let server = TestServer::start_api_only().await;
    let user = create_user_client(&server, "her@test.com", ROLE_CONTRIBUTOR).await;
    let user_id = create_user_with_role(&server, "second@test.com", ROLE_CONTRIBUTOR).await;

    let created = user
        .post_json(
            "/api/v1/admin/wanted",
            &json!({ "title": "Attribution Test", "author": "A. Nother" }),
        )
        .await;
    created.assert_ok();
    let book_id = created.json::<Value>()["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("book id");

    let attempt_id = api::repositories::wanted_attempts::create(
        &server.pool,
        api::repositories::wanted_attempts::WantedAttemptInput {
            book_id: Some(book_id),
            source: "indexer".to_string(),
            status: "downloading".to_string(),
            user_id: Some(user_id),
            ..Default::default()
        },
    )
    .await
    .expect("attempt created");

    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM wanted_attempts WHERE id = $1")
            .bind(attempt_id)
            .fetch_one(&server.pool)
            .await
            .expect("query");
    assert_eq!(stored, Some(user_id));

    let requester = api::repositories::wanted_attempts::latest_requester(&server.pool, book_id)
        .await
        .expect("query");
    assert_eq!(
        requester,
        Some(user_id),
        "this is how 'your download finished' finds the person who asked"
    );
}

#[tokio::test]
async fn queueing_a_release_records_who_asked() {
    let kobload = MockServer::start().await;
    let download_id = Uuid::now_v7();
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": download_id,
            "status": "queued"
        })))
        .mount(&kobload)
        .await;

    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": kobload.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": "http://localhost:6766/api/v1/ingest/kobload-token"
            }),
        )
        .await
        .assert_ok();

    let her = create_user_client(&server, "her@test.com", ROLE_CONTRIBUTOR).await;
    let created = her
        .post_json(
            "/api/v1/admin/wanted",
            &json!({ "title": "Queue Test", "author": "A. Nother" }),
        )
        .await;
    created.assert_ok();
    let book_id = created.json::<Value>()["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("book id");

    her.post_json(
        &format!("/api/v1/admin/books/{book_id}/queue"),
        &json!({
            "source": "manual",
            "release": {
                "protocol": "torrent",
                "download_url": "https://example.com/download",
                "title": "Queue Test"
            }
        }),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);

    let her_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind("her@test.com")
        .fetch_one(&server.pool)
        .await
        .expect("query");
    let attributed: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM wanted_attempts WHERE book_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(book_id)
    .fetch_one(&server.pool)
    .await
    .expect("query");
    assert_eq!(attributed, Some(her_id));

    assert_eq!(
        api::repositories::wanted_attempts::latest_requester(&server.pool, book_id)
            .await
            .expect("query"),
        Some(her_id)
    );
}

// ============================================================================
// Webhook — end to end over a real signed token
//
// The route is unauthenticated and its whole security is the JWT, so the accept
// path needs proving, not just the rejections. These stand up a JWK endpoint for a
// keypair we control and point the transport at it.
// ============================================================================

const KID: &str = "kobbler-test-kid";
const PROJECT_NUMBER: &str = "123456789012";
const CHAT_ISSUER: &str = "chat@system.gserviceaccount.com";

const SIGNING_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC+VIMKjBmJC9eE\n\
GWEX70VfliFqUyuxxQVFBio1PbDGYh00XTqKFaHdOkns97IhPamp5hslSAMFW+FH\n\
zfkPb+Li2H0IY59UITMCVqGze6lxGVOePyM/o+xOiuYqqOcz3uzCF9M7sO1coXR2\n\
QbY17+hEJ/QBBkDt/U7Z2BewE5+ZIMMntiUTd1/ViFLW+YS9vUhvnTRvZxzcjTwZ\n\
JLxbvDLmKC44mRe6nhYy5Z3RmQFollo9cl1+LaEcnatSoQxu3V/z4gWPEDS+JB25\n\
qIlDVKgzbW/78t4q3fpdnW/5xufw1AhG3C6XTECPa7s0DoXFiIhDOrGJ/eWb+aon\n\
KW6tf/LnAgMBAAECggEACUm6LAQX5Opehd7v2t6oIy4s4EbsbXGWEb8SGo9cS+S9\n\
fxuDlU8mmyin3eKyidERXANTf3dq/6fIWnGF6MfG1IIdtpCXVbWEYJCrYG+1nkQk\n\
TVHQcwpuqDXlXd89381OVCRx1Z+13ZzawECcVzfSElgOyL6uNs7zOknuJ1S10kpD\n\
dpVbU+mvqe873dCgpuJdxtq1VGc2TUCp8njiryHbUlnnUKoZrQGj1nVxG4tmuI3u\n\
sxoW4fawxFe5ulxK4XKcw61MWwMOuSCeLEG+uk2Ouq16+ckI41iTr4Xk/zlvdGHR\n\
xwIlkAKECdNDpvqpdu8roDct2lhrTzZNBGwnntI+ZQKBgQD5sRh4X3ZPpS5ngRe9\n\
11YpLKBp08HWAlD7UFyYWn/78nRuar3NCU6hc1DN48H/DX/R6SosyvPc6S3V44NT\n\
H/LL5fDbxzPQ165jD8V9nMpFtLwOVj9jthSepU1JDUXgFpbmwlTHa+eyIol69WQX\n\
qcATs+ESJ4AQdwK92k78+5AeUwKBgQDDI31GyH6pkuHu4VlXaJUmIMF2dSB6RXx5\n\
T/4eevERkTbgspFXmR/lBKN0jsxNRNmEauYD5Zhm9UuROmQUBWGklmN3UgAV0GSu\n\
mZ4S6uPmboHBVYwztIhU8ramL+9K+ONERXjh4W//wZ1i+oIa1vx0IPuChreZl9W5\n\
xf0q0P3+nQKBgEY5iKhSMPQ1gNebdxHtfk4WPweEhv+ppx0HfQNccKJrjlN/seH4\n\
vCSsSg2eAPG62jgvW609Z1i6PCnr0wON+Ltf8F3fqLMT9ffmd6TcdAcmzb06hJmG\n\
nEYCN7pUAVMFB2vfO/jaYE+5ZvEuEviUxXDFJof0lBANaLf8rjp4hhABAoGBAJV+\n\
+trffyoqfSylN+zfEmTDc3NtCBSkZ1gJEIYJaeYref9qzi5uASFTDGaff6UyufHY\n\
c59Iphk6k0oPkdxEwyl0jX88MxSDMp8Q0SltCmve/UlAYMi75jYuO//VeV3Wj0Ye\n\
cx2ZuFQX4JuTj1lAmufc3NKCouXojLRmGi5TLkOBAoGAXys4sOTkF9zMvE8D9y+8\n\
iqVGbREljxrP48axb+grbJBv4GKvDWEkuK/pxp810YvAKbrhZBJv8rDRCTS/gegd\n\
/WN4mvNIpddUzcJJ/K8pcN3ERqozt5jPUd1mVcdgQb+82xOIbHg7ts/fLZIMUng/\n\
u0A0CarOK5Hhka9cVbT5HCw=\n\
-----END PRIVATE KEY-----\n";
const SIGNING_KEY_MODULUS: &str = "vlSDCowZiQvXhBlhF-9FX5YhalMrscUFRQYqNT2wxmIdNF06ihWh3TpJ7PeyIT2pqeYbJUgDBVvhR835D2_i4th9CGOfVCEzAlahs3upcRlTnj8jP6PsTormKqjnM97swhfTO7DtXKF0dkG2Ne_oRCf0AQZA7f1O2dgXsBOfmSDDJ7YlE3df1YhS1vmEvb1Ib500b2cc3I08GSS8W7wy5iguOJkXup4WMuWd0ZkBaJZaPXJdfi2hHJ2rUqEMbt1f8-IFjxA0viQduaiJQ1SoM21v-_LeKt36XZ1v-cbn8NQIRtwul0xAj2u7NA6FxYiIQzqxif3lm_mqJylurX_y5w";
const FORGED_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCmcd3wuvKAUqFf\n\
OMgM1drh4znIIvwq3waAn1bNKE1RfAgH5GW9OMNhcJ7k7/ZnEK1WoCONkavSIAPX\n\
D+7HD5TuXvOJyDAuob9JRECcua1BagNj4A2dt5HXmulpCoMJq1jB1XlEr/tF3jfM\n\
Lkz25UMVcqvDlgXGQ+KDWWoV54r+2owO1D2VqLFro9CMN7Znk2LqRG9/oj/cERZ2\n\
FfWP9khiReN9dwq3/9lPF5zUTi4DJPUDMs2xNuvsHs+JKLl875V5TXKhU5Kg0Fjc\n\
CK+oWGuFVzhozSoJLOUDh1RxQOz3ZGRTbwKX3m4mCbcE6AUse8e5UUjaOD47bxQ/\n\
SXOzw5NTAgMBAAECggEAB+vdYyqShfboxS5fnvTCkQOOaW74z1WD6rUqnWLwoxMj\n\
ErUU/ndoHQgOF9kvweHyTfAbIQ6wvbRWWnLgq0qbjOip8IG9gUqjXupHpO3VSAqy\n\
lJtCKqJk53zw5NXiPKCR/c+aEOHLNJ0JzMbbVNi+Y5PshsdqCPCMJDIqMVaur8hF\n\
KNLDPeQeRCfR3NWBjui9wvqFWl7iYIIWijL/gGd0vwUK9WkjXmsJjo6qCQ752m8v\n\
YWZtljTkGAcB5bNy2nP27HxCqH1nC29uK4FnmdydCR50Zks5AXp7WZ8SGeagqaCu\n\
zg305AMOvkOWPOuCB9wPU/UrHBod5S+ZgNfswFnUaQKBgQDkPF0xRan474kAUCQ7\n\
CuIxyWSrmy2OxHj2JFN4g0M3O0N+sv6kAi39YbeGiYMo1O9L0UUp/EfpglX0oX1Q\n\
+uaE0O9iv9TdXOLh7ECMbX6dDsuWH7oPdlSrILpLtg7ydQUbY3/H2fSBes+DQee8\n\
aLN6urSzt00gmYTOCIlz9O1OlwKBgQC6sTrhQAra71WpW7pQPL1Iyi1NIpGvlSKB\n\
T77m0IpDGTzVsUO5uiL0LGSNfdgo7sKvd4u41KiP6/OKqDH4UOLJNpfANDlkBZd6\n\
zK//gRd0hyq9ZEX3ohIv3S9ixc41QYCnrSSVmrTsJmYSj48vdGftIGMKZzA5rZyS\n\
gnjz/Fj0pQKBgEPIycTAc+LDAWVeZ+RBLzyqG4P76mPkZih0UXYRRf05VKpkbsw2\n\
+YGva9pBLPsLgqZ52JjHnHP1iaHF9HoAikYX54DkiEvWMmv2xpTuUBRFOKG1eoTt\n\
F+hbJ/kkcPwBVSr1OhYY3U03NDo/cJu8XNGuccyOPBHahl7KmkgeDfyxAoGAP5oX\n\
fBusVxAId+0XGTTIK58tNcUcKtw86B7543qST++uKQL+Fq0SMWtaSlXgKXnF+69X\n\
e31/e6Cf1NIeTAGKcZBWvJVOJAQ/qgdHwl0BV6Kg7erWZ1ngpkvNU/IdoUkSgo1b\n\
FFijYHxoc9RSrJEUwtUwyJ+E73F1EPjAKicDSz0CgYBo2MHy89R6j50d9aDci/lX\n\
vSx0MGi375hDrT/NzmujgAsH1lKuZbTO53t84LXHgmjO+n/o0oTWtPXQngxsI8qq\n\
1w91gKvoFonYzRNlP19jkdxgajvvVWnLNV14h7bHHb+jNjBl/oyceSSqA6T3an6q\n\
3KMZT5oOXjGTdhl/MxlBrA==\n\
-----END PRIVATE KEY-----\n";

struct Webhook {
    server: TestServer,
    _jwks: MockServer,
}

async fn webhook_fixture() -> Webhook {
    let jwks = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [{
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "kid": KID,
                "n": SIGNING_KEY_MODULUS,
                "e": "AQAB",
            }]
        })))
        .mount(&jwks)
        .await;

    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .post_json(
            "/api/v1/admin/notifications/agents",
            &json!({
                "agent_type": "google_chat",
                "name": "Chat",
                "enabled": true,
                "config": {
                    "project_number": PROJECT_NUMBER,
                    "space": "spaces/AAA",
                    "jwk_base_url": format!("{}/", jwks.uri()),
                },
                "triggers": [],
            }),
        )
        .await
        .assert_status(StatusCode::CREATED);

    Webhook {
        server,
        _jwks: jwks,
    }
}

fn sign(pem: &str, claims: Value) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.to_string());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).expect("test key parses");
    jsonwebtoken::encode(&header, &claims, &key).expect("signs")
}

fn chat_claims(email: Option<&str>) -> Value {
    let now = jsonwebtoken::get_current_timestamp() as i64;
    let mut claims = json!({
        "iss": CHAT_ISSUER,
        "aud": PROJECT_NUMBER,
        "sub": "users/1234",
        "iat": now - 10,
        "exp": now + 600,
    });
    if let Some(email) = email {
        claims["email"] = json!(email);
    }
    claims
}

fn message_event(text: &str) -> Value {
    json!({
        "type": "MESSAGE",
        "eventTime": "2026-08-24T10:00:00.000000Z",
        "space": { "name": "spaces/AAA", "type": "ROOM" },
        "user": {
            "name": "users/1234",
            "displayName": "Her",
            "email": "her@example.com",
            "type": "HUMAN"
        },
        "message": {
            "name": "spaces/AAA/messages/MMM",
            "sender": {
                "name": "users/1234",
                "displayName": "Her",
                "email": "her@example.com",
                "type": "HUMAN"
            },
            "text": format!("@kobbler {text}"),
            "argumentText": format!(" {text}"),
            "thread": { "name": "spaces/AAA/threads/TTT" }
        }
    })
}

fn card_click_event() -> Value {
    json!({
        "type": "CARD_CLICKED",
        "space": { "name": "spaces/AAA", "type": "ROOM" },
        "user": {
            "name": "users/1234",
            "displayName": "Her",
            "email": "her@example.com",
            "type": "HUMAN"
        },
        "common": {
            "invokedFunction": "chat_choice",
            "parameters": { "choice_id": "episode:abc" }
        }
    })
}

async fn post_event(fixture: &Webhook, jwt: &str, event: &Value) -> TestResponse {
    TestClient::new(&fixture.server.api_url)
        .with_auth(jwt)
        .post_json("/api/v1/chat/google_chat/webhook", event)
        .await
}

#[tokio::test]
async fn a_validly_signed_message_is_accepted_and_resolves_its_sender() {
    let fixture = webhook_fixture().await;
    // users.role is admin, the ceiling is the default contributor: chat must be capped.
    let bound = bind(
        &fixture.server,
        "her@test.com",
        ROLE_ADMIN,
        Some("her@example.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;
    let other = bind(
        &fixture.server,
        "him@test.com",
        ROLE_CONTRIBUTOR,
        Some("him@example.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let jwt = sign(SIGNING_KEY_PEM, chat_claims(Some("her@example.com")));
    let response = post_event(&fixture, &jwt, &message_event("get season 24")).await;
    response.assert_ok();
    assert_eq!(
        response.json::<Value>(),
        json!({ "text": "Looking…" }),
        "the ack is the transport's synchronous reply, so it costs no round trip"
    );

    let hers = chat_identities::get(&fixture.server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert!(hers.last_seen_at.is_some(), "her binding resolved");
    assert_eq!(
        hers.external_id.as_deref(),
        Some("users/1234"),
        "the transport's stable id is learned on first contact"
    );

    let his = chat_identities::get(&fixture.server.pool, other.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert!(
        his.last_seen_at.is_none(),
        "only the sender's row is touched"
    );

    let p = principal(
        identity::resolve(
            &fixture.server.pool,
            &sender(Some("users/1234"), Some("her@example.com")),
        )
        .await
        .expect("resolve succeeds"),
    );
    assert_eq!(p.identity_id, bound.identity_id);
    assert_eq!(p.auth.user_id, bound.user_id);
    assert_eq!(
        p.auth.role, ROLE_CONTRIBUTOR,
        "an admin account is still capped by the identity's ceiling"
    );
}

#[tokio::test]
async fn a_validly_signed_card_click_is_accepted() {
    let fixture = webhook_fixture().await;
    let bound = bind(
        &fixture.server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("her@example.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    let jwt = sign(SIGNING_KEY_PEM, chat_claims(Some("her@example.com")));
    let response = post_event(&fixture, &jwt, &card_click_event()).await;
    response.assert_ok();
    assert_eq!(
        response.json::<Value>(),
        json!({ "text": "Got it." }),
        "a tap carries no text, so the ack is the no-text one"
    );

    let hers = chat_identities::get(&fixture.server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert!(hers.last_seen_at.is_some());
}

#[tokio::test]
async fn a_token_signed_by_the_wrong_key_is_rejected() {
    let fixture = webhook_fixture().await;
    let bound = bind(
        &fixture.server,
        "her@test.com",
        ROLE_CONTRIBUTOR,
        Some("her@example.com"),
        None,
        ROLE_CONTRIBUTOR,
    )
    .await;

    // Correct iss, correct aud, a kid Google really publishes — signed by somebody else.
    let jwt = sign(FORGED_KEY_PEM, chat_claims(Some("her@example.com")));
    post_event(&fixture, &jwt, &message_event("get season 24"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let hers = chat_identities::get(&fixture.server.pool, bound.identity_id)
        .await
        .expect("load")
        .expect("still there");
    assert!(
        hers.last_seen_at.is_none(),
        "a forged token must never reach the resolver"
    );
}

#[tokio::test]
async fn wrong_audience_wrong_issuer_and_expiry_are_rejected() {
    let fixture = webhook_fixture().await;
    let now = jsonwebtoken::get_current_timestamp() as i64;

    let mut wrong_aud = chat_claims(Some("her@example.com"));
    wrong_aud["aud"] = json!("999999999999");

    let mut wrong_iss = chat_claims(Some("her@example.com"));
    wrong_iss["iss"] = json!("attacker@example.com");

    // Well past jsonwebtoken's 60 s clock-skew leeway.
    let mut expired = chat_claims(Some("her@example.com"));
    expired["exp"] = json!(now - 3600);
    expired["iat"] = json!(now - 7200);

    for claims in [wrong_aud, wrong_iss, expired] {
        let jwt = sign(SIGNING_KEY_PEM, claims);
        post_event(&fixture, &jwt, &message_event("hi"))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn an_ignored_event_kind_is_accepted_and_answers_nothing() {
    let fixture = webhook_fixture().await;
    let jwt = sign(SIGNING_KEY_PEM, chat_claims(Some("her@example.com")));

    let response = post_event(
        &fixture,
        &jwt,
        &json!({
            "type": "ADDED_TO_SPACE",
            "space": { "name": "spaces/AAA", "type": "ROOM" },
            "user": { "name": "users/1234", "type": "HUMAN" }
        }),
    )
    .await;
    response.assert_ok();
    assert!(response.text().is_empty(), "nothing to say, so say nothing");
}

#[tokio::test]
async fn an_unmapped_sender_gets_one_sentence_and_no_turn() {
    let fixture = webhook_fixture().await;
    let jwt = sign(SIGNING_KEY_PEM, chat_claims(Some("stranger@example.com")));

    let mut event = message_event("get season 24");
    event["user"]["email"] = json!("stranger@example.com");
    event["message"]["sender"]["email"] = json!("stranger@example.com");

    let response = post_event(&fixture, &jwt, &event).await;
    response.assert_ok();
    let body = response.json::<Value>();
    assert!(
        body["text"]
            .as_str()
            .expect("a text reply")
            .contains("not set up"),
        "got {body}"
    );
}

#[tokio::test]
async fn a_sender_without_an_email_is_offered_a_pairing_code() {
    let fixture = webhook_fixture().await;
    let jwt = sign(SIGNING_KEY_PEM, chat_claims(None));

    let mut event = message_event("hello");
    event["user"]
        .as_object_mut()
        .expect("object")
        .remove("email");
    event["message"]["sender"]
        .as_object_mut()
        .expect("object")
        .remove("email");

    let response = post_event(&fixture, &jwt, &event).await;
    response.assert_ok();
    let text = response.json::<Value>()["text"]
        .as_str()
        .expect("a text reply")
        .to_string();

    let pending = chat_identities::list_pending_pairing_codes(&fixture.server.pool)
        .await
        .expect("list");
    assert_eq!(pending.len(), 1);
    assert!(text.contains(&pending[0].code), "got {text}");
    assert_eq!(pending[0].external_id, "users/1234");
}
