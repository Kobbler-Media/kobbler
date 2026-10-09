//! The one tool registry for the whole chat bot.
//!
//! Everything the model is told about its tools derives from [`Tool::ALL`], and
//! `dispatch` matches exhaustively on [`Tool`] — so a tool cannot be advertised
//! without a dispatcher, or the crate does not compile. Two sources of tools is
//! how you get a model calling neither, which is exactly what taking the loop
//! off Meili's chat proxy was for.
//!
//! Tools are intents, not subsystems. Media type is an *argument*: the model
//! should never learn that books and video are different stacks, because
//! "the new Sanderson" and "season 24 of someShow" are the same request shape.
//!
//! Schema rules, learned the expensive way on this model:
//!
//! * 3-5 arguments each, semantic enums over free-form strings;
//! * every argument gets a description that says when to fill it in. The live
//!   box dropped a `season: 24` that was plainly in the user's message until
//!   the description named that phrasing;
//! * no argument the model would have to invent — ids come from a previous
//!   tool result, never from the model's memory.

pub mod act;
pub mod credits;
pub mod dispatch;
pub mod iptv;
pub mod livetv;
pub mod read;
pub mod seasons;
pub mod suggest;

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    ResolveMedia,
    SearchLibrary,
    LibraryStats,
    FindPassage,
    WhatsDownloading,
    RecentlyAdded,
    NextAiring,
    SeasonStatus,
    Recommend,
    WhatsNew,
    ListReleases,
    GrabRelease,
    Regrab,
    WantIt,
    CancelDownload,
    AddToWatchlist,
    SetNotificationPrefs,
    WhatsOn,
    RecordThis,
    PersonCredits,
    CastOf,
    LivetvSearchGuide,
    LivetvScheduleRecording,
    LivetvCreateSeriesRule,
    LivetvListRecordings,
    LivetvCancel,
}

