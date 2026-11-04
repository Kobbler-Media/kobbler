use serde_json::Value;
use std::path::Path;
use std::process::Command;

pub fn write_tagged_vbr_mp3(path: &Path, seconds: u32) {
    ffmpeg_mp3(
        path,
        &format!("sine=f=220:r=22050:d={seconds},volume='0.2+0.8*mod(t,3)/3':eval=frame"),
        &["-q:a", "6", "-ac", "1"],
    );
}

// ffmpeg's parser folds junk between frames into the next packet, and ID3v1/APE tags
// follow the last frame, so this exercises both ways the frames stop being clean.
pub fn write_cbr_mp3_with_junk_and_trailing_tags(path: &Path, seconds: u32) {
    ffmpeg_mp3(
        path,
        &format!("sine=f=440:r=44100:d={seconds}"),
        &["-b:a", "128k", "-ac", "2"],
    );
    let bytes = std::fs::read(path).unwrap();
    let packets = packets(path, None);
    let at = packets[packets.len() / 2].pos as usize;
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(&[0xA5; 333]);
    out.extend_from_slice(&bytes[at..]);
    out.extend_from_slice(&ape_footer());
    out.extend_from_slice(&id3v1());
    std::fs::write(path, out).unwrap();
}

fn ffmpeg_mp3(path: &Path, source: &str, codec: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
        .args(["-f", "lavfi", "-i", "color=c=red:s=64x64:r=1:d=1"])
        .args(["-map", "0:a", "-map", "1:v", "-c:a", "libmp3lame"])
        .args(codec)
        .args(["-c:v", "mjpeg", "-disposition:v", "attached_pic"])
        .args(["-id3v2_version", "3", "-metadata", "title=Chapter"])
        .arg(path)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
}

fn ape_footer() -> Vec<u8> {
    let mut tag = b"APETAGEX".to_vec();
    for field in [2000u32, 32, 0, 0] {
        tag.extend_from_slice(&field.to_le_bytes());
    }
    tag.extend_from_slice(&[0; 8]);
    tag
}

fn id3v1() -> Vec<u8> {
    let mut tag = b"TAG".to_vec();
    tag.extend_from_slice(b"Trailing title");
    tag.resize(128, 0);
    tag
}

pub fn temp_copy(bytes: &[u8]) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    file
}

pub fn decode_pcm(path: &Path) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a", "-f", "s16le", "-"])
        .output()
        .expect("ffmpeg");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioPacket {
    pub pts_time: f64,
    pub duration_time: f64,
    pub pos: i64,
    pub hash: String,
}

pub fn packets(path: &Path, read_intervals: Option<&str>) -> Vec<AudioPacket> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error", "-select_streams", "a:0", "-show_packets"])
        .args(["-show_data_hash", "MD5", "-of", "json"]);
    if let Some(intervals) = read_intervals {
        cmd.args(["-read_intervals", intervals]);
    }
    let out = cmd.arg(path).output().expect("ffprobe");
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let num = |p: &Value, k: &str| p[k].as_str().unwrap().parse::<f64>().unwrap();
    json["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| AudioPacket {
            pts_time: num(p, "pts_time"),
            duration_time: num(p, "duration_time"),
            pos: p["pos"].as_str().unwrap().parse().unwrap(),
            hash: p["data_hash"].as_str().unwrap().to_string(),
        })
        .collect()
}

pub fn audio_stream(path: &Path) -> Value {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_streams"])
        .args(["-of", "json"])
        .arg(path)
        .output()
        .expect("ffprobe");
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    json["streams"][0].clone()
}

pub fn seconds(stream: &Value, key: &str) -> f64 {
    stream[key].as_str().unwrap().parse().unwrap()
}
