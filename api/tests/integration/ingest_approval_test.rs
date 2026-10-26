use crate::common::audiobooks::{Audio, write_audio};
use crate::common::*;
use api::watcher::{FileMode, IngestOptions, process_queue_item};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
enum Entry {
    Approve,
    AutoIngest,
}

const ENTRIES: [Entry; 2] = [Entry::Approve, Entry::AutoIngest];

async fn ingest(server: &TestServer, entry: Entry, item: Uuid, body: Value) -> Uuid {
    match entry {
        Entry::Approve => {
            let resp = server
                .admin_client()
                .post_json(&format!("/api/v1/admin/ingest-queue/{item}/approve"), &body)
                .await;
            resp.assert_ok();
            resp.json::<Value>()["book_id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap()
        }
        Entry::AutoIngest => process_queue_item(
            &server.pool,
            &server.storage_root(),
            &server.library_events,
            &server.nats_publisher,
            &server.runtime_config,
            item,
            IngestOptions::auto(FileMode::Copy),
        )
        .await
        .unwrap(),
    }
}

async fn queue(server: &TestServer, path: &Path, format: &str, author: &str) -> Uuid {
    let name = path.file_name().unwrap().to_str().unwrap();
    let item = add_queue_item(server, path.to_str().unwrap(), name, format).await;
    sqlx::query(
        "UPDATE book_ingest_source
         SET pending_authors = $2, pending_series = $3, pending_series_index = 3
         WHERE book_id = $1",
    )
    .bind(item)
    .bind(json!([{ "name": author }]))
    .bind(json!({ "name": format!("{author} Saga") }))
    .execute(&server.pool)
    .await
    .unwrap();
    item
}

fn minimal_epub() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let stored: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/content.opf", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:00000000-0000-0000-0000-000000000001</dc:identifier>
    <dc:title>Queued</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/c1.xhtml", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title></head>
<body><p>Once upon a time.</p></body></html>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

async fn audiobook_files(server: &TestServer, book_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audiobook_files WHERE book_id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn jobs(server: &TestServer, job_type: &str, book_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = $1 AND payload->>'book_id' = $2",
    )
    .bind(job_type)
    .bind(book_id.to_string())
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

async fn ebook_formats(server: &TestServer, book_id: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT format FROM book_files WHERE book_id = $1 ORDER BY format")
        .bind(book_id)
        .fetch_all(&server.pool)
        .await
        .unwrap()
}

async fn device_entitlements(server: &TestServer, book_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT scope_id FROM entitlements
         WHERE book_id = $1 AND scope_type = 'DEVICE' AND NOT is_removed
         ORDER BY scope_id",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn assert_lands_on_queue_book(server: &TestServer, item: Uuid, book_id: Uuid, author: &str) {
    assert_eq!(book_id, item, "the queue item is the book");
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = $1")
        .bind(item)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let same_title: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE title = $1")
        .bind(&title)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(same_title, 1, "no second book is created for {title}");
    let ingest_status: Option<String> =
        sqlx::query_scalar("SELECT ingest_status FROM books WHERE id = $1")
            .bind(item)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(ingest_status, None);

    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
         WHERE ba.book_id = $1",
    )
    .bind(book_id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(authors, vec![author.to_string()]);
    let series: (String, Option<f64>) = sqlx::query_as(
        "SELECT s.name, bs.series_index FROM book_series bs JOIN series s ON s.id = bs.series_id
         WHERE bs.book_id = $1",
    )
    .bind(book_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(series, (format!("{author} Saga"), Some(3.0)));
}

#[tokio::test]
async fn audio_is_filed_as_an_audiobook_from_every_entry_point() {
    let server = TestServer::start_api_only().await;
    let watch = TempDir::new().unwrap();

    for entry in ENTRIES {
        for (ext, format) in [("m4b", "M4B"), ("mp3", "MP3")] {
            let author = format!("{entry:?} {format} Author");
            let path = watch.path().join(format!("{entry:?}-{ext}.{ext}"));
            write_audio(&path, &Audio::Tone(3.0), &[]);
            let item = queue(&server, &path, format, &author).await;

            let book_id = ingest(&server, entry, item, json!({})).await;

            assert_lands_on_queue_book(&server, item, book_id, &author).await;
            let case = format!("{entry:?} {format}");
            assert_eq!(
                audiobook_files(&server, book_id).await,
                1,
                "{case}: audiobook_files row"
            );
            assert_eq!(
                ebook_formats(&server, book_id).await,
                Vec::<String>::new(),
                "{case}: no ebook rows"
            );
            assert_eq!(
                jobs(&server, "audiobook_probe", book_id).await,
                1,
                "{case}: probe enqueued"
            );
            assert_eq!(
                jobs(&server, "metadata_write", book_id).await,
                0,
                "{case}: no ebook metadata write"
            );
        }
    }
}

#[tokio::test]
async fn ebooks_are_converted_from_every_entry_point() {
    let server = TestServer::start_api_only().await;
    let watch = TempDir::new().unwrap();

    for entry in ENTRIES {
        let author = format!("{entry:?} Epub Author");
        let path = watch.path().join(format!("{entry:?}-novel.epub"));
        std::fs::write(&path, minimal_epub()).unwrap();
        let item = queue(&server, &path, "EPUB", &author).await;

        let book_id = ingest(&server, entry, item, json!({})).await;

        assert_lands_on_queue_book(&server, item, book_id, &author).await;
        assert_eq!(
            ebook_formats(&server, book_id).await,
            vec!["EPUB".to_string(), "KEPUB".to_string()],
            "{entry:?}"
        );
        assert_eq!(audiobook_files(&server, book_id).await, 0, "{entry:?}");
        assert_eq!(
            jobs(&server, "metadata_write", book_id).await,
            1,
            "{entry:?}"
        );
        assert_eq!(
            jobs(&server, "audiobook_probe", book_id).await,
            0,
            "{entry:?}"
        );
    }
}

#[tokio::test]
async fn approval_entitles_exactly_the_requested_devices() {
    let server = TestServer::start_api_only().await;
    let watch = TempDir::new().unwrap();
    let user = create_user(&server, "approver@test.com").await;
    let (picked, _) = create_device(&server, user).await;
    let (other, _) = create_device(&server, user).await;
    let mut both = vec![picked, other];
    both.sort();

    let cases: [(&str, Value, Vec<Uuid>); 3] = [
        ("picked", json!({ "device_ids": [picked] }), vec![picked]),
        ("none", json!({ "device_ids": [] }), vec![]),
        ("auto-sync", json!({}), both),
    ];
    for (name, body, expected) in cases {
        for (ext, format) in [("epub", "EPUB"), ("m4b", "M4B")] {
            let path = watch.path().join(format!("{name}-{ext}.{ext}"));
            if ext == "epub" {
                std::fs::write(&path, minimal_epub()).unwrap();
            } else {
                write_audio(&path, &Audio::Tone(3.0), &[]);
            }
            let item = queue(&server, &path, format, &format!("{name} {format} Author")).await;

            let book_id = ingest(&server, Entry::Approve, item, body.clone()).await;

            assert_eq!(
                device_entitlements(&server, book_id).await,
                expected,
                "{name} {format}"
            );
        }
    }
}
