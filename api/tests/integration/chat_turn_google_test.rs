//! The Google Chat half of "one assistant, two front ends".
//!
//! The Command Palette's half is `chatbot_loop_test.rs`; this is the same loop and the
//! same tool registry reaching a thread that cannot stream. Two things are being proved:
//!
//! * the `TurnEvent` consumer behaves the way a chat client needs — no post per token,
//!   one message that evolves, and never silence;
//! * a webhook event runs a **real** turn, with role gating and the write confirmation
//!   coming out of exactly the same code the web transport uses.
//!
//! The LLM is a wiremock server and the transport is a recorder. Nothing here touches
//! the GPU box or Google.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::common::*;
use api::chatbot::events::{self, Source, TurnEvent, TurnSender};
use api::chatbot::transport::turn::{CANCEL_PREFIX, CONFIRM_PREFIX, PICK_PREFIX, ThreadSink};
use api::chatbot::transport::{
    Capabilities, ChatConfigField, ChatError, ChatTransport, ChatTransportInfo, Inbound, LinkStyle,
    OutMessage, Target,
};
use async_trait::async_trait;
use axum::http::HeaderMap;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// A transport that records instead of calling Google
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Post(OutMessage),
    Edit(String, OutMessage),
}

impl Op {
    fn text(&self) -> &str {
        match self {
            Op::Post(m) | Op::Edit(_, m) => &m.text,
        }
    }

    fn message(&self) -> &OutMessage {
        match self {
            Op::Post(m) | Op::Edit(_, m) => m,
        }
    }
}

static FAKE_FIELDS: &[ChatConfigField] = &[];

static FAKE_INFO: ChatTransportInfo = ChatTransportInfo {
    id: "fake",
    display_name: "Fake",
    description: "records what the sink asked for",
    config_fields: FAKE_FIELDS,
    capabilities: Capabilities {
        edit_in_place: true,
        cards: true,
        buttons: true,
        files: false,
    },
};

struct Recorder {
    ops: Mutex<Vec<Op>>,
    capabilities: Capabilities,
    link_style: LinkStyle,
    /// Discord rejects a whole message whose buttons share a `custom_id`, and a
    /// transport that will not take the card is the only way to prove the answer
    /// inside it still arrives.
    rejects_buttons: bool,
}

impl Recorder {
    /// Defaults to Google Chat's link syntax, because this file's assertions are
    /// Google Chat's rendering. A markdown transport is [`Recorder::with_link_style`].
    fn new() -> Self {
        Self {
            ops: Mutex::new(Vec::new()),
            capabilities: FAKE_INFO.capabilities,
            link_style: LinkStyle::AnglePipe,
            rejects_buttons: false,
        }
    }

    fn with_capabilities(capabilities: Capabilities) -> Self {
        Self {
            capabilities,
            ..Self::new()
        }
    }

    fn rejecting_buttons() -> Self {
        Self {
            rejects_buttons: true,
            ..Self::new()
        }
    }

    fn with_link_style(link_style: LinkStyle) -> Self {
        Self {
            link_style,
            ..Self::new()
        }
    }

    fn ops(&self) -> Vec<Op> {
        self.ops.lock().expect("recorder").clone()
    }

    fn posts(&self) -> Vec<Op> {
        self.ops()
            .into_iter()
            .filter(|o| matches!(o, Op::Post(_)))
            .collect()
    }

    fn edits(&self) -> Vec<Op> {
        self.ops()
            .into_iter()
            .filter(|o| matches!(o, Op::Edit(..)))
            .collect()
    }

    fn last(&self) -> Op {
        self.ops().pop().expect("the sink said nothing at all")
    }
}

