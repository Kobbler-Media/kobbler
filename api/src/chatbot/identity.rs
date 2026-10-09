//! Maps a chat sender to a real kobbler user.
//!
//! The output of a successful resolve is a genuine [`AuthUser`], so tool dispatch calls
//! `require_contributor()` / `require_admin()` exactly as the REST handlers do. There is
//! deliberately no parallel permission model here.

use rand::RngExt;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::chat_identities::{self, NewPairingCode, PairingCode};
use crate::routes::AuthUser;

pub const ROLE_VIEWER: &str = "viewer";
pub const ROLE_CONTRIBUTOR: &str = "contributor";
pub const ROLE_ADMIN: &str = "admin";

/// A pairing code is a credential: minutes, not days.
pub const PAIRING_TTL_SECONDS: i64 = 15 * 60;
/// Sliding window used to rate-limit code generation per sender.
pub const PAIRING_RATE_WINDOW_SECONDS: i64 = 60 * 60;
/// Codes a single unknown sender may cause to be minted inside the window.
pub const PAIRING_RATE_LIMIT: i64 = 3;
const PAIRING_CODE_LEN: usize = 6;
/// No 0/O/1/I — these get read aloud and typed into an admin screen.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PAIRING_MINT_ATTEMPTS: usize = 8;
const PAIRING_PRUNE_AFTER_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("database error: {0}")]
    Database(#[from] crate::repositories::RepositoryError),
    #[error("could not mint a unique pairing code")]
    CodeExhausted,
}

/// The sender, as the transport describes them. Both keys are optional because
/// Google Chat only populates `email` for users inside the host Workspace domain.
#[derive(Debug, Clone, Default)]
pub struct ChatSender<'a> {
    pub transport: &'a str,
    pub external_id: Option<&'a str>,
    pub email: Option<&'a str>,
    pub display_name: Option<&'a str>,
    pub space: Option<&'a str>,
    pub thread: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ChatPrincipal {
    pub identity_id: Uuid,
    pub auth: AuthUser,
}

#[derive(Debug, Clone)]
pub struct PairingChallenge {
    pub code: String,
    pub expires_at: OffsetDateTime,
    /// True when the sender already had a live code and we handed back the same one.
    pub reissued: bool,
}

/// What the caller is allowed to do with this sender.
///
/// Everything except [`Resolution::Principal`] is terminal: the caller sends at most one
/// short reply and **never** invokes the LLM. That contract lives here rather than in the
/// turn loop so an unknown address in a space cannot burn GPU time.
#[derive(Debug, Clone)]
pub enum Resolution {
    Principal(ChatPrincipal),
    /// Bound, but the admin switched the binding off. One refusal, no pairing code.
    Disabled,
    /// Unknown sender with no email to bind against — issue a code and wait for approval.
    Pairing(PairingChallenge),
    /// Unknown sender we could bind by address if an admin added one. One refusal.
    Unknown,
    /// Unknown sender who has already been told. Say nothing at all.
    Silent,
}

fn role_rank(role: &str) -> u8 {
    match role {
        ROLE_ADMIN => 2,
        ROLE_CONTRIBUTOR => 1,
        _ => 0,
    }
}

/// `min(users.role, chat_identities.max_role)` on `viewer < contributor < admin`.
///
/// An unrecognised role string ranks as `viewer`, so a typo can only ever remove privilege.
pub fn effective_role(user_role: &str, max_role: &str) -> String {
    let rank = role_rank(user_role).min(role_rank(max_role));
    match rank {
        2 => ROLE_ADMIN,
        1 => ROLE_CONTRIBUTOR,
        _ => ROLE_VIEWER,
    }
    .to_string()
}

fn mint_code() -> String {
    let mut rng = rand::rng();
    (0..PAIRING_CODE_LEN)
        .map(|_| PAIRING_ALPHABET[rng.random_range(0..PAIRING_ALPHABET.len())] as char)
        .collect()
}

/// Resolve a sender, learning `external_id` and stamping `last_seen_at` on success.
pub async fn resolve(pool: &PgPool, sender: &ChatSender<'_>) -> Result<Resolution, IdentityError> {
    let found =
        chat_identities::resolve(pool, sender.transport, sender.external_id, sender.email).await?;

    if let Some(identity) = found {
        if !identity.enabled {
            return Ok(Resolution::Disabled);
        }

        // Learn the transport's stable id so a later address change cannot orphan the binding.
        let learn = match (sender.external_id, identity.external_id.as_deref()) {
            (Some(external), None) => Some(external),
            _ => None,
        };
        chat_identities::mark_seen(pool, identity.id, learn).await?;

        return Ok(Resolution::Principal(ChatPrincipal {
            identity_id: identity.id,
            auth: AuthUser {
                user_id: identity.user_id,
                role: effective_role(&identity.user_role, &identity.max_role),
            },
        }));
    }

    // An email is a key an admin can type into the Users-adjacent screen, so an unknown
    // sender who has one needs no pairing dance — just a binding.
    let Some(external_id) = sender.external_id else {
        return Ok(Resolution::Unknown);
    };
    if sender.email.is_some() {
        return Ok(Resolution::Unknown);
    }

    Ok(match issue_pairing_code(pool, sender, external_id).await? {
        Some(challenge) => Resolution::Pairing(challenge),
        None => Resolution::Silent,
    })
}

