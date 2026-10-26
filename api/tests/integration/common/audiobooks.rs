use super::server::{TestServer, create_book};
use api::repositories::audiobook_chapters::{self, ChapterInput};
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

pub type Span<'a> = (&'a str, f64, f64);

pub enum Audio<'a> {
    None,
    Tone(f64),
    Chaptered(f64, &'a [Span<'a>]),
    TwoHalves,
}

pub struct FileSpec<'a> {
    pub name: &'a str,
    pub duration: f64,
    pub audio: Audio<'a>,
    pub tags: Vec<(&'a str, &'a str)>,
}

pub fn file<'a>(name: &'a str, duration: f64, audio: Audio<'a>) -> FileSpec<'a> {
    FileSpec {
        name,
        duration,
        audio,
        tags: Vec::new(),
    }
}

impl<'a> FileSpec<'a> {
    pub fn tag(mut self, key: &'a str, value: &'a str) -> Self {
        self.tags.push((key, value));
        self
    }

    pub fn by(self, artist: &'a str) -> Self {
        self.tag("artist", artist)
    }
}

pub async fn seed_audiobook(
    server: &TestServer,
    title: &str,
    files: &[FileSpec<'_>],
) -> (Uuid, Vec<Uuid>) {
    let book_id = create_book(server, title).await;
    let ids = seed_files(server, book_id, files).await;
    (book_id, ids)
}

pub fn audio_dir(server: &TestServer, book_id: Uuid) -> std::path::PathBuf {
    Path::new(&server.storage_root())
        .join("audiobooks")
        .join(book_id.to_string())
}

pub fn write_file(server: &TestServer, book_id: Uuid, spec: &FileSpec<'_>) -> std::path::PathBuf {
    let path = audio_dir(server, book_id).join(spec.name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_audio(&path, &spec.audio, &spec.tags);
    path
}

pub async fn seed_files(server: &TestServer, book_id: Uuid, files: &[FileSpec<'_>]) -> Vec<Uuid> {
    let existing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audiobook_files WHERE book_id = $1")
            .bind(book_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let mut ids = Vec::new();
    for (i, spec) in files.iter().enumerate() {
        let path = write_file(server, book_id, spec);
        let format = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO audiobook_files (id, book_id, file_index, format, file_path, file_size, duration_seconds)
             VALUES ($1, $2, $3, $4, $5, 1000, $6)",
        )
        .bind(id)
        .bind(book_id)
        .bind(existing as i32 + i as i32)
        .bind(format)
        .bind(path.to_string_lossy().to_string())
        .bind(spec.duration)
        .execute(&server.pool)
        .await
        .unwrap();
        ids.push(id);
    }
    ids
}

pub fn write_audio(path: &Path, audio: &Audio<'_>, tags: &[(&str, &str)]) {
    let mut args: Vec<String> = vec!["-v".into(), "error".into(), "-y".into()];
    match audio {
        Audio::None => return,
        Audio::Tone(seconds) => {
            args.extend(["-f", "lavfi", "-i"].map(String::from));
            args.push(format!("sine=r=8000:d={seconds}"));
        }
        Audio::Chaptered(seconds, spans) => {
            let meta = path.with_extension("ffmeta");
            let mut text = String::from(";FFMETADATA1\n");
            for (title, start, end) in spans.iter() {
                text.push_str(&format!(
                    "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={title}\n",
                    (start * 1000.0) as i64,
                    (end * 1000.0) as i64
                ));
            }
            std::fs::write(&meta, text).unwrap();
            args.extend(["-f", "lavfi", "-i"].map(String::from));
            args.push(format!("sine=r=8000:d={seconds}"));
            args.extend(["-i".to_string(), meta.to_string_lossy().to_string()]);
            args.extend(["-map", "0", "-map_chapters", "1"].map(String::from));
        }
        Audio::TwoHalves => {
            args.push("-filter_complex".into());
            args.push(
                "sine=r=8000:d=60[a];anullsrc=r=8000:cl=mono:d=3[b];sine=r=8000:d=60[c];[a][b][c]concat=n=3:v=0:a=1"
                    .into(),
            );
        }
    }
    let codec = if path.extension().is_some_and(|e| e == "mp3") {
        "libmp3lame"
    } else {
        "aac"
    };
    args.extend(["-c:a", codec, "-b:a", "16k", "-ac", "1"].map(String::from));
    for (key, value) in tags {
        args.extend(["-metadata".to_string(), format!("{key}={value}")]);
    }
    if codec == "aac" {
        args.extend(["-f", "mp4"].map(String::from));
    }
    args.push(path.to_string_lossy().to_string());
    let status = std::process::Command::new("ffmpeg")
        .args(&args)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg {args:?}");
}

pub fn chapters(source: &str, spans: &[Span<'_>]) -> Vec<ChapterInput> {
    spans
        .iter()
        .enumerate()
        .map(|(i, (title, start, end))| ChapterInput {
            chapter_index: i as i32,
            title: Some(title.to_string()),
            start_seconds: *start,
            end_seconds: *end,
            file_path: None,
            source: source.to_string(),
        })
        .collect()
}

pub async fn store(server: &TestServer, file_id: Uuid, source: &str, spans: &[Span<'_>]) {
    audiobook_chapters::store_chapters(&server.pool, file_id, &chapters(source, spans))
        .await
        .unwrap();
}

pub async fn add_transcript(server: &TestServer, file_id: Uuid, lines: &[(f64, &str)]) {
    for (i, (start, text)) in lines.iter().enumerate() {
        sqlx::query(
            "INSERT INTO audiobook_transcripts (audiobook_file_id, segment_index, start_seconds, end_seconds, text)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(file_id)
        .bind(i as i32)
        .bind(*start)
        .bind(*start + 5.0)
        .bind(*text)
        .execute(&server.pool)
        .await
        .unwrap();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unified {
    pub title: String,
    pub start: f64,
    pub end: f64,
    pub source: String,
}

pub async fn unified(server: &TestServer, book_id: Uuid) -> Vec<Unified> {
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/chapters"))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    body.as_array()
        .unwrap()
        .iter()
        .map(|c| Unified {
            title: c["title"].as_str().unwrap_or_default().to_string(),
            start: c["global_start_seconds"].as_f64().unwrap(),
            end: c["global_end_seconds"].as_f64().unwrap(),
            source: c["source"].as_str().unwrap().to_string(),
        })
        .collect()
}

pub fn titles(chapters: &[Unified]) -> Vec<&str> {
    chapters.iter().map(|c| c.title.as_str()).collect()
}