#[async_trait]
impl ChatTransport for Recorder {
    fn info(&self) -> &'static ChatTransportInfo {
        &FAKE_INFO
    }

    async fn verify(
        &self,
        _headers: &HeaderMap,
        _body: &[u8],
    ) -> Result<Option<Inbound>, ChatError> {
        Ok(None)
    }

    async fn post(&self, _target: &Target, msg: &OutMessage) -> Result<String, ChatError> {
        if self.rejects_buttons && msg.choices.is_some() {
            return Err(ChatError::Transport("400: duplicate custom_id".into()));
        }
        let mut ops = self.ops.lock().expect("recorder");
        ops.push(Op::Post(msg.clone()));
        Ok("spaces/AAA/messages/M1".to_string())
    }

    async fn edit(
        &self,
        _target: &Target,
        message_id: &str,
        msg: &OutMessage,
    ) -> Result<(), ChatError> {
        if self.rejects_buttons && msg.choices.is_some() {
            return Err(ChatError::Transport("400: duplicate custom_id".into()));
        }
        let mut ops = self.ops.lock().expect("recorder");
        ops.push(Op::Edit(message_id.to_string(), msg.clone()));
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn link_style(&self) -> LinkStyle {
        self.link_style
    }
}

fn target() -> Target {
    Target {
        transport: "fake".to_string(),
        space: "spaces/AAA".to_string(),
        thread: Some("spaces/AAA/threads/TTT".to_string()),
    }
}