/// `None` means the sender has burned their allowance — say nothing rather than letting a
/// stranger in a space mint approval requests at the admin.
async fn issue_pairing_code(
    pool: &PgPool,
    sender: &ChatSender<'_>,
    external_id: &str,
) -> Result<Option<PairingChallenge>, IdentityError> {
    if let Some(existing) =
        chat_identities::active_pairing_code(pool, sender.transport, external_id).await?
    {
        return Ok(Some(PairingChallenge {
            code: existing.code,
            expires_at: existing.expires_at,
            reissued: true,
        }));
    }

    let recent = chat_identities::count_recent_pairing_codes(
        pool,
        sender.transport,
        external_id,
        PAIRING_RATE_WINDOW_SECONDS,
    )
    .await?;
    if recent >= PAIRING_RATE_LIMIT {
        return Ok(None);
    }

    let _ = chat_identities::prune_pairing_codes(pool, PAIRING_PRUNE_AFTER_SECONDS).await;

    for _ in 0..PAIRING_MINT_ATTEMPTS {
        let code = mint_code();
        let input = NewPairingCode {
            transport: sender.transport,
            external_id,
            code: &code,
            display_name: sender.display_name,
            space: sender.space,
            thread: sender.thread,
            ttl_seconds: PAIRING_TTL_SECONDS,
        };
        match chat_identities::insert_pairing_code(pool, &input).await {
            Ok(row) => {
                return Ok(Some(PairingChallenge {
                    code: row.code,
                    expires_at: row.expires_at,
                    reissued: false,
                }));
            }
            Err(crate::repositories::RepositoryError::Database(sqlx::Error::Database(e)))
                if e.is_unique_violation() => {}
            Err(e) => return Err(e.into()),
        }
    }

    Err(IdentityError::CodeExhausted)
}

/// Approve a pending code, writing the transport's stable id onto an identity row.
///
/// Single-use: the claim and the write happen in one transaction so two admins racing
/// cannot bind the same code twice.
pub async fn approve_pairing(
    pool: &PgPool,
    transport: &str,
    code: &str,
    identity_id: Uuid,
) -> Result<Option<PairingCode>, IdentityError> {
    let mut tx = pool.begin().await.map_err(map_sqlx)?;

    let Some(claimed) =
        chat_identities::claim_pairing_code(&mut *tx, transport, code, identity_id).await?
    else {
        tx.rollback().await.map_err(map_sqlx)?;
        return Ok(None);
    };

    let patch = chat_identities::ChatIdentityPatch {
        external_id: Some(Some(claimed.external_id.as_str())),
        ..Default::default()
    };
    let updated = chat_identities::update(&mut *tx, identity_id, &patch).await?;
    if updated.is_none() {
        tx.rollback().await.map_err(map_sqlx)?;
        return Ok(None);
    }

    tx.commit().await.map_err(map_sqlx)?;
    Ok(Some(claimed))
}

fn map_sqlx(e: sqlx::Error) -> IdentityError {
    IdentityError::Database(crate::repositories::RepositoryError::Database(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_is_a_minimum_over_both_roles() {
        let cases = [
            ("viewer", "viewer", "viewer"),
            ("viewer", "contributor", "viewer"),
            ("viewer", "admin", "viewer"),
            ("contributor", "viewer", "viewer"),
            ("contributor", "contributor", "contributor"),
            ("contributor", "admin", "contributor"),
            ("admin", "viewer", "viewer"),
            ("admin", "contributor", "contributor"),
            ("admin", "admin", "admin"),
        ];
        for (user_role, max_role, expected) in cases {
            assert_eq!(
                effective_role(user_role, max_role),
                expected,
                "user={user_role} max={max_role}"
            );
        }
    }

    #[test]
    fn unknown_role_strings_rank_as_viewer() {
        assert_eq!(effective_role("wizard", "admin"), "viewer");
        assert_eq!(effective_role("admin", "wizard"), "viewer");
    }

    #[test]
    fn minted_codes_avoid_ambiguous_glyphs() {
        for _ in 0..200 {
            let code = mint_code();
            assert_eq!(code.len(), PAIRING_CODE_LEN);
            assert!(!code.contains(['0', 'O', '1', 'I']), "{code}");
        }
    }
}
