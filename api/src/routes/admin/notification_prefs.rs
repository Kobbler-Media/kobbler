//! The current user's chat notification preferences.
//!
//! Everything here goes through [`crate::notifications::prefs`], the same entry
//! the `set_notification_prefs` chat tool uses. Two implementations of "is she
//! muted" is how the bot ends up talking at 2 am, so this module owns no
//! preference logic of its own — it is a view over [`prefs::apply`].
//!
//! The routes are `Role::Any`: they read and write only the caller's own row,
//! and muting yourself needs no privilege.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use time::OffsetDateTime;

use crate::{
    AppState,
    notifications::{
        NotificationTrigger,
        prefs::{self, PrefUpdate, PrefsError, SetNotificationPrefs, Zone},
    },
    routes::AuthUser,
};

#[derive(Debug, Serialize)]
pub struct NotificationPrefsResponse {
    pub enabled: bool,
    pub triggers: Vec<NotificationTrigger>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub muted_until: Option<OffsetDateTime>,
    /// `muted_until` in the past is not a mute, and a client comparing clocks
    /// itself would get it wrong whenever the two disagree.
    pub muted: bool,
    /// `null` in both when there are no quiet hours at all — either an explicit
    /// zero-width window or one the user cleared. Otherwise `HH:MM`, and
    /// `quiet_from` later than `quiet_to` is a window that crosses midnight,
    /// which is the ordinary case.
    pub quiet_from: Option<String>,
    pub quiet_to: Option<String>,
    /// The window is the built-in one because the user has never set it, so a

    /// IANA name, `null` when the user has not said where they are.
    pub timezone: Option<String>,
    /// The zone the window is actually evaluated in, named out loud — a wrong
    /// zone then reads as a wrong label instead of a message at 2 am.
    pub timezone_label: String,
    pub summary: String,
}

fn view(update: PrefUpdate, now: OffsetDateTime) -> NotificationPrefsResponse {
    let PrefUpdate {
        prefs,
        summary,
        zone,
    } = update;
    let window = prefs.quiet_window();
    NotificationPrefsResponse {
        enabled: prefs.enabled,
        triggers: prefs.triggers,
        muted_until: prefs.muted_until,
        muted: prefs.muted_until.is_some_and(|until| until > now),
        quiet_from: window.map(|(from, _)| prefs::hhmm(from)),
        quiet_to: window.map(|(_, to)| prefs::hhmm(to)),
        timezone: match zone {
            Zone::Iana(tz) => Some(tz.name().to_string()),
            Zone::ServerLocal => None,
        },
        timezone_label: zone.label_at(now),
        summary,
    }
}

fn respond(result: Result<PrefUpdate, PrefsError>) -> axum::response::Response {
    match result {
        Ok(update) => Json(view(update, OffsetDateTime::now_utc())).into_response(),
        Err(PrefsError::Invalid(message)) => (StatusCode::BAD_REQUEST, message).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to read or change notification prefs");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// A user with no row reads back the defaults rather than a 404: the row is
/// created by the first change, not by asking.
pub async fn get_notification_prefs(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    respond(prefs::apply(&state.pool, auth.user_id, &SetNotificationPrefs::default()).await)
}

/// The body is [`SetNotificationPrefs`] verbatim, so the settings page and the
/// chat tool speak one vocabulary.
pub async fn update_notification_prefs(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SetNotificationPrefs>,
) -> impl IntoResponse {
    respond(prefs::apply(&state.pool, auth.user_id, &payload).await)
}