/// Feed a turn's events through the sink. `coalesce` is injected, so nothing here
/// waits fifteen real seconds to prove a rate limit.
async fn drive(recorder: &Recorder, coalesce: Duration, script: Vec<TurnEvent>) -> Option<String> {
    let (tx, rx) = events::channel();
    tokio::spawn(async move {
        for event in script {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    ThreadSink::with_coalesce(recorder, target(), coalesce)
        .consume(rx)
        .await
}

/// Same, with the "Public URL" setting an admin may or may not have filled in.
async fn drive_with_public_url(
    recorder: &Recorder,
    public_url: Option<&str>,
    script: Vec<TurnEvent>,
) -> Option<String> {
    let (tx, rx) = events::channel();
    tokio::spawn(async move {
        for event in script {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    ThreadSink::with_coalesce(recorder, target(), NEVER)
        .with_public_url(public_url.map(str::to_string))
        .consume(rx)
        .await
}

const NEVER: Duration = Duration::from_secs(3600);
const ALWAYS: Duration = Duration::ZERO;

fn tool_started(summary: &str) -> TurnEvent {
    TurnEvent::ToolStarted {
        name: "search_library".to_string(),
        summary: summary.to_string(),
    }
}

fn tool_finished(detail: &str, outputs: Value) -> TurnEvent {
    TurnEvent::ToolFinished {
        name: "search_library".to_string(),
        detail: detail.to_string(),
        outputs,
    }
}

// =============================================================================
// The consumer
// =============================================================================

/// Chat cannot stream. A post per token would be a message per word.
#[tokio::test]
async fn tokens_never_reach_the_thread() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        ALWAYS,
        vec![
            TurnEvent::Delta("You ".to_string()),
            TurnEvent::Delta("have ".to_string()),
            TurnEvent::Delta("The Stand.".to_string()),
        ],
    )
    .await;

    // The one message is the "loop died without a verdict" backstop, never a token.
    assert_eq!(recorder.ops().len(), 1, "{:?}", recorder.ops());
    assert!(
        recorder.last().text().contains("lost that one"),
        "a delta produced a message: {:?}",
        recorder.last()
    );
}

#[tokio::test]
async fn the_first_tool_event_posts_and_everything_after_edits_it() {
    let recorder = Recorder::new();
    let message_id = drive(
        &recorder,
        ALWAYS,
        vec![
            TurnEvent::Delta("ignored".to_string()),
            tool_started("searching for carrie"),
            tool_finished("3 results", json!({"ok": true})),
            tool_started("checking the schedule for carrie"),
            TurnEvent::Done {
                text: "Carrie is in the library.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(message_id.as_deref(), Some("spaces/AAA/messages/M1"));
    let posts = recorder.posts();
    assert_eq!(posts.len(), 1, "only the first tool event posts: {posts:?}");
    assert_eq!(posts[0].text(), "Searching for carrie…");

    let edits = recorder.edits();
    assert!(edits.len() >= 2, "{edits:?}");
    for edit in &edits {
        match edit {
            Op::Edit(id, _) => {
                assert_eq!(id, "spaces/AAA/messages/M1", "a second message appeared")
            }
            Op::Post(_) => unreachable!(),
        }
    }
    assert_eq!(recorder.last().text(), "Carrie is in the library.");
}

/// The window is `watches::COALESCE_WINDOW`, and progress inside a turn honours it
/// for the same reason progress after one does: Google will not take a patch per
/// event, and a thread that rewrites itself ten times a second is unreadable.
#[tokio::test]
async fn progress_edits_are_coalesced_but_the_answer_always_lands() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            tool_started("resolving carrie"),
            tool_finished("2 possibilities", json!({"ok": true})),
            tool_started("looking for copies"),
            tool_finished("4 releases", json!({"ok": true})),
            tool_started("starting the download"),
            TurnEvent::Done {
                text: "Grabbing the 1080p one.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(recorder.posts().len(), 1, "{:?}", recorder.ops());
    assert_eq!(
        recorder.edits().len(),
        1,
        "five progress events inside one window became one edit — the answer: {:?}",
        recorder.ops()
    );
    assert_eq!(recorder.last().text(), "Grabbing the 1080p one.");
}

/// The loop's ten-second heartbeat. The Command Palette ignores it because tokens
/// are already arriving; a chat thread has nothing on screen at all, and the LLM
/// gate can be evicted by a GPU render, so this is the difference between "slow"
/// and "broken".
#[tokio::test]
async fn the_still_thinking_nudge_is_rendered_rather_than_dropped() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            TurnEvent::ToolStarted {
                name: "thinking".to_string(),
                summary: "still working on it".to_string(),
            },
            TurnEvent::Done {
                text: "Around nine on Thursday.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(recorder.posts()[0].text(), "Still working on it…");
    assert_eq!(recorder.last().text(), "Around nine on Thursday.");
    assert_eq!(recorder.edits().len(), 1, "{:?}", recorder.ops());
}

/// The commonest turn there is. No tool runs, so there is no first tool event to
/// post on, and the answer has to be the post.
#[tokio::test]
async fn a_question_with_no_tool_call_still_answers() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            TurnEvent::Delta("Around ".to_string()),
            TurnEvent::Delta("nine on Thursday.".to_string()),
            TurnEvent::Done {
                text: "Around nine on Thursday.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(recorder.ops().len(), 1, "{:?}", recorder.ops());
    assert_eq!(recorder.posts()[0].text(), "Around nine on Thursday.");
}

#[tokio::test]
async fn a_failure_is_visible_rather_than_silent() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![TurnEvent::Failed(
            "the language model is unreachable".to_string(),
        )],
    )
    .await;

    assert_eq!(recorder.ops().len(), 1);
    assert_eq!(
        recorder.last().text(),
        "The language model is unreachable",
        "silence reads as broken"
    );
}

#[tokio::test]
async fn a_loop_that_dies_without_a_verdict_still_says_something() {
    let recorder = Recorder::new();
    let (tx, rx) = events::channel();
    tx.send(tool_started("searching for carrie"))
        .await
        .expect("send");
    drop(tx);

    ThreadSink::with_coalesce(&recorder, target(), ALWAYS)
        .consume(rx)
        .await;

    assert!(
        recorder.last().text().contains("lost that one"),
        "{:?}",
        recorder.ops()
    );
}

#[tokio::test]
async fn citations_ride_the_answer_because_chat_has_no_sources_event() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            TurnEvent::Sources(vec![Source {
                item_type: "book".to_string(),
                id: "1".to_string(),
                title: "The Stand".to_string(),
                url: "/library/books/1".to_string(),
                cover_url: None,
                year: None,
                description: Some("Stephen King".to_string()),
            }]),
            TurnEvent::Done {
                text: "The Stand is here.".to_string(),
            },
        ],
    )
    .await;

    let text = recorder.last().text().to_string();
    assert!(text.starts_with("The Stand is here."), "{text}");
    assert!(text.contains("Stephen King"), "{text}");
    assert!(
        !text.contains("/library/books/1"),
        "a web path is noise on a phone: {text}"
    );
}

fn two_cited_books() -> TurnEvent {
    TurnEvent::Sources(vec![
        Source {
            item_type: "book".to_string(),
            id: "1".to_string(),
            title: "The Stand".to_string(),
            url: "/library/books/1".to_string(),
            cover_url: None,
            year: None,
            description: Some("Stephen King".to_string()),
        },
        Source {
            item_type: "movie".to_string(),
            id: "2".to_string(),
            title: "Carrie".to_string(),
            url: "/movies/2".to_string(),
            cover_url: None,
            year: Some(1976),
            description: None,
        },
    ])
}

/// The whole point of the setting: a path is dead in a phone notification, an
/// absolute URL is not. `<url|label>` is Google Chat's own hyperlink syntax for
/// message text.
#[tokio::test]
async fn a_configured_public_url_makes_each_citation_tappable() {
    let recorder = Recorder::new();
    drive_with_public_url(
        &recorder,
        Some("https://kobbler.example.com"),
        vec![
            two_cited_books(),
            TurnEvent::Done {
                text: "Two King adaptations.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(
        recorder.last().text(),
        "Two King adaptations.\n\n• <https://kobbler.example.com/library/books/1|The Stand> — Stephen King\n• <https://kobbler.example.com/movies/2|Carrie> (1976)"
    );
    assert!(
        !recorder.last().text().contains("com//"),
        "exactly one slash between base and path"
    );
}

/// The same turn on a markdown transport. Discord renders `<url|text>` as literal
/// text, so a citation block in Google Chat's syntax would put raw markup in front of
/// the user on nearly every answer — this is the end-to-end guard that each transport
/// gets the markup it actually renders.
#[tokio::test]
async fn a_markdown_transport_gets_markdown_citations_end_to_end() {
    let recorder = Recorder::with_link_style(LinkStyle::Markdown);
    drive_with_public_url(
        &recorder,
        Some("https://kobbler.example.com"),
        vec![
            two_cited_books(),
            TurnEvent::Done {
                text: "Two King adaptations.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(
        recorder.last().text(),
        "Two King adaptations.\n\n• [The Stand](https://kobbler.example.com/library/books/1) — Stephen King\n• [Carrie](https://kobbler.example.com/movies/2) (1976)"
    );
}

/// The no-regression case. Every install that has never touched the setting must
/// get the text it got before the setting existed.
#[tokio::test]
async fn without_a_public_url_the_citation_block_is_byte_identical() {
    let recorder = Recorder::new();
    drive_with_public_url(
        &recorder,
        None,
        vec![
            two_cited_books(),
            TurnEvent::Done {
                text: "Two King adaptations.".to_string(),
            },
        ],
    )
    .await;

    assert_eq!(
        recorder.last().text(),
        "Two King adaptations.\n\n• The Stand — Stephen King\n• Carrie (1976)"
    );
}

// =============================================================================
// Cards and buttons — Phase 9
// =============================================================================

fn confirm_outputs(token: &str) -> Value {
    json!({
        "ok": true,
        "pending_confirmation": true,
        "confirm_token": token,
        "confirmed": false,
        "action": "get",
        "target": "someShow S24E01",
        "say": "I'm about to get someShow S24E01 — say go ahead, or stop if that's wrong.",
    })
}

/// The sentence is the confirmation; the buttons are an addition for whoever would
/// rather tap. Losing the sentence would be losing the design constraint.
#[tokio::test]
async fn a_pending_write_arrives_as_a_sentence_with_buttons_beside_it() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            tool_started("getting that"),
            tool_finished("waiting on: get someShow S24E01", confirm_outputs("tok-9")),
            TurnEvent::Done {
                text: "I'm about to get someShow S24E01 — say go ahead, or stop if that's wrong."
                    .to_string(),
            },
        ],
    )
    .await;

    let last = recorder.last();
    assert!(
        last.text()
            .contains("say go ahead, or stop if that's wrong"),
        "the sentence must survive: {}",
        last.text()
    );
    let choices = last
        .message()
        .choices
        .as_ref()
        .expect("a pending write offers buttons");
    assert_eq!(choices.choices.len(), 2);
    assert_eq!(choices.choices[0].id, format!("{CONFIRM_PREFIX}tok-9"));
    assert_eq!(choices.choices[1].id, format!("{CANCEL_PREFIX}tok-9"));
}

/// A transport with no buttons is not a transport with no confirmation.
#[tokio::test]
async fn without_buttons_the_sentence_alone_still_asks() {
    let recorder = Recorder::with_capabilities(Capabilities {
        edit_in_place: true,
        cards: false,
        buttons: false,
        files: false,
    });
    drive(
        &recorder,
        NEVER,
        vec![
            tool_finished("waiting on: get someShow S24E01", confirm_outputs("tok-9")),
            TurnEvent::Done {
                text: "I'm about to get someShow S24E01 — say go ahead, or stop if that's wrong."
                    .to_string(),
            },
        ],
    )
    .await;

    let last = recorder.last();
    assert!(last.message().choices.is_none());
    assert!(last.text().contains("say go ahead"), "{}", last.text());
}

#[tokio::test]
async fn an_ambiguous_answer_offers_the_candidates_as_taps() {
    let recorder = Recorder::new();
    drive(
        &recorder,
        NEVER,
        vec![
            tool_finished(
                "2 possibilities",
                json!({
                    "ok": true,
                    "resolution": "ambiguous",
                    "candidates": [
                        {"ref": "movie:1", "title": "Carrie", "year": 1976},
                        {"ref": "movie:2", "title": "Carrie", "year": 2013},
                    ],
                }),
            ),
            TurnEvent::Done {
                text: "The 1976 one or the 2013 one?".to_string(),
            },
        ],
    )
    .await;

    let last = recorder.last();
    let choices = last.message().choices.as_ref().expect("candidates to tap");
    // Both are called Carrie, so the year is what each button says and means.
    assert_eq!(choices.choices[0].id, format!("{PICK_PREFIX}Carrie (1976)"));
    assert_eq!(choices.choices[0].label, "Carrie (1976)");
    assert_ne!(choices.choices[1].id, choices.choices[0].id);
}

/// The question is in the words; the buttons are a convenience on top. A
/// transport that refuses the card must not take the question with it — that is
/// how a Discord thread went silent mid-conversation, the answer lost to a 400
/// about the buttons attached to it.
#[tokio::test]
async fn an_answer_the_buttons_would_have_lost_arrives_anyway() {
    let recorder = Recorder::rejecting_buttons();
    drive(
        &recorder,
        NEVER,
        vec![
            tool_finished(
                "2 possibilities",
                json!({
                    "ok": true,
                    "resolution": "ambiguous",
                    "candidates": [
                        {"ref": "show:1", "title": "Seen", "year": 2021},
                        {"ref": "show:2", "title": "Seen", "year": 2018},
                    ],
                }),
            ),
            TurnEvent::Done {
                text: "The 2021 one or the 2018 one?".to_string(),
            },
        ],
    )
    .await;

    let last = recorder.last();
    assert_eq!(last.text(), "The 2021 one or the 2018 one?");
    assert!(
        last.message().choices.is_none(),
        "the retry drops the card that would not go",
    );
    assert!(
        matches!(last, Op::Post(_)),
        "and posts it, because the message it was editing may be the problem",
    );
}

// =============================================================================
// The webhook, end to end
// =============================================================================

const KID: &str = "kobbler-test-kid";
const PROJECT_NUMBER: &str = "123456789012";
const CHAT_ISSUER: &str = "chat@system.gserviceaccount.com";
const SPACE: &str = "spaces/AAA";
const THREAD: &str = "spaces/AAA/threads/TTT";

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

struct Fixture {
    server: TestServer,
    llm: MockServer,
    _jwks: MockServer,
}

impl Fixture {
    fn pool(&self) -> &PgPool {
        &self.server.pool
    }
}

/// One SSE body in the shape an OpenAI-compatible provider streams.
fn sse(chunks: &[Value]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn says(text: &str) -> String {
    sse(&[json!({"choices": [{"index": 0, "delta": {"content": text}}]})])
}

fn calls(name: &str, arguments: Value) -> String {
    sse(&[json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": 0, "id": "c1", "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()},
    }]}}]})])
}

