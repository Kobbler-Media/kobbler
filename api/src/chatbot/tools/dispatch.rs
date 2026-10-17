//! Where a tool call becomes work, and the only place a role is checked.
//!
//! The check happens **before** the tool runs, never inside it. The tools are
//! in-process function calls, so nothing extracts an `AuthUser` for them the way
//! an axum handler would — the caller hands one in, built either by the REST
//! extractor or by `chatbot::identity` from a mapped chat address. Either way
//! `require_contributor()` here is the same call the REST handlers make, so the
//! two cannot drift.
//!
//! The match is exhaustive on [`Tool`]. Adding a variant without an arm is a
//! compile error, which is the whole point of the registry.

use serde_json::Value;

use crate::AppState;
use crate::routes::AuthUser;

use super::act;
use super::credits;
use super::iptv;
use super::livetv;
use super::read::{self, ToolOutcome};
use super::seasons;
use super::suggest;
use super::{Role, Tool};

pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub auth: &'a AuthUser,
    /// The conversation this call belongs to. A pending write confirmation is
    /// scoped to it, so a "yes" in one thread cannot fire an offer made in
    /// another.
    pub session_id: uuid::Uuid,
    /// What the last confident `resolve_media` settled on. This is what makes
    /// "no, the other one" and "the next one" resolvable rather than a guess.
    pub context_ref: Option<&'a str>,
}

/// Run one tool call. Unknown names and refused roles come back as failed
/// outcomes rather than errors: the model gets told what went wrong and can
/// say so, which is strictly better than the turn dying.
pub async fn dispatch(ctx: &Ctx<'_>, name: &str, args: &Value) -> ToolOutcome {
    let Some(tool) = Tool::parse(name) else {
        return ToolOutcome::failed(format!("{name} is not a tool I have"));
    };
    if let Err(reason) = permitted(ctx.auth, tool) {
        return ToolOutcome::failed(reason);
    }
    match tool {
        Tool::ResolveMedia => {
            read::resolve_media(ctx.state, ctx.auth.user_id, ctx.context_ref, args).await
        }
        Tool::SearchLibrary => read::search_library(ctx.state, args).await,
        Tool::LibraryStats => read::library_stats(ctx.state).await,
        Tool::FindPassage => read::find_passage(ctx.state, args).await,
        Tool::WhatsDownloading => read::whats_downloading(ctx.state, args).await,
        Tool::RecentlyAdded => read::recently_added(ctx.state, args).await,
        Tool::NextAiring => read::next_airing(ctx.state, args).await,
        Tool::SeasonStatus => seasons::season_status(ctx.state, args).await,
        Tool::Recommend => suggest::recommend(ctx.state, &[ctx.auth.user_id], args).await,
        Tool::WhatsNew => suggest::whats_new(ctx.state, args).await,
        Tool::ListReleases => act::list_releases(ctx, args).await,
        Tool::GrabRelease => act::grab_release(ctx, args).await,
        Tool::Regrab => act::regrab(ctx, args).await,
        Tool::WantIt => act::want_it(ctx, args).await,
        Tool::CancelDownload => act::cancel_download(ctx, args).await,
        Tool::AddToWatchlist => act::add_to_watchlist(ctx, args).await,
        Tool::SetNotificationPrefs => act::set_notification_prefs(ctx, args).await,
        Tool::WhatsOn => iptv::whats_on(ctx, args).await,
        Tool::RecordThis => iptv::record_this(ctx, args).await,
        Tool::PersonCredits => credits::person_credits(ctx.state, args).await,
        Tool::CastOf => credits::cast_of(ctx.state, args).await,
        Tool::LivetvSearchGuide => livetv::search_guide(ctx, args).await,
        Tool::LivetvScheduleRecording => livetv::schedule_recording(ctx, args).await,
        Tool::LivetvCreateSeriesRule => livetv::create_series_rule(ctx, args).await,
        Tool::LivetvListRecordings => livetv::list_recordings(ctx, args).await,
        Tool::LivetvCancel => livetv::cancel(ctx, args).await,
    }
}

fn permitted(auth: &AuthUser, tool: Tool) -> Result<(), String> {
    let allowed = match tool.required_role() {
        Role::Any => true,
        Role::Contributor => auth.require_contributor().is_ok(),
        Role::Admin => auth.require_admin().is_ok(),
    };
    match allowed {
        true => Ok(()),
        false => Err(format!(
            "you do not have permission to {} — tell the user, do not retry",
            tool.name()
        )),
    }
}

