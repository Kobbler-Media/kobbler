//! The download ahead walking past a refused release, against a kobload that behaves like
//! the real one: every search re-ranks, and an NZB that was never imported is only grabbed
//! warm if it is what the latest search would auto-play.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::jobs::JobResult;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, Request, Respond, ResponseTemplate};

const OTHER_PROVIDER: &str = "00000000-0000-0000-0000-000000000002";
const FILENAME: &str = "ep.mkv";
const QUICK_CHECK: &str = "Debrid error: Usenet release is not cached: a quick check found 3 of 64 articles missing (missing_articles)";
const TORRENT_REFUSED: &str =
    "Debrid error: Real-Debrid refused this release. Try a different source.";
const NZB_GONE: &str =
    "Debrid error: Usenet release is not cached: the indexer no longer has its NZB (404 Not Found)";
const WARM_SKIPPED: &str = "Debrid error: Not warmed: only the release a search would auto-play is grabbed from the indexer ahead of a tap";
const TIMED_OUT: &str = "Debrid error: Timed out waiting for usenet download";
const RATE_LIMITED: &str = "Debrid error: Failed to add magnet: Rate limited (retry after 5s)";
const AUTH_FAILED: &str = "Debrid error: Failed to add magnet: Authentication failed";

#[derive(Clone, Copy)]
enum Answer {
    Serves,
    ServesUncached,
    ServesNotVideo,
    Demoted(&'static str),
    Undemoted(&'static str),
}

#[derive(Clone, Copy)]
struct Spec {
    answer: Answer,
    imported: bool,
    same_nzb_as: Option<usize>,
}

fn fresh(answer: Answer) -> Spec {
    Spec {
        answer,
        imported: false,
        same_nzb_as: None,
    }
}

struct Listed {
    source: Source,
    provider: &'static str,
    guid: String,
    listed_key: Option<String>,
    key: String,
    answer: Answer,
    failed: bool,
}

#[derive(Default)]
struct Kobload {
    listed: Vec<Listed>,
    auto_play: HashSet<usize>,
    resolved: Vec<usize>,
    grabs: Vec<usize>,
    template: Value,
    stream: Value,
}

impl Kobload {
    fn search(&mut self) -> Value {
        let mut order: Vec<usize> = (0..self.listed.len()).collect();
        order.sort_by_key(|i| self.listed[*i].failed);
        if let Some(pick) = order.iter().find(|i| !self.listed[**i].failed)
            && self.listed[*pick].source == Source::Usenet
        {
            self.auto_play.insert(*pick);
        }
        let options: Vec<Value> = order.iter().map(|i| self.option(*i)).collect();
        json!({ "options": options })
    }

    fn option(&self, i: usize) -> Value {
        let l = &self.listed[i];
        let mut option = self.template.clone();
        let fields = option.as_object_mut().unwrap();
        fields.insert("id".into(), json!(format!("opt-{i}")));
        fields.insert("release_title".into(), json!(format!("Ahead.S01E02.{i}")));
        fields.insert("release_key".into(), json!(l.listed_key));
        fields.insert("source_type".into(), json!(l.source.source_type()));
        fields.insert("debrid_provider_id".into(), json!(l.provider));
        if l.source == Source::Usenet {
            fields.insert(
                "nzb_ref".into(),
                json!({ "indexer_id": "7", "guid": l.guid }),
            );
        }
        if l.failed {
            fields.insert("playback".into(), json!("failed"));
            fields.insert("cached".into(), json!(false));
            fields.insert("availability".into(), json!("uncached"));
        }
        option
    }

    fn find(&self, request: &Value) -> usize {
        let provider = request["debrid_provider_id"].as_str().unwrap();
        self.listed
            .iter()
            .position(|l| {
                l.provider == provider
                    && match request["nzb_ref"]["guid"].as_str() {
                        Some(guid) => l.guid == guid,
                        None => request["release_key"].as_str() == Some(l.key.as_str()),
                    }
            })
            .unwrap_or_else(|| panic!("unlisted release: {request}"))
    }

    fn resolve(&mut self, request: &Value) -> ResponseTemplate {
        let i = self.find(request);
        self.resolved.push(i);
        let usenet = self.listed[i].source == Source::Usenet;
        let unimported = usenet && self.listed[i].listed_key.is_none();
        if unimported && !self.auto_play.contains(&i) {
            return ResponseTemplate::new(500).set_body_string(WARM_SKIPPED);
        }
        if unimported {
            self.grabs.push(i);
        }
        let key = self.listed[i].key.clone();
        let mut served = self.stream.clone();
        served["release_key"] = json!(key);
        match self.listed[i].answer {
            Answer::Serves => {}
            Answer::ServesUncached => served["cached"] = json!(false),
            Answer::ServesNotVideo => served["filename"] = json!("setup.exe"),
            Answer::Demoted(message) => {
                self.listed[i].listed_key = Some(key);
                self.listed[i].failed = true;
                return ResponseTemplate::new(500).set_body_string(message);
            }
            Answer::Undemoted(message) => {
                return ResponseTemplate::new(500).set_body_string(message);
            }
        }
        if usenet {
            self.listed[i].listed_key = Some(key);
        }
        ResponseTemplate::new(200).set_body_json(served)
    }
}

#[derive(Clone)]
struct Shared(Arc<Mutex<Kobload>>);

impl Respond for Shared {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut kobload = self.0.lock().unwrap();
        if request.url.path().ends_with("/stream/search") {
            return ResponseTemplate::new(200).set_body_json(kobload.search());
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        kobload.resolve(&body)
    }
}

struct Walk {
    rig: Rig,
    kobload: Arc<Mutex<Kobload>>,
    user: Uuid,
    episode: Uuid,
}

fn key_of(source: Source, i: usize) -> String {
    match source {
        Source::Torrent => format!("{:040x}", i + 1),
        Source::Usenet => format!("nzb-{:064x}", i + 1),
    }
}

impl Walk {
    async fn start(sources: &[(Source, Spec)]) -> Self {
        let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), FILENAME).await;
        let listed = sources
            .iter()
            .enumerate()
            .map(|(i, (source, spec))| {
                let root = spec.same_nzb_as.unwrap_or(i);
                let key = key_of(*source, root);
                Listed {
                    source: *source,
                    provider: if spec.same_nzb_as.is_some() {
                        OTHER_PROVIDER
                    } else {
                        PROVIDER_ID
                    },
                    guid: format!("guid-{root}"),
                    listed_key: (*source == Source::Torrent || spec.imported).then(|| key.clone()),
                    key,
                    answer: spec.answer,
                    failed: false,
                }
            })
            .collect();
        let kobload = Arc::new(Mutex::new(Kobload {
            listed,
            template: rig.option("template"),
            stream: json!({
                "stream_url": rig.upstream.url(FILENAME),
                "filename": FILENAME,
                "filesize": rig.upstream.size(),
                "cached": true,
            }),
            ..Default::default()
        }));
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/(search|resolve)$"))
            .respond_with(Shared(kobload.clone()))
            .with_priority(1)
            .mount(&rig.kobload)
            .await;
        let user = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
        let episode = season_of(&rig.server, 2).await[1];
        Self {
            rig,
            kobload,
            user,
            episode,
        }
    }