/// One canned reply per round, in order. The loop asks again after every tool call,
/// so a turn that acts needs at least two.
async fn script_llm(llm: &MockServer, rounds: &[String]) {
    llm.reset().await;
    for (i, body) in rounds.iter().enumerate() {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.clone(), "text/event-stream"),
            )
            .up_to_n_times(1)
            .with_priority((i + 1) as u8)
            .mount(llm)
            .await;
    }
}

async fn completions_seen(llm: &MockServer) -> usize {
    llm.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method == wiremock::http::Method::POST)
        .filter(|r| r.url.path().ends_with("/chat/completions"))
        .count()
}

async fn fixture() -> Fixture {
    let jwks = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "keys": [{
                "kty": "RSA", "alg": "RS256", "use": "sig", "kid": KID,
                "n": SIGNING_KEY_MODULUS, "e": "AQAB",
            }]
        })))
        .mount(&jwks)
        .await;

    let llm = MockServer::start().await;
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
                    "space": SPACE,
                    "jwk_base_url": format!("{}/", jwks.uri()),
                },
                "triggers": [],
            }),
        )
        .await
        .assert_status(StatusCode::CREATED);

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "chat_enabled": true,
                "chat_llm_provider": "openai",
                "chat_llm_api_key": "test-key",
                "chat_llm_model": "mock-model",
                "chat_llm_base_url": format!("{}/v1", llm.uri()),
            }),
        )
        .await
        .assert_ok();

    Fixture {
        server,
        llm,
        _jwks: jwks,
    }
}

