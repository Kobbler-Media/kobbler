//! A Kobload download for a wanted book is only filed once the file agrees it
//! is that book. Otherwise the release is rejected for good and the book is
//! searched for again. A release someone picked by hand is filed as it is.
//! Regression: a want for "Michael" by Bob Mortimer filed the Danish "Mumien"
//! by Michael Katz Krefeld.

use crate::common::audiobooks::{Audio, write_audio};
use crate::common::*;
use api::jobs::handlers::LibraryVerifyHandler;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RELEASE: &str = "Michael.2025.RETAiL.ePub.eBOOK-GRP";

enum File {
    Epub {
        title: &'static str,
        creator: Option<&'static str>,
        date: Option<&'static str>,
        language: &'static str,
        text: &'static str,
    },
    Mp3 {
        album: &'static str,
        author: Option<&'static str>,
    },
}

const DANISH_NOVEL: File = File::Epub {
    title: "Mumien",
    creator: Some("Michael Katz Krefeld"),
    date: Some("2026-01-05"),
    language: "da",
    text: "Det var en mørk nat.",
};

const THE_BOOK: File = File::Epub {
    title: "Michael",
    creator: Some("Bob Mortimer"),
    date: Some("2025-06-05"),
    language: "en",
    text: "It was a dark night.",
};

impl File {
    fn bytes(&self) -> (Vec<u8>, &'static str) {
        match self {
            File::Epub {
                title,
                creator,
                date,
                language,
                text,
            } => {
                let chapter = chapter_xhtml("One", &[text]);
                (
                    epub_book(title, *creator, *date, language, &[chapter]),
                    "epub",
                )
            }
            File::Mp3 { album, author } => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("book.mp3");
                let mut tags = vec![("album", *album), ("title", *album)];
                tags.extend(author.map(|a| ("album_artist", a)));
                write_audio(&path, &Audio::Tone(2.0), &tags);
                (std::fs::read(&path).unwrap(), "mp3")
            }
        }
    }
}

struct Delivery<'a> {
    file: &'a File,
    hand_picked: bool,
    /// What speech-to-text hears at the start of the narration.
    narration: Option<&'static str>,
}

struct Rig {
    server: TestServer,
    kobload: MockServer,
    book_id: Uuid,
}

/// A want for "Michael" by Bob Mortimer (2025), and another of his books in
/// the library.
async fn rig(audiobook: bool) -> Rig {
    let kobload = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    Mock::given(method("POST"))
        .and(path_regex(
            r"^/api/v1/(downloads/.*/imported|want/.*/confirm)$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "deleted" })))
        .mount(&kobload)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/want"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&kobload)
        .await;

    server
        .admin_client()
        .post_json(
            "/api/v1/library/books",
            &json!({ "title": "The Satsuma Complex", "authors": [{ "name": "Bob Mortimer" }] }),
        )
        .await
        .assert_success();
    let created = server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "Michael",
                "author": "Bob Mortimer",
                "want_format": if audiobook { "audiobook" } else { "ebook" },
            }),
        )
        .await;
    created.assert_ok();
    let book_id: Uuid = created.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE books SET pubdate = '2025-06-05' WHERE id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();
    Rig {
        server,
        kobload,
        book_id,
    }
}

