//! A Kobload download for a wanted book is only filed once the file agrees it
//! is that book. Otherwise the release is rejected for good and the book is
//! searched for again. Regression: a want for "Michael" by Bob Mortimer filed
//! the Danish "Mumien" by Michael Katz Krefeld.

use crate::common::audiobooks::{Audio, write_audio};
use crate::common::*;
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
        text: &'static str,
    },
    Mp3 {
        album: &'static str,
        author: &'static str,
    },
}

impl File {
    fn bytes(&self) -> (Vec<u8>, &'static str) {
        match self {
            File::Epub {
                title,
                creator,
                date,
                text,
            } => {
                let chapter = chapter_xhtml("One", &[text]);
                (epub_book(title, *creator, *date, &[chapter]), "epub")
            }
            File::Mp3 { album, author } => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("book.mp3");
                write_audio(
                    &path,
                    &Audio::Tone(2.0),
                    &[("album", album), ("title", album), ("album_artist", author)],
                );
                (std::fs::read(&path).unwrap(), "mp3")
            }
        }
    }
}

/// Delivers `file` for a want of "Michael" by Bob Mortimer (2025) and reports
/// whether it was filed.
async fn deliver(file: &File) -> (TestServer, Uuid, bool) {
    let kobload = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;

    server
        .admin_client()
        .post_json(
            "/api/v1/library/books",
            &json!({ "title": "The Satsuma Complex", "authors": [{ "name": "Bob Mortimer" }] }),
        )
        .await
        .assert_success();
    let audiobook = matches!(file, File::Mp3 { .. });
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

    let download_id = Uuid::now_v7();
    let (bytes, extension) = file.bytes();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/api/v1/downloads/{download_id}/files/.*"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&kobload)
        .await;
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

    TestClient::new(&server.api_url)
        .post_json(
            &format!("/api/v1/ingest/{}", server.ingest_token),
            &json!({
                "download_id": download_id,
                "status": "completed",
                "metadata": { "book_id": book_id },
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
    server.process_pending_ingest_jobs().await;

    let mut filed = false;
    for _ in 0..40 {
        filed = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM book_files WHERE book_id = $1)
                 OR EXISTS (SELECT 1 FROM audiobook_files WHERE book_id = $1)",
        )
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        if filed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    (server, book_id, filed)
}

#[tokio::test]
async fn only_the_wanted_book_is_filed() {
    let cases = [
        (
            "the Danish novel",
            File::Epub {
                title: "Mumien",
                creator: Some("Michael Katz Krefeld"),
                date: Some("2026-01-05"),
                text: "Det var en mørk nat.",
            },
            false,
        ),
        (
            "the book",
            File::Epub {
                title: "Michael",
                creator: Some("Bob Mortimer"),
                date: Some("2025-06-05"),
                text: "It was a dark night.",
            },
            true,
        ),
        (
            "no creator, author in the text",
            File::Epub {
                title: "Michael",
                creator: None,
                date: None,
                text: "Copyright Bob Mortimer 2025.",
            },
            true,
        ),
        (
            "no creator, author nowhere",
            File::Epub {
                title: "Michael",
                creator: None,
                date: None,
                text: "It was a dark night.",
            },
            false,
        ),
        (
            "another of the author's books",
            File::Epub {
                title: "The Satsuma Complex",
                creator: Some("Bob Mortimer"),
                date: None,
                text: "It was a dark night.",
            },
            false,
        ),
        (
            "dated before publication",
            File::Epub {
                title: "Michael",
                creator: Some("Bob Mortimer"),
                date: Some("2001-01-01"),
                text: "It was a dark night.",
            },
            false,
        ),
        (
            "audiobook by someone else",
            File::Mp3 {
                album: "Mumien",
                author: "Michael Katz Krefeld",
            },
            false,
        ),
        (
            "the audiobook",
            File::Mp3 {
                album: "Michael",
                author: "Bob Mortimer",
            },
            true,
        ),
    ];
    for (name, file, wanted) in cases {
        let (server, book_id, filed) = deliver(&file).await;
        assert_eq!(filed, wanted, "{name}: filed={filed}");
        let (rejected, status): (Value, String) =
            sqlx::query_as("SELECT rejected_releases, want_status FROM books WHERE id = $1")
                .bind(book_id)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        let rejected_release = rejected
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["release_title"] == RELEASE);
        assert_eq!(
            rejected_release, !wanted,
            "{name}: rejected_releases {rejected}"
        );
        if !wanted {
            assert_eq!(status, "wanted", "{name}: the book is searched for again");
        }
    }
}