async fn bind(fixture: &Fixture, email: &str, role: &str, max_role: &str) -> Uuid {
    let user_id = create_user_with_role(&fixture.server, email, role).await;
    api::repositories::chat_identities::create(
        fixture.pool(),
        &api::repositories::chat_identities::NewChatIdentity {
            transport: "google_chat",
            address: Some(email),
            external_id: None,
            user_id,
            max_role,
            enabled: true,
        },
    )
    .await
    .expect("identity created");
    user_id
}

fn sign(claims: Value) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.to_string());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(SIGNING_KEY_PEM.as_bytes()).expect("key");
    jsonwebtoken::encode(&header, &claims, &key).expect("signs")
}

fn jwt(email: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp() as i64;
    sign(json!({
        "iss": CHAT_ISSUER, "aud": PROJECT_NUMBER, "sub": "users/1234",
        "iat": now - 10, "exp": now + 600, "email": email,
    }))
}

fn sender(email: &str) -> Value {
    json!({"name": "users/1234", "displayName": "Her", "email": email, "type": "HUMAN"})
}

fn message_event(email: &str, text: &str) -> Value {
    json!({
        "type": "MESSAGE",
        "space": {"name": SPACE, "type": "ROOM"},
        "user": sender(email),
        "message": {
            "name": "spaces/AAA/messages/MMM",
            "sender": sender(email),
            "text": format!("@kobbler {text}"),
            "argumentText": format!(" {text}"),
            "thread": {"name": THREAD},
        },
    })
}