    async fn of(source: Source, specs: &[Spec]) -> Self {
        let sources: Vec<_> = specs.iter().map(|s| (source, *s)).collect();
        Self::start(&sources).await
    }

    async fn run(&self) -> JobResult<Value> {
        DebridDownloadAheadHandler::new(self.rig.server.app_state.clone())
            .execute(
                Uuid::now_v7(),
                json!({ "user_id": self.user, "episode_id": self.episode }),
                CancellationToken::new(),
            )
            .await
    }

    fn resolved(&self) -> Vec<usize> {
        self.kobload.lock().unwrap().resolved.clone()
    }

    fn grabs(&self) -> Vec<usize> {
        self.kobload.lock().unwrap().grabs.clone()
    }

    fn downloaded(&self, i: usize) -> bool {
        let key = self.kobload.lock().unwrap().listed[i].key.clone();
        std::fs::read(self.rig.cache_file(&key)).ok().as_deref() == Some(ITA_ENG_FIXTURE)
    }
}

fn refusal(source: Source) -> &'static str {
    match source {
        Source::Torrent => TORRENT_REFUSED,
        Source::Usenet => QUICK_CHECK,
    }
}

#[tokio::test]
async fn a_refused_top_pick_downloads_the_second() {
    for source in Source::ALL {
        let walk = Walk::of(
            source,
            &[
                fresh(Answer::Demoted(refusal(source))),
                fresh(Answer::Serves),
                fresh(Answer::Serves),
            ],
        )
        .await;
        let out = walk.run().await.unwrap();
        assert!(out.get("session_id").is_some(), "{source:?}: {out}");
        assert_eq!(walk.resolved(), [0, 1], "{source:?}");
        assert!(walk.downloaded(1), "{source:?}: second option not on disk");
        if source == Source::Usenet {
            assert_eq!(walk.grabs(), [0, 1], "one grab per attempt");
        }
    }
}