/// One short line naming what a call is about to do, for a transport to show
/// while it runs ("Looking for horror films…").
pub fn args_summary(tool: Tool, args: &Value) -> String {
    let first = ["query", "title", "person", "show", "media_type"]
        .into_iter()
        .find_map(|k| args.get(k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (tool, first) {
        (Tool::SearchLibrary, Some(q)) => format!("searching for {q}"),
        (Tool::FindPassage, Some(q)) => format!("looking for \"{q}\""),
        (Tool::NextAiring, Some(t)) => format!("checking the schedule for {t}"),
        (Tool::SeasonStatus, Some(t)) => match args.get("season").and_then(Value::as_i64) {
            Some(n) => format!("counting season {n} of {t}"),
            None => format!("counting the current season of {t}"),
        },
        (Tool::LibraryStats, _) => "counting the library".to_string(),
        (Tool::WhatsDownloading, _) => "checking downloads".to_string(),
        (Tool::RecentlyAdded, _) => "checking what's new".to_string(),
        (Tool::Recommend, _) => match args.get("genre").and_then(Value::as_str) {
            Some(genre) if !genre.trim().is_empty() => format!("looking for {genre} to watch"),
            _ => "looking for something to watch".to_string(),
        },
        (Tool::WhatsNew, _) => "checking the calendar".to_string(),
        (Tool::WantIt, _) => "getting that".to_string(),
        (Tool::ListReleases, _) => "looking for copies".to_string(),
        (Tool::GrabRelease, _) => "starting the download".to_string(),
        (Tool::Regrab, _) => "looking for a different copy".to_string(),
        (Tool::CancelDownload, _) => "stopping that".to_string(),
        (Tool::AddToWatchlist, _) => "adding that to your list".to_string(),
        (Tool::SetNotificationPrefs, _) => "changing your notifications".to_string(),
        (Tool::WhatsOn, _) => match args.get("channel").and_then(Value::as_str) {
            Some(channel) if !channel.trim().is_empty() => format!("checking what's on {channel}"),
            _ => "checking what's on".to_string(),
        },
        (Tool::RecordThis, Some(t)) => format!("setting a recording for {t}"),
        (Tool::RecordThis, _) => "setting a recording".to_string(),
        (Tool::PersonCredits, Some(p)) => format!("looking for things with {p}"),
        (Tool::CastOf, Some(t)) => format!("checking who's in {t}"),
        (Tool::LivetvSearchGuide, Some(t)) => format!("looking for {t} in the TV guide"),
        (Tool::LivetvSearchGuide, _) => "reading the TV guide".to_string(),
        (Tool::LivetvScheduleRecording, _) => "setting a recording".to_string(),
        (Tool::LivetvCreateSeriesRule, _) => "setting up a series recording".to_string(),
        (Tool::LivetvListRecordings, _) => "checking the recordings".to_string(),
        (Tool::LivetvCancel, _) => "stopping that recording".to_string(),
        (t, _) => t.name().replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn viewer() -> AuthUser {
        AuthUser {
            user_id: Uuid::now_v7(),
            role: "viewer".to_string(),
            is_kids: false,
        }
    }

    fn contributor() -> AuthUser {
        AuthUser {
            user_id: Uuid::now_v7(),
            role: "contributor".to_string(),
            is_kids: false,
        }
    }

    #[test]
    fn every_read_tool_is_open_to_a_viewer() {
        for tool in Tool::ALL {
            if tool.required_role() != Role::Any {
                continue;
            }
            assert!(permitted(&viewer(), tool).is_ok(), "{}", tool.name());
            assert!(permitted(&contributor(), tool).is_ok(), "{}", tool.name());
        }
    }

    /// The refusal has to happen here, before the tool, or "no permission" is a
    /// thing we say after having already done it.
    #[test]
    fn a_viewer_is_refused_every_act_tool_except_muting_itself() {
        for tool in Tool::ALL {
            let acting = matches!(
                tool,
                Tool::ListReleases
                    | Tool::GrabRelease
                    | Tool::Regrab
                    | Tool::WantIt
                    | Tool::CancelDownload
                    | Tool::AddToWatchlist
                    | Tool::RecordThis
                    | Tool::LivetvSearchGuide
                    | Tool::LivetvScheduleRecording
                    | Tool::LivetvCreateSeriesRule
                    | Tool::LivetvListRecordings
                    | Tool::LivetvCancel
            );
            assert_eq!(
                permitted(&viewer(), tool).is_err(),
                acting,
                "{}",
                tool.name()
            );
            assert!(permitted(&contributor(), tool).is_ok(), "{}", tool.name());
        }
        assert!(permitted(&viewer(), Tool::SetNotificationPrefs).is_ok());
    }

    #[test]
    fn a_summary_names_what_the_call_is_about() {
        assert_eq!(
            args_summary(Tool::SearchLibrary, &json!({"query": "horror"})),
            "searching for horror"
        );
        assert_eq!(
            args_summary(Tool::LibraryStats, &json!({})),
            "counting the library"
        );
        assert_eq!(
            args_summary(Tool::SearchLibrary, &json!({})),
            "search library"
        );
    }
}