async fn post_event(fixture: &Fixture, email: &str, event: &Value) -> TestResponse {
    TestClient::new(&fixture.server.api_url)
        .with_auth(&jwt(email))
        .post_json("/api/v1/chat/google_chat/webhook", event)
        .await
}

/// The webhook has already answered by the time the turn starts, so every assertion
/// about what the turn did has to wait for it.
async fn wait_for_turns(pool: &PgPool, wanted: i64) {
    for _ in 0..200 {
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_messages WHERE role = 'assistant' AND status <> 'running'",
        )
        .fetch_one(pool)
        .await
        .expect("count");
        if done >= wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the turn never finished");
}

async fn answers(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT content FROM chat_messages WHERE role = 'assistant' ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .expect("answers")
}

async fn user_turns(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT content FROM chat_messages WHERE role = 'user' ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .expect("user turns")
}

async fn session_address(pool: &PgPool) -> (String, String, Option<String>) {
    sqlx::query_as("SELECT transport, space, thread FROM chat_sessions LIMIT 1")
        .fetch_one(pool)
        .await
        .expect("a session was opened at the chat address")
}

async fn create_episode(fixture: &Fixture) -> Uuid {
    let resp = fixture
        .server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({"title": "someShow", "year": 2024}),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let show: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let season = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '24', 24, true, false)",
    )
    .bind(season)
    .bind(show)
    .execute(fixture.pool())
    .await
    .unwrap();

    let episode = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2, '1', 1, 'Episode 1')",
    )
    .bind(episode)
    .bind(season)
    .execute(fixture.pool())
    .await
    .unwrap();
    episode
}