#[tokio::test]
async fn three_refusals_skip_and_leave_the_fourth_alone() {
    for source in Source::ALL {
        let bad = fresh(Answer::Demoted(refusal(source)));
        let walk = Walk::of(source, &[bad, bad, bad, fresh(Answer::Serves)]).await;
        let out = walk.run().await.unwrap();
        let reason = match source {
            Source::Torrent => "blocked",
            Source::Usenet => "not_cached",
        };
        assert_eq!(out["skipped"], reason, "{source:?}: {out}");
        assert_eq!(walk.resolved(), [0, 1, 2], "{source:?}");
        assert!(!walk.downloaded(3), "{source:?}");
        if source == Source::Usenet {
            assert_eq!(walk.grabs(), [0, 1, 2], "one grab per attempt");
        }
    }
}

#[tokio::test]
async fn errors_that_say_nothing_about_the_release_do_not_walk() {
    for source in Source::ALL {
        for message in [TIMED_OUT, RATE_LIMITED, AUTH_FAILED] {
            let walk = Walk::of(
                source,
                &[fresh(Answer::Undemoted(message)), fresh(Answer::Serves)],
            )
            .await;
            let err = walk.run().await.unwrap_err();
            assert!(err.is_retryable(), "{source:?} {message}: {err}");
            assert_eq!(walk.resolved(), [0], "{source:?} {message}");
            assert!(!walk.downloaded(1), "{source:?} {message}");
        }
    }
}

#[tokio::test]
async fn a_resolved_release_that_cannot_be_kept_walks_on() {
    for answer in [Answer::ServesUncached, Answer::ServesNotVideo] {
        let walk = Walk::of(Source::Torrent, &[fresh(answer), fresh(Answer::Serves)]).await;
        let out = walk.run().await.unwrap();
        assert!(out.get("session_id").is_some(), "{out}");
        assert_eq!(walk.resolved(), [0, 1]);
        assert!(walk.downloaded(1));
    }
}

/// A refusal kobload does not rank down leaves that release as the auto-play pick, so an
/// NZB never imported cannot be grabbed: the walk passes it for one that needs no grab.
#[tokio::test]
async fn an_undemoted_refusal_spends_no_grab_on_the_gated_option() {
    let imported = Spec {
        imported: true,
        ..fresh(Answer::Serves)
    };
    let walk = Walk::of(
        Source::Usenet,
        &[
            fresh(Answer::Undemoted(NZB_GONE)),
            fresh(Answer::Serves),
            imported,
        ],
    )
    .await;
    let out = walk.run().await.unwrap();
    assert!(out.get("session_id").is_some(), "{out}");
    assert_eq!(walk.resolved(), [0, 2]);
    assert_eq!(walk.grabs(), [0]);
    assert!(walk.downloaded(2));
}

#[tokio::test]
async fn a_torrent_refusal_never_grabs_a_gated_nzb_behind_it() {
    let walk = Walk::start(&[
        (Source::Torrent, fresh(Answer::Undemoted(TORRENT_REFUSED))),
        (Source::Usenet, fresh(Answer::Serves)),
    ])
    .await;
    let out = walk.run().await.unwrap();
    assert_eq!(out["skipped"], "blocked", "{out}");
    assert_eq!(walk.resolved(), [0]);
    assert!(walk.grabs().is_empty());
}

#[tokio::test]
async fn the_same_nzb_on_another_provider_is_not_tried_again() {
    let imported = Spec {
        imported: true,
        ..fresh(Answer::Serves)
    };
    let twin = Spec {
        same_nzb_as: Some(0),
        ..fresh(Answer::Serves)
    };
    let walk = Walk::of(
        Source::Usenet,
        &[fresh(Answer::Undemoted(NZB_GONE)), twin, imported],
    )
    .await;
    let out = walk.run().await.unwrap();
    assert!(out.get("session_id").is_some(), "{out}");
    assert_eq!(walk.resolved(), [0, 2]);
}

#[tokio::test]
async fn prewarm_moves_on_at_its_next_request() {
    for source in Source::ALL {
        let walk = Walk::of(
            source,
            &[
                fresh(Answer::Demoted(refusal(source))),
                fresh(Answer::Serves),
            ],
        )
        .await;
        let body = json!({
            "media_type": "episode",
            "media_id": walk.episode,
            "capabilities": direct_play_caps(),
        });
        let client = walk.rig.server.admin_client();
        client
            .post_json("/api/v1/admin/stream/prewarm", &body)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        client
            .post_json("/api/v1/admin/stream/prewarm", &body)
            .await
            .assert_ok();
        assert_eq!(walk.resolved().first(), Some(&0), "{source:?}");
        assert!(walk.resolved().contains(&1), "{source:?}");
    }
}