impl Rig {
    async fn deliver(&self, delivery: &Delivery<'_>) -> bool {
        let download_id = Uuid::now_v7();
        if delivery.hand_picked {
            Mock::given(method("POST"))
                .and(path("/api/v1/downloads"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({ "id": download_id, "status": "queued" })),
                )
                .mount(&self.kobload)
                .await;
            let resp = self
                .server
                .admin_client()
                .post_json(
                    &format!("/api/v1/admin/books/{}/queue", self.book_id),
                    &json!({
                        "source": "manual",
                        "release": {
                            "id": "guid:hand-picked",
                            "protocol": "usenet",
                            "download_url": "https://example.com/download",
                            "title": RELEASE,
                            "indexer": "MockIndexer",
                            "size": 1_000,
                        }
                    }),
                )
                .await;
            assert_eq!(resp.status, reqwest::StatusCode::NO_CONTENT);
        }
        if let Some(heard) = delivery.narration {
            let stt = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/audio/transcriptions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "text": heard,
                    "segments": [{ "start": 0.0, "end": 5.0, "text": heard }],
                })))
                .mount(&stt)
                .await;
            self.server
                .admin_client()
                .patch_json(
                    "/api/v1/admin/config",
                    &json!({ "stt_service_url": stt.uri(), "route_gpu_via_koblem": false }),
                )
                .await
                .assert_ok();
            // The mock must outlive the delivery.
            std::mem::forget(stt);
        }

        let (bytes, extension) = delivery.file.bytes();
        Mock::given(method("GET"))
            .and(path_regex(format!(
                r"/api/v1/downloads/{download_id}/files/.*"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&self.kobload)
            .await;
        TestClient::new(&self.server.api_url)
            .post_json(
                &format!("/api/v1/ingest/{}", self.server.ingest_token),
                &json!({
                    "download_id": download_id,
                    "status": "completed",
                    "metadata": { "book_id": self.book_id },
                    "release_info": { "title": RELEASE },
                    "files": [{
                        "path": format!("book.{extension}"),
                        "size": 1_000,
                        "extension": extension,
                    }],
                }),
            )
            .await
            .assert_success();
        self.server.process_pending_ingest_jobs().await;
        self.filed().await
    }

    async fn filed(&self) -> bool {
        for _ in 0..40 {
            let filed: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM book_files WHERE book_id = $1)
                     OR EXISTS (SELECT 1 FROM audiobook_files WHERE book_id = $1)",
            )
            .bind(self.book_id)
            .fetch_one(&self.server.pool)
            .await
            .unwrap();
            if filed {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    }

    async fn rejected_and_searching_again(&self) -> bool {
        let (rejected, status): (Value, String) =
            sqlx::query_as("SELECT rejected_releases, want_status FROM books WHERE id = $1")
                .bind(self.book_id)
                .fetch_one(&self.server.pool)
                .await
                .unwrap();
        let rejected = rejected
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["release_title"] == RELEASE);
        rejected && status == "wanted"
    }
}

#[tokio::test]
async fn only_the_wanted_book_is_filed() {
    let epub = |file| Delivery {
        file,
        hand_picked: false,
        narration: None,
    };
    let cases = [
        ("the Danish novel", epub(&DANISH_NOVEL), false),
        ("the book", epub(&THE_BOOK), true),
        (
            "no creator, author in the opening",
            epub(&File::Epub {
                title: "Michael",
                creator: None,
                date: None,
                language: "en",
                text: "Copyright Bob Mortimer 2025.",
            }),
            true,
        ),
        (
            "no creator, author nowhere",
            epub(&File::Epub {
                title: "Michael",
                creator: None,
                date: None,
                language: "en",
                text: "It was a dark night.",
            }),
            false,
        ),
        (
            "another of the author's books",
            epub(&File::Epub {
                title: "The Satsuma Complex",
                creator: Some("Bob Mortimer"),
                date: None,
                language: "en",
                text: "It was a dark night.",
            }),
            false,
        ),
        (
            "his book, dated before the library's edition",
            epub(&File::Epub {
                title: "Michael",
                creator: Some("Bob Mortimer"),
                date: Some("2001-01-01"),
                language: "en",
                text: "It was a dark night.",
            }),
            true,
        ),
        (
            "his book, in Danish",
            epub(&File::Epub {
                title: "Michael",
                creator: Some("Bob Mortimer"),
                date: Some("2025-06-05"),
                language: "da",
                text: "Det var en mørk nat.",
            }),
            false,
        ),
        (
            "the Danish novel, picked by hand",
            Delivery {
                file: &DANISH_NOVEL,
                hand_picked: true,
                narration: None,
            },
            true,
        ),
    ];
    for (name, delivery, wanted) in cases {
        let rig = rig(false).await;
        let filed = rig.deliver(&delivery).await;
        assert_eq!(filed, wanted, "{name}: filed={filed}");
        if !wanted {
            assert!(
                rig.rejected_and_searching_again().await,
                "{name}: release rejected and the book searched for again"
            );
        }
    }
}

/// Library metadata often names a translation's language; a file in the
/// household's own language is still the book.
#[tokio::test]
async fn a_file_in_the_household_language_is_filed_whatever_the_book_says() {
    let rig = rig(false).await;
    sqlx::query("UPDATE books SET language = 'de' WHERE id = $1")
        .bind(rig.book_id)
        .execute(&rig.server.pool)
        .await
        .unwrap();
    assert!(
        rig.deliver(&Delivery {
            file: &THE_BOOK,
            hand_picked: false,
            narration: None,
        })
        .await
    );
}