async fn want_status(pool: &PgPool, episode: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT want_status FROM tv_episodes WHERE id = $1")
        .bind(episode)
        .fetch_one(pool)
        .await
        .expect("episode")
}

// -----------------------------------------------------------------------------

/// The headline: the same loop, reached from a chat address instead of a browser.
#[tokio::test]
async fn a_message_runs_a_real_turn_at_the_chat_address() {
    let fixture = fixture().await;
    bind(&fixture, "her@example.com", "admin", "contributor").await;
    script_llm(
        &fixture.llm,
        &[
            calls("library_stats", json!({})),
            says("You have 412 books."),
        ],
    )
    .await;

    post_event(
        &fixture,
        "her@example.com",
        &message_event("her@example.com", "how many books?"),
    )
    .await
    .assert_ok();
    wait_for_turns(fixture.pool(), 1).await;

    assert_eq!(answers(fixture.pool()).await, vec!["You have 412 books."]);
    assert_eq!(
        session_address(fixture.pool()).await,
        (
            "google_chat".to_string(),
            SPACE.to_string(),
            Some(THREAD.to_string())
        ),
        "space + thread is the conversation key the idle sweep also looks for",
    );
}

/// No tool call means no first tool event, and this is the commonest turn there is.
#[tokio::test]
async fn a_plain_question_answers_without_calling_a_tool() {
    let fixture = fixture().await;
    bind(&fixture, "her@example.com", "contributor", "contributor").await;
    script_llm(&fixture.llm, &[says("Around nine on Thursday.")]).await;

    post_event(
        &fixture,
        "her@example.com",
        &message_event("her@example.com", "when's the new one on?"),
    )
    .await
    .assert_ok();
    wait_for_turns(fixture.pool(), 1).await;

    assert_eq!(
        answers(fixture.pool()).await,
        vec!["Around nine on Thursday."]
    );
}