impl Tool {
    pub const ALL: [Tool; 26] = [
        Tool::ResolveMedia,
        Tool::SearchLibrary,
        Tool::LibraryStats,
        Tool::FindPassage,
        Tool::WhatsDownloading,
        Tool::RecentlyAdded,
        Tool::NextAiring,
        Tool::SeasonStatus,
        Tool::Recommend,
        Tool::WhatsNew,
        Tool::ListReleases,
        Tool::GrabRelease,
        Tool::Regrab,
        Tool::WantIt,
        Tool::CancelDownload,
        Tool::AddToWatchlist,
        Tool::SetNotificationPrefs,
        Tool::WhatsOn,
        Tool::RecordThis,
        Tool::PersonCredits,
        Tool::CastOf,
        Tool::LivetvSearchGuide,
        Tool::LivetvScheduleRecording,
        Tool::LivetvCreateSeriesRule,
        Tool::LivetvListRecordings,
        Tool::LivetvCancel,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::ResolveMedia => "resolve_media",
            Tool::SearchLibrary => "search_library",
            Tool::LibraryStats => "library_stats",
            Tool::FindPassage => "find_passage",
            Tool::WhatsDownloading => "whats_downloading",
            Tool::RecentlyAdded => "recently_added",
            Tool::NextAiring => "next_airing",
            Tool::SeasonStatus => "season_status",
            Tool::Recommend => "recommend",
            Tool::WhatsNew => "whats_new",
            Tool::ListReleases => "list_releases",
            Tool::GrabRelease => "grab_release",
            Tool::Regrab => "regrab",
            Tool::WantIt => "want_it",
            Tool::CancelDownload => "cancel_download",
            Tool::AddToWatchlist => "add_to_watchlist",
            Tool::SetNotificationPrefs => "set_notification_prefs",
            Tool::WhatsOn => "whats_on",
            Tool::RecordThis => "record_this",
            Tool::PersonCredits => "person_credits",
            Tool::CastOf => "cast_of",
            Tool::LivetvSearchGuide => "livetv_search_guide",
            Tool::LivetvScheduleRecording => "livetv_schedule_recording",
            Tool::LivetvCreateSeriesRule => "livetv_create_series_rule",
            Tool::LivetvListRecordings => "livetv_list_recordings",
            Tool::LivetvCancel => "livetv_cancel",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Tool::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The role the caller must already hold for this tool to run. Checked in
    /// `dispatch`, before the tool, never inside it.
    pub fn required_role(self) -> Role {
        match self {
            Tool::ResolveMedia
            | Tool::SearchLibrary
            | Tool::LibraryStats
            | Tool::FindPassage
            | Tool::WhatsDownloading
            | Tool::RecentlyAdded
            | Tool::NextAiring
            | Tool::SeasonStatus
            | Tool::Recommend
            | Tool::WhatsNew
            | Tool::WhatsOn
            | Tool::PersonCredits
            | Tool::CastOf => Role::Any,
            // Muting yourself needs no privilege, and it is the one thing a
            // viewer must be able to do — a bot she cannot silence gets blocked.
            Tool::SetNotificationPrefs => Role::Any,
            Tool::ListReleases
            | Tool::GrabRelease
            | Tool::Regrab
            | Tool::WantIt
            | Tool::CancelDownload
            | Tool::AddToWatchlist
            | Tool::RecordThis
            // Every Live TV tool schedules work or reads the schedule it is
            // about to change, and each one is a step in the same
            // ask-then-record conversation, so they share one floor.
            | Tool::LivetvSearchGuide
            | Tool::LivetvScheduleRecording
            | Tool::LivetvCreateSeriesRule
            | Tool::LivetvListRecordings
            | Tool::LivetvCancel => Role::Contributor,
        }
    }

    pub fn schema(self) -> Value {
        let (description, parameters) = match self {
            Tool::ResolveMedia => (
                "Work out exactly which book, film, show, season or episode the user means, \
                 including vague phrasing like 'the new one' or 'last night's episode'. It checks \
                 the library first, then the download providers, and drops trailing words one at \
                 a time until something answers - 'Ludwig with David Mitchell' finds the show the \
                 providers call 'Ludwig'. A single show comes back with its seasons when the \
                 provider knows them, and that list is all that exists - do not add to it or \
                 predict from it. Use it before answering about one specific thing, and \
                 prefer it over search_library whenever the user is pointing at a single title.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "What the user called it, in their own words - the whole name as spoken, with the person or year they gave, not the rest of the sentence. 'the latest episode of X' from 'get me a new copy of the latest episode of X, the sound is out of sync'. Pass the phrasing through: 'the new one' and 'last night's episode' are understood.",
                        },
                        "media_type": {
                            "type": "string",
                            "enum": ["book", "audiobook", "movie", "show", "season", "episode", "author", "series", "sports"],
                            "description": "Only when the user made the kind explicit. Leave it out when they did not — a wrong hint is worse than none.",
                        },
                        "season": {
                            "type": "integer",
                            "description": "Season number, whenever the user named one — 'season 24', 'series 3', 'S24E07' all mean fill this in.",
                        },
                        "episode": {
                            "type": "integer",
                            "description": "Episode number, whenever the user named one — 'episode 7' or 'S24E07' mean fill this in.",
                        },
                        "author": {
                            "type": "string",
                            "description": "The author's name when the user named one, for books. 'the new Sanderson' means author='Sanderson'.",
                        },
                    },
                    "required": ["query"],
                }),
            ),
            Tool::SearchLibrary => (
                "Search the media library: books, audiobooks, movies, TV shows, episodes, \
                 people, authors, series and genres. Use this for any question about what is \
                 in the library or what something is.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The title or name to search for, in the user's own words. Do not put season or episode numbers in here — they have their own arguments.",
                        },
                        "media_type": {
                            "type": "string",
                            "enum": ["any", "book", "movie", "show", "episode", "person", "author", "series", "genre"],
                            "description": "Narrow the search to one kind of thing. Use 'any' when the user did not say, or when a title could be either a film or a show.",
                        },
                        "season": {
                            "type": "integer",
                            "description": "Season number, whenever the user named one — 'season 24', 'series 3', 'S24E07' all mean fill this in. Leave it out if they did not.",
                        },
                        "episode": {
                            "type": "integer",
                            "description": "Episode number, whenever the user named one — 'episode 7' or 'S24E07' mean fill this in. Leave it out if they did not.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many results to return. Default 10, maximum 40. Ask for more when the user wants a shortlist to choose from.",
                        },
                    },
                    "required": ["query"],
                }),
            ),
            Tool::LibraryStats => (
                "Counts of what is in the library: books, audiobooks, movies, shows, sports, \
                 available episodes, and how many things are on the want list. Use it for \
                 'how many' and 'how big' questions.",
                json!({"type": "object", "properties": {}}),
            ),
            Tool::FindPassage => (
                "Find a specific passage, quote or scene inside an audiobook or ebook. \
                 Returns the surrounding text with a timestamp for audiobooks and a CFI \
                 location for ebooks.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The words to look for, as close to what the user remembers as possible.",
                        },
                        "book_id": {
                            "type": "string",
                            "description": "Restrict the search to one book, using an id returned by a previous search_library call. Leave it out to search every transcribed book.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many passages to return. Default 5, maximum 20.",
                        },
                    },
                    "required": ["query"],
                }),
            ),
            Tool::WhatsDownloading => (
                "What is downloading right now, with progress. Use it for 'is it here yet', \
                 'what's downloading' and 'did that ever finish'.",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "How many downloads to return. Default 10, maximum 25.",
                        },
                    },
                }),
            ),
            Tool::RecentlyAdded => (
                "What has arrived in the library lately, newest first. Use it for \
                 'what's new', 'anything good just come in' and 'what did we just get'.",
                json!({
                    "type": "object",
                    "properties": {
                        "media_type": {
                            "type": "string",
                            "enum": ["any", "book", "audiobook", "movie", "episode", "sport"],
                            "description": "Narrow to one kind of thing. Use 'any' unless the user asked specifically. 'audiobook' is dated from when the audio arrived, which is not when the book did.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many items to return. Default 10, maximum 30.",
                        },
                    },
                }),
            ),
            Tool::NextAiring => (
                "When the next episode of a TV show or the next fixture of a sport airs. \
                 Use it for 'when's the new one out' and 'is there another season'. The \
                 result carries a caveat about how reliable the date is — repeat it.",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "The name of the show or competition, in the user's own words.",
                        },
                    },
                    "required": ["title"],
                }),
            ),
            Tool::SeasonStatus => (
                "How many episodes one season of a TV show has, how many have aired, how many \
                 you actually have, and which aired ones are missing. Use it for 'how many \
                 episodes does this season have', 'have I got all of season 3' and 'what am I \
                 missing' — it counts properly, where search_library only returns a page of \
                 episodes. Episodes it lists as still to come have not aired and are not gaps.",
                json!({
                    "type": "object",
                    "properties": {
                        "show": {
                            "type": "string",
                            "description": "The name of the show, in the user's own words. Do not put a season number in here — that has its own argument.",
                        },
                        "season": {
                            "type": "integer",
                            "description": "Season number, only when the user named one — 'season 3', 'series 3', 'S03'. Leave it out for 'the current season', 'this season' or when they did not say: the newest season that has started airing is used.",
                        },
                    },
                    "required": ["show"],
                }),
            ),
            Tool::WantIt => (
                "Get something: download a book, film, episode, a season or all the seasons of a show, \
                 or start following an author or series so their new things arrive on their own. \
                 the same whatever kind of thing it is, and whether or not it is out yet — \
                 something with no copy to be had waits and arrives on its own once there is \
                 one. It acts immediately - only call it for an explicit request.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The ref from resolve_media for the one thing they mean, e.g. 'provider:tmdb:1399:s02' or 'show:<uuid>' - a season is always 'provider:<provider>:<id>:s<NN>'. Never make one up, and never act on an ambiguous result - ask which one instead. If resolve_media found nothing, say so; do not guess a ref.",
                        },
                        "season": {
                            "type": "integer",
                            "description": "Only with a provider show ref when they named a season: it makes the season ref for you ('provider:tmdb:243360' + season 2). Never with a season or episode ref - those already say which.",
                        },
                        "episode": {
                            "type": "integer",
                            "description": "Only with a provider show ref when they named an episode, together with season.",
                        },
                        "format": {
                            "type": "string",
                            "enum": ["ebook", "audiobook", "both"],
                            "description": "For a book, when they said which they wanted or resolve_media worked it out. Leave it out otherwise.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            Tool::ListReleases => (
                "Every copy of one thing that is out there right now, best first — the raw \
                 search, not what the library's quality settings would allow. Use it only \
                 when they want to choose one themselves or have named a resolution. When \
                 they just want a different copy, regrab does it without a list and stays \
                 inside those settings. Only a few rows come back, so resolutions_available \
                 is the only thing that says what exists — never tell them a resolution is \
                 unavailable off the rows alone. Reading the list changes nothing.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The ref from resolve_media for the film, episode or book to look for. Seasons and shows do not have their own release list.",
                        },
                        "resolution": {
                            "type": "string",
                            "enum": ["any", "2160p", "1080p", "720p", "480p"],
                            "description": "Only when they said what they wanted — 'we want 1080p'. It filters the whole search, not the rows you were shown.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            Tool::GrabRelease => (
                "Download one specific copy from a list_releases result. Only use it after \
                 showing them the list, because it takes exactly what they named and ignores \
                 the library's quality settings. For 'get me another copy, this one is \
                 broken' use regrab instead — it leaves the choosing to the downloader.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The same ref you passed to list_releases.",
                        },
                        "release_index": {
                            "type": "integer",
                            "description": "The release_index of the copy they picked, straight from the list_releases result. Never a number they said unless the list was numbered.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            Tool::Regrab => (
                "Get a different copy of something already in the library — 'another copy', \
                 'a new copy', 'this one is broken', 'the sound is out of sync', 'it has audio \
                 descriptions'. It rules out the copy they have and hands the search back to \
                 the downloader, which picks under the quality settings the library is set up \
                 with. That is also the answer when they ask you to choose for them. Do not \
                 show a list first. Only when they insist on a particular resolution do you \
                 use list_releases and grab_release instead.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The ref from resolve_media for the episode, film or book they already have.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            Tool::CancelDownload => (
                "Stop a download that is running — 'stop that', 'cancel it', 'don't bother'. \
                 It only stops the download; nothing already on disk is touched.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The ref from resolve_media for the thing being downloaded. Use whats_downloading first if you are not sure which one they mean.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            Tool::AddToWatchlist => (
                "Put a film or show on their own list so it turns up in Up Next. Costs nothing \
                 and nothing gets downloaded. It is the right offer when something is not out \
                 yet or they are not sure.",
                json!({
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "The ref from resolve_media for the film or show. An episode or season ref puts its show on the list.",
                        },
                    },
                    "required": ["ref"],
                }),
            ),
            // The one tool whose schema is not written here: preferences are
            // `notifications::prefs`'s vocabulary, and a second copy of the enum
            // is how the tool starts accepting words the logic cannot apply.
            Tool::SetNotificationPrefs => (
                crate::notifications::prefs::tool_description(),
                crate::notifications::prefs::tool_parameters(),
            ),
            Tool::Recommend => (
                "Suggest something to watch or read from a mood, a vibe or a comparison — \
                 'a horror film like Carrie', 'something light for a weeknight'. Turn the \
                 mood into the filters below, then pick three from the shortlist it returns \
                 and give one sentence each on why. Things the caller has already seen are \
                 left out unless they ask for a rewatch.",
                json!({
                    "type": "object",
                    "properties": {
                        "media_type": {
                            "type": "string",
                            "enum": ["any", "movie", "show", "book"],
                            "description": "Narrow to one kind of thing. Use 'any' unless the user said what they were in the mood for.",
                        },
                        "genre": {
                            "type": "string",
                            "description": "The genre the mood maps onto — 'like Carrie' means genre='Horror'. One genre, in its normal English name.",
                        },
                        "years": {
                            "type": "string",
                            "description": "When they want it from, in their words: '1970-1985', 'the 70s', 'since 2020', 'before 1990'. Leave it out if they did not say.",
                        },
                        "also_for": {
                            "type": "string",
                            "description": "Another person in the household when the user says 'we' or names someone — their watch history is subtracted too, so nobody gets a repeat.",
                        },
                        "include_watched": {
                            "type": "boolean",
                            "description": "True only when the user asks for a rewatch or says they do not mind having seen it. Defaults to false.",
                        },
                    },
                }),
            ),
            Tool::WhatsOn => (
                "What is on live TV, from the TV guide. Use it for 'what's on', 'anything on \
                 tonight' and 'what's on BBC One'. It only reads the guide, so nothing is \
                 recorded and nothing is downloaded.",
                json!({
                    "type": "object",
                    "properties": {
                        "channel": {
                            "type": "string",
                            "description": "The channel they named, in their own words — 'BBC One', 'Sky Sports'. Leave it out when they did not name one and you get their own channels instead.",
                        },
                        "when": {
                            "type": "string",
                            "enum": ["now", "tonight", "later"],
                            "description": "Which part of the day they mean: 'now' for what is on this minute, 'tonight' for this evening, 'later' for the next few hours. Defaults to now. Never work the dates out yourself.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many programmes to return. Default 8, maximum 12. Ask for fewer when they only want the headline.",
                        },
                    },
                }),
            ),
            Tool::RecordThis => (
                "Record something off live TV. The recording runs by itself at the time the \
                 guide gives, so there is nothing to download and nothing to wait for. It acts \
                 immediately - only call it for an explicit request.",
                json!({
                    "type": "object",
                    "properties": {
                        "programme_id": {
                            "type": "string",
                            "description": "The programme_id of a listing from a whats_on result, copied exactly. Always prefer this, and never make one up.",
                        },
                        "title": {
                            "type": "string",
                            "description": "The name of the programme in their own words, when you have not looked it up with whats_on yet. It gets matched against the guide.",
                        },
                        "channel": {
                            "type": "string",
                            "description": "The channel they named, when they named one. It settles a programme that is showing on more than one channel.",
                        },
                    },
                }),
            ),
            Tool::PersonCredits => (
                "What the library holds with one particular actor, director or writer in it. \
                 Use it for 'what Ashton Kutcher films do I have', 'what else has she been in' \
                 and 'anything with him in it' — search_library finds the person but never the \
                 titles crediting them.",
                json!({
                    "type": "object",
                    "properties": {
                        "person": {
                            "type": "string",
                            "description": "The person's name in the user's own words — 'Ashton Kutcher', 'Tilda Swinton'. Pass it through as they said it.",
                        },
                        "media_type": {
                            "type": "string",
                            "enum": ["any", "movie", "show"],
                            "description": "Only when the user said films or TV specifically. Use 'any' for 'what has she been in', and whenever they did not say.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many titles to return of each kind. Default 15, maximum 40. Ask for more when they want the whole list.",
                        },
                    },
                    "required": ["person"],
                }),
            ),
            Tool::CastOf => (
                "Who is in one film or show: the billed cast with the characters they play, \
                 plus its director or creator. Use it for \"who's in it\", \"who plays the dad\" \
                 and \"who made this\".",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "The name of the film or show in the user's own words. Do not put a year or a season number in here.",
                        },
                        "media_type": {
                            "type": "string",
                            "enum": ["any", "movie", "show"],
                            "description": "Only when the user made the kind explicit — 'the film', 'the series'. Use 'any' when they did not: a wrong hint is worse than none.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many cast members to return, in billing order. Default 15, maximum 40. Ask for more when they want the full cast.",
                        },
                    },
                    "required": ["title"],
                }),
            ),
            Tool::WhatsNew => (
                "What arrived or is about to arrive across TV, sport, films and books. \
                 Use it for 'what came out this week' and 'anything on this week'. The \
                 result carries a caveat about how reliable the dates are — repeat it.",
                json!({
                    "type": "object",
                    "properties": {
                        "window": {
                            "type": "string",
                            "enum": ["past_week", "coming_week"],
                            "description": "Which direction to look: 'past_week' for what has already landed, 'coming_week' for what is due. Defaults to past_week.",
                        },
                    },
                }),
            ),
            Tool::LivetvSearchGuide => (
                "Search the broadcast TV guide for programmes to record, in the next 48 hours, \
                 by title, genre or channel. Call this before any recording: it comes back with \
                 the duplicate airings already collapsed, one airing to record per episode, a \
                 count of the duplicates it dropped, and an `ask_about` list naming what is still \
                 ambiguous. Ask the user about everything in `ask_about` before recording anything.",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "The name of the programme in the user's own words, kept loose — just the words that name it, not the rest of the sentence. Do not add a season, an episode number or a channel to this string; they have their own arguments or belong in the follow-up question.",
                        },
                        "genre": {
                            "type": "string",
                            "description": "Only when the user asked by kind rather than by name — 'anything on the footy', 'a film tonight'. Words like drama, news, sport, movie. Leave it out when they named a programme.",
                        },
                        "channel": {
                            "type": "string",
                            "description": "Only when the user named a channel — 'on Nine', 'ABC'. Leave it out otherwise: narrowing to a channel hides the other channels carrying the same series, which is the thing you are meant to ask about.",
                        },
                        "when": {
                            "type": "string",
                            "enum": ["next_few_hours", "tonight", "tomorrow", "next_48_hours"],
                            "description": "Which slice of the guide the user meant. Use next_48_hours, the default, whenever they did not say a time — never work out dates yourself.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many programmes to return. Default 8, maximum 20. Ask for more only when the user wants a longer list to choose from.",
                        },
                    },
                }),
            ),
            Tool::LivetvScheduleRecording => (
                "Record one airing of one programme off broadcast TV, addressed by a guide_id from \
                 livetv_search_guide. This is the one-off: use livetv_create_series_rule when the \
                 user wants every episode. It acts immediately, and recording holds a tuner for \
                 hours, so only call it for an explicit request. If every tuner is busy it schedules nothing and hands you a later \
                 airing of the same episode to offer instead.",
                json!({
                    "type": "object",
                    "properties": {
                        "guide_id": {
                            "type": "string",
                            "description": "The guide_id of the single airing to record, copied from a livetv_search_guide result. Use the one under `record` unless the user picked a different showing. Never make one up and never send more than one airing of the same episode.",
                        },
                    },
                    "required": ["guide_id"],
                }),
            ),
            Tool::LivetvCreateSeriesRule => (
                "Set a standing rule that records a series off broadcast TV as its episodes air, \
                 addressed by a guide_id of any one airing from livetv_search_guide. Use it for \
                 'record every episode' and 'keep recording it'; use livetv_schedule_recording for \
                 one showing. A series often airs on two channels at once, one of them a repeat \
                 of an old season, so ask which the user means and pass it as scope.",
                json!({
                    "type": "object",
                    "properties": {
                        "guide_id": {
                            "type": "string",
                            "description": "The guide_id of one airing of the series, copied from a livetv_search_guide result. The rule follows that airing's series, not that one showing.",
                        },
                        "scope": {
                            "type": "string",
                            "enum": ["this_channel_and_slot", "this_channel", "any_channel"],
                            "description": "Which airings the rule may take, once the user has told you. this_channel_and_slot pins it to that channel on that weekday and around that time, which is what keeps a rerun channel out. Leave it out when they have not said and the tool will ask.",
                        },
                        "new_only": {
                            "type": "boolean",
                            "description": "True, the default, records only episodes not already recorded. Send false only when the user explicitly asks for repeats and the back catalogue too.",
                        },
                    },
                    "required": ["guide_id"],
                }),
            ),
            Tool::LivetvListRecordings => (
                "What broadcast TV is recording right now, what is queued, and what has finished, \
                 plus the standing series rules. Use it for 'is that recording', 'what have you \
                 got queued' and before cancelling anything — the ids it returns are the only ones \
                 livetv_cancel accepts.",
                json!({
                    "type": "object",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["any", "recording", "scheduled", "done", "failed"],
                            "description": "Narrow to one kind. Use 'recording' for 'what's recording now', 'scheduled' for what is queued, 'done' for what has finished. Use 'any', the default, when they did not say.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many recordings to return, soonest first. Default 8, maximum 20. Ask for more when they want the whole list.",
                        },
                    },
                }),
            ),
            Tool::LivetvCancel => (
                "Drop a queued broadcast recording, stop one that is in progress, or turn off a \
                 standing series rule. Takes an id from livetv_list_recordings and nothing else. \
                 It will not delete a recording that has already finished.",
                json!({
                    "type": "object",
                    "properties": {
                        "recording_id": {
                            "type": "string",
                            "description": "The recording_id of the one recording to drop or stop, copied from a livetv_list_recordings result. Never make one up.",
                        },
                        "rule_id": {
                            "type": "string",
                            "description": "The rule_id of the standing series rule to turn off, copied from a livetv_list_recordings result. Send this instead of recording_id when they want to stop following a series.",
                        },
                    },
                }),
            ),
        };
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": description,
                "parameters": parameters,
            },
        })
    }
}