#[tokio::test]
async fn only_the_wanted_audiobook_is_filed() {
    let mp3 = |album, author, narration| Delivery {
        file: Box::leak(Box::new(File::Mp3 { album, author })),
        hand_picked: false,
        narration,
    };
    let cases = [
        (
            "by someone else",
            mp3("Mumien", Some("Michael Katz Krefeld"), None),
            false,
        ),
        (
            "the audiobook",
            mp3("Michael", Some("Bob Mortimer"), None),
            true,
        ),
        (
            "untagged, narration names him",
            mp3("Michael", None, Some("Michael, written by Bob Mortimer.")),
            true,
        ),
        (
            "untagged, narration names someone else",
            mp3("Michael", None, Some("Mumien, af Michael Katz Krefeld.")),
            false,
        ),
    ];
    for (name, delivery, wanted) in cases {
        let rig = rig(true).await;
        let filed = rig.deliver(&delivery).await;
        assert_eq!(filed, wanted, "{name}: filed={filed}");
    }
}

/// Files already in the library are checked the same way, and only reported.
#[tokio::test]
async fn library_verification_lists_files_that_disagree() {
    let rig = rig(false).await;
    assert!(
        rig.deliver(&Delivery {
            file: &DANISH_NOVEL,
            hand_picked: true,
            narration: None,
        })
        .await
    );
    let started = rig
        .server
        .admin_client()
        .post_json("/api/v1/admin/books/verify-library", &json!({}))
        .await;
    assert_eq!(started.status, reqwest::StatusCode::ACCEPTED);
    let job_id = started.json::<Value>()["job_id"]
        .as_str()
        .unwrap()
        .to_string();

    rig.server
        .process_pending_jobs(&LibraryVerifyHandler::new(
            rig.server.pool.clone(),
            rig.server.runtime_config.clone(),
            rig.server.app_state.job_events.clone(),
            &rig.server.storage_root(),
        ))
        .await;
    let result: Value = sqlx::query_scalar("SELECT result FROM job_status WHERE id = $1::uuid")
        .bind(&job_id)
        .fetch_one(&rig.server.pool)
        .await
        .unwrap();
    let mismatches = result["mismatches"].as_array().unwrap();
    assert_eq!(mismatches.len(), 1, "{result}");
    assert_eq!(mismatches[0]["title"], "Michael");
    assert!(
        mismatches[0]["reason"]
            .as_str()
            .unwrap()
            .contains("Michael Katz Krefeld"),
        "{result}"
    );
    assert!(rig.filed().await, "verification only reports");
}

/// Formats other than EPUB are read too: their metadata, and the opening of
/// their text when the metadata names nobody. Calibre writes the fixtures, as
/// it writes such files in the wild ("Unknown" where there is no author).
#[tokio::test]
async fn other_formats_are_read_too() {
    use api::wanted::verify::{Expected, Inspector, Verdict, inspect, judge};

    let dir = tempfile::tempdir().unwrap();
    let chapter = chapter_xhtml("One", &["Copyright Bob Mortimer 2025."]);
    let epub = dir.path().join("michael.epub");
    std::fs::write(&epub, epub_book("Michael", None, None, "en", &[chapter])).unwrap();
    let expected = Expected {
        title: "Michael".into(),
        alternates: vec![],
        others: vec![],
        authors: vec!["Bob Mortimer".into()],
        year: Some(2025),
        language: Some("english".into()),
    };
    let inspector = Inspector {
        stt_url: None,
        scratch_dir: dir.path().to_path_buf(),
    };
    for ext in ["azw3", "pdf"] {
        let converted = dir.path().join(format!("michael.{ext}"));
        let status = std::process::Command::new("ebook-convert")
            .arg(&epub)
            .arg(&converted)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("ebook-convert (Calibre) is needed to write the fixtures");
        assert!(status.success(), "{ext}: ebook-convert failed");
        let found = inspect(&converted, &expected.authors, &inspector)
            .await
            .unwrap_or_else(|| panic!("{ext}: unreadable"));
        assert_eq!(found.mentions_author, Some(true), "{ext}: {found:?}");
        assert_eq!(
            judge(&expected, &found),
            Verdict::Matches,
            "{ext}: {found:?}"
        );
    }
}