/// An event Google did not thread still needs a conversation key, or the second
/// message in a DM opens a second session and forgets the first.
#[tokio::test]
async fn an_unthreaded_dm_still_keys_a_conversation() {
    let fixture = fixture().await;
    bind(&fixture, "her@example.com", "contributor", "contributor").await;
    let unthreaded = json!({
        "type": "MESSAGE",
        "space": {"name": SPACE, "type": "DM"},
        "user": sender("her@example.com"),
        "message": {"sender": sender("her@example.com"), "text": "hello"},
    });

    script_llm(&fixture.llm, &[says("Hello."), says("Still here.")]).await;
    post_event(&fixture, "her@example.com", &unthreaded)
        .await
        .assert_ok();
    wait_for_turns(fixture.pool(), 1).await;
    post_event(&fixture, "her@example.com", &unthreaded)
        .await
        .assert_ok();
    wait_for_turns(fixture.pool(), 2).await;

    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions")
        .fetch_one(fixture.pool())
        .await
        .expect("count");
    assert_eq!(sessions, 1, "an unthreaded DM forked its own conversation");
    assert_eq!(
        session_address(fixture.pool()).await,
        (
            "google_chat".to_string(),
            SPACE.to_string(),
            Some(SPACE.to_string())
        ),
        "with no thread the space is the only stable key left",
    );
}

/// An unmapped sender is one sentence and nothing else. Burning GPU time for a
/// stranger in a space is the failure this guards.
#[tokio::test]
async fn an_unmapped_sender_gets_a_sentence_and_no_llm_call() {
    let fixture = fixture().await;
    script_llm(&fixture.llm, &[says("never reached")]).await;

    let response = post_event(
        &fixture,
        "stranger@example.com",
        &message_event("stranger@example.com", "download everything"),
    )
    .await;
    response.assert_ok();
    let text = response.json::<Value>()["text"]
        .as_str()
        .expect("one sentence")
        .to_string();
    assert!(text.contains("not set up"), "{text}");

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        completions_seen(&fixture.llm).await,
        0,
        "an unknown address reached the language model"
    );
    assert!(answers(fixture.pool()).await.is_empty());
}

/// Role gating is `dispatch`'s, unchanged, reached with the `AuthUser` the identity
/// resolver built. The refusal has to land before the write, not after it.
#[tokio::test]
async fn a_viewer_is_refused_an_act_tool_over_chat() {
    let fixture = fixture().await;
    bind(&fixture, "kid@example.com", "viewer", "contributor").await;
    let episode = create_episode(&fixture).await;
    script_llm(
        &fixture.llm,
        &[
            calls("want_it", json!({"ref": format!("episode:{episode}")})),
            says("You're not allowed to do that, sorry."),
        ],
    )
    .await;

    post_event(
        &fixture,
        "kid@example.com",
        &message_event("kid@example.com", "get season 24"),
    )
    .await
    .assert_ok();
    wait_for_turns(fixture.pool(), 1).await;

    assert_eq!(
        want_status(fixture.pool(), episode).await,
        "none",
        "a refused tool started a download",
    );
    let steps: Vec<Value> =
        sqlx::query_scalar("SELECT steps FROM chat_messages ORDER BY created_at")
            .fetch_all(fixture.pool())
            .await
            .expect("steps");
    let refusal = steps
        .iter()
        .filter_map(|s| s.as_array())
        .flatten()
        .find(|s| s["tool"] == json!("want_it"))
        .expect("the call was attempted");
    assert_eq!(refusal["state"], json!("failed"));
    assert!(
        refusal["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("permission"),
        "{refusal}"
    );
}

/// The two clocks. Google gives the webhook ~30 s; the turn behind it may take as
/// long as it likes.
#[tokio::test]
async fn the_ack_returns_immediately_and_the_turn_runs_behind_it() {
    let fixture = fixture().await;
    bind(&fixture, "her@example.com", "contributor", "contributor").await;
    fixture.llm.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(says("Took a while."), "text/event-stream")
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&fixture.llm)
        .await;

    let started = Instant::now();
    let response = post_event(
        &fixture,
        "her@example.com",
        &message_event("her@example.com", "anything good on?"),
    )
    .await;
    let acked = started.elapsed();
    response.assert_ok();

    assert_eq!(response.json::<Value>()["text"], json!("Looking…"));
    assert!(
        acked < Duration::from_secs(2),
        "the ack waited for the turn: {acked:?}",
    );

    wait_for_turns(fixture.pool(), 1).await;
    assert_eq!(answers(fixture.pool()).await, vec!["Took a while."]);
}