/// The role floor a tool needs. Read tools are `Any`; the act tools a later
/// phase adds are `Contributor`, and `dispatch` already enforces both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Any,
    Contributor,
    Admin,
}

pub fn schemas() -> Vec<Value> {
    Tool::ALL.iter().map(|t| t.schema()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_advertised_tool_round_trips_through_its_name() {
        for tool in Tool::ALL {
            assert_eq!(Tool::parse(tool.name()), Some(tool), "{}", tool.name());
        }
        assert_eq!(Tool::parse("download_everything"), None);
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<&str> = Tool::ALL.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    /// A wide schema drowns this model, and an argument with no description is
    /// the argument it silently drops.
    #[test]
    fn every_schema_is_narrow_and_fully_described() {
        for tool in Tool::ALL {
            let schema = tool.schema();
            let properties = schema["function"]["parameters"]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{} has no properties object", tool.name()));
            // want_it alone carries six: ref plus season/episode (a show-plus-season is one
            // call; folding them into ref would make the model compose provider ids by hand)
            // Everything else stays at five.
            let cap = if tool == Tool::WantIt { 6 } else { 5 };
            assert!(
                properties.len() <= cap,
                "{} exposes {} arguments",
                tool.name(),
                properties.len()
            );
            for (name, spec) in properties {
                assert!(
                    spec.get("description")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|d| d.len() > 20),
                    "{}.{name} needs a description that says when to fill it in",
                    tool.name()
                );
            }
            assert!(
                !schema["function"]["description"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty()
            );
        }
    }

    /// Read is free. Everything that spends disk needs a contributor, and the
    /// check lives in `dispatch`, before the tool runs.
    #[test]
    fn reading_is_open_and_acting_is_not() {
        let read = [
            Tool::ResolveMedia,
            Tool::SearchLibrary,
            Tool::LibraryStats,
            Tool::FindPassage,
            Tool::WhatsDownloading,
            Tool::RecentlyAdded,
            Tool::NextAiring,
            Tool::SeasonStatus,
            Tool::Recommend,
            Tool::WhatsNew,
            Tool::WhatsOn,
        ];
        for tool in read {
            assert_eq!(tool.required_role(), Role::Any, "{}", tool.name());
        }
        for tool in [
            Tool::ListReleases,
            Tool::GrabRelease,
            Tool::Regrab,
            Tool::WantIt,
            Tool::CancelDownload,
            Tool::AddToWatchlist,
            Tool::RecordThis,
            Tool::LivetvSearchGuide,
            Tool::LivetvScheduleRecording,
            Tool::LivetvCreateSeriesRule,
            Tool::LivetvListRecordings,
            Tool::LivetvCancel,
        ] {
            assert_eq!(tool.required_role(), Role::Contributor, "{}", tool.name());
        }
        assert_eq!(Tool::SetNotificationPrefs.required_role(), Role::Any);
    }

    /// Every act tool is addressed by a resolver ref, never by a title the model
    /// typed: that is what stops it acting on something it merely guessed.
    #[test]
    fn every_act_tool_is_addressed_by_a_ref() {
        for tool in [
            Tool::ListReleases,
            Tool::GrabRelease,
            Tool::Regrab,
            Tool::WantIt,
            Tool::CancelDownload,
            Tool::AddToWatchlist,
        ] {
            let schema = tool.schema();
            let required = schema["function"]["parameters"]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{} has no required list", tool.name()));
            assert!(
                required.contains(&json!("ref")),
                "{} does not require a ref",
                tool.name()
            );
        }
    }
}
