//! Which of an audiobook's files belong to the edition the book plays, and
//! what becomes of the rest.
//!
//! A book's files sometimes hold more than one recording of it: a narrated
//! mp3 set next to a full-cast m4b, two narrators interleaved in one folder,
//! a second copy of one part, or half of another edition. Files are grouped
//! into releases (container, narrator tag, folder) and a release is another
//! edition when its transcript repeats the book's text, or, before there is a
//! transcript, when it is a different recording of about the same length.
//! A complete other edition becomes its own book; a partial one, or a copy of
//! a file the book already has, leaves the library.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use uuid::Uuid;

const SHINGLE_WORDS: usize = 6;
const MIN_TEXT: usize = 50;
const SHARED_TEXT: f64 = 0.3;
const COPY_TEXT: f64 = 0.8;
const COPY_LENGTH: f64 = 0.9;
const COMPLETE: f64 = 0.8;
const TOUCHED: f64 = 0.05;
const SIMILAR_LENGTH: f64 = 0.85;
const MIN_EDITION_SECONDS: f64 = 300.0;
const SAME_LENGTH_SECONDS: f64 = 0.5;
const LOCATE_WINDOW_SECONDS: f64 = 90.0;

#[derive(Debug, Clone, Default)]
pub struct EditionFile {
    pub id: Uuid,
    pub index: i32,
    pub path: String,
    pub format: String,
    pub duration: f64,
    pub narrator: Option<String>,
    pub album: Option<String>,
    pub abridged: Option<bool>,
    pub bit_rate: Option<i64>,
    pub chaptered: bool,
    pub listeners: usize,
    pub text: Vec<(u64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Release {
    pub format: String,
    pub narrator: String,
}

impl Release {
    pub fn of(format: &str, narrator: Option<&str>) -> Self {
        Self {
            format: format.to_lowercase(),
            narrator: narrator.map(normalize).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sibling {
    pub book_id: Uuid,
    pub release: Release,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Split,
    Join(Uuid),
    Drop,
}

#[derive(Debug, Clone)]
pub struct Move {
    pub action: Action,
    pub files: Vec<Uuid>,
    pub narrator: Option<String>,
    pub abridged: Option<bool>,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub moves: Vec<Move>,
    pub copies: HashMap<Uuid, Uuid>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    pub fn leaving(&self) -> HashSet<Uuid> {
        self.moves
            .iter()
            .flat_map(|m| m.files.iter().copied())
            .collect()
    }
}

pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn shingles(segments: &[(f64, String)]) -> Vec<(u64, f64)> {
    let words: Vec<(String, f64)> = segments
        .iter()
        .flat_map(|(start, text)| {
            text.split_whitespace()
                .map(normalize)
                .filter(|w| !w.is_empty())
                .map(move |w| (w, *start))
                .collect::<Vec<_>>()
        })
        .collect();
    words
        .windows(SHINGLE_WORDS)
        .map(|w| {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for (word, _) in w {
                for byte in word.bytes().chain(std::iter::once(b' ')) {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0100_0000_01b3);
                }
            }
            (hash, w[0].1)
        })
        .collect()
}

fn release_dir(path: &str) -> String {
    let mut dir = Path::new(path).parent();
    while let Some(d) = dir {
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace());
        let numbered = stem.len() < name.len();
        if numbered && ["cd", "disc", "disk", "part", "pt", "vol", "volume"].contains(&stem.trim())
        {
            dir = d.parent();
        } else {
            break;
        }
    }
    dir.map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default()
}

struct Group<'a> {
    release: Release,
    files: Vec<&'a EditionFile>,
    text: HashSet<u64>,
}

impl Group<'_> {
    fn duration(&self) -> f64 {
        self.files.iter().map(|f| f.duration).sum()
    }

    fn ids(&self) -> Vec<Uuid> {
        self.files.iter().map(|f| f.id).collect()
    }

    fn first_index(&self) -> i32 {
        self.files.iter().map(|f| f.index).min().unwrap_or(0)
    }
}

fn group<'a>(files: &[&'a EditionFile]) -> Vec<Group<'a>> {
    let mut by_key: BTreeMap<(Release, String), Vec<&'a EditionFile>> = BTreeMap::new();
    for file in files {
        let release = Release::of(&file.format, file.narrator.as_deref());
        by_key
            .entry((release, release_dir(&file.path)))
            .or_default()
            .push(file);
    }
    let mut groups: Vec<Group> = by_key
        .into_iter()
        .map(|((release, _), files)| Group {
            text: files
                .iter()
                .flat_map(|f| f.text.iter().map(|(h, _)| *h))
                .collect(),
            release,
            files,
        })
        .collect();
    groups.sort_by_key(Group::first_index);
    groups
}

fn containment(part: &HashSet<u64>, whole: &HashSet<u64>) -> f64 {
    if part.is_empty() {
        return 0.0;
    }
    part.iter().filter(|h| whole.contains(h)).count() as f64 / part.len() as f64
}

fn different_release(a: &EditionFile, b: &EditionFile) -> bool {
    let album = match (&a.album, &b.album) {
        (Some(x), Some(y)) => normalize(x) != normalize(y),
        _ => false,
    };
    album
        || Release::of(&a.format, a.narrator.as_deref())
            != Release::of(&b.format, b.narrator.as_deref())
}

fn is_copy(a: &EditionFile, b: &EditionFile, texts: &HashMap<Uuid, HashSet<u64>>) -> bool {
    let (short, long) = if a.duration <= b.duration {
        (a, b)
    } else {
        (b, a)
    };
    if short.duration <= 0.0 || short.duration / long.duration < COPY_LENGTH {
        return false;
    }
    let (ts, tl) = (&texts[&short.id], &texts[&long.id]);
    if ts.len() >= MIN_TEXT && tl.len() >= MIN_TEXT {
        return containment(ts, tl) >= COPY_TEXT;
    }
    short.duration >= MIN_EDITION_SECONDS
        && long.duration - short.duration <= SAME_LENGTH_SECONDS
        && different_release(a, b)
}

fn copies(files: &[EditionFile]) -> HashMap<Uuid, Uuid> {
    let texts: HashMap<Uuid, HashSet<u64>> = files
        .iter()
        .map(|f| (f.id, f.text.iter().map(|(h, _)| *h).collect()))
        .collect();
    let all: Vec<&EditionFile> = files.iter().collect();
    let weight: HashMap<Uuid, f64> = group(&all)
        .iter()
        .flat_map(|g| {
            let d = g.duration();
            g.files.iter().map(move |f| (f.id, d))
        })
        .collect();
    let mut dropped: HashMap<Uuid, Uuid> = HashMap::new();
    for (i, a) in files.iter().enumerate() {
        for b in &files[i + 1..] {
            if dropped.contains_key(&a.id) || dropped.contains_key(&b.id) || !is_copy(a, b, &texts)
            {
                continue;
            }
            let a_first = (weight[&a.id], b.index) >= (weight[&b.id], a.index);
            let (keep, drop) = if a_first { (a, b) } else { (b, a) };
            dropped.insert(drop.id, keep.id);
        }
    }
    dropped
}

fn coverage(other: &Group, reference: &Group) -> f64 {
    let total = reference.duration();
    if total <= 0.0 {
        return 0.0;
    }
    let touched: f64 = reference
        .files
        .iter()
        .filter(|f| {
            let own: HashSet<u64> = f.text.iter().map(|(h, _)| *h).collect();
            !own.is_empty()
                && own.iter().filter(|h| other.text.contains(h)).count() as f64
                    >= own.len() as f64 * TOUCHED
        })
        .map(|f| f.duration)
        .sum();
    touched / total
}

enum Overlap {
    None,
    Partial,
    Complete,
}

fn overlap(other: &Group, reference: &Group) -> Overlap {
    if other.text.len() >= MIN_TEXT && reference.text.len() >= MIN_TEXT {
        if containment(&other.text, &reference.text) < SHARED_TEXT {
            return Overlap::None;
        }
        return if coverage(other, reference) >= COMPLETE {
            Overlap::Complete
        } else {
            Overlap::Partial
        };
    }
    let (a, b) = (other.duration(), reference.duration());
    if other.release != reference.release
        && a >= MIN_EDITION_SECONDS
        && a.min(b) / a.max(b) >= SIMILAR_LENGTH
    {
        Overlap::Complete
    } else {
        Overlap::None
    }
}

fn preference(g: &Group, longest: f64) -> (usize, bool, bool, bool, i64, Option<i64>, i32) {
    (
        g.files.iter().map(|f| f.listeners).sum(),
        !g.files.iter().any(|f| f.abridged == Some(true)),
        g.duration() >= longest * 0.9,
        g.files.iter().any(|f| f.chaptered),
        -(g.files.len() as i64),
        g.files.iter().filter_map(|f| f.bit_rate).min(),
        -g.first_index(),
    )
}

fn edition_move(action: Action, g: &Group, reason: String) -> Move {
    Move {
        action,
        files: g.ids(),
        narrator: g.files.iter().find_map(|f| f.narrator.clone()),
        abridged: g.files.iter().find_map(|f| f.abridged),
        reason,
    }
}

pub fn plan(files: &[EditionFile], siblings: &[Sibling]) -> Plan {
    let mut plan = Plan::default();
    if files.len() < 2 {
        return plan;
    }
    plan.copies = copies(files);
    let mut copied: Vec<Uuid> = plan.copies.keys().copied().collect();
    copied.sort_by_key(|id| files.iter().position(|f| f.id == *id));
    if !copied.is_empty() {
        plan.moves.push(Move {
            action: Action::Drop,
            files: copied,
            narrator: None,
            abridged: None,
            reason: "copy of a file the book already has".into(),
        });
    }

    let live: Vec<&EditionFile> = files
        .iter()
        .filter(|f| !plan.copies.contains_key(&f.id))
        .collect();
    let groups = group(&live);
    let Some(reference) = groups
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| a.duration().total_cmp(&b.duration()).then(ib.cmp(ia)))
        .map(|(i, _)| i)
    else {
        return plan;
    };

    let mut complete = vec![reference];
    for (i, g) in groups.iter().enumerate() {
        if i == reference {
            continue;
        }
        if g.release != groups[reference].release
            && let Some(sibling) = siblings.iter().find(|s| s.release == g.release)
        {
            plan.moves.push(edition_move(
                Action::Join(sibling.book_id),
                g,
                "belongs to the book's other edition".into(),
            ));
            continue;
        }
        match overlap(g, &groups[reference]) {
            Overlap::None => {}
            Overlap::Partial => plan.moves.push(edition_move(
                Action::Drop,
                g,
                "part of another edition".into(),
            )),
            Overlap::Complete => complete.push(i),
        }
    }
    if complete.len() < 2 {
        return plan;
    }

    let longest = complete
        .iter()
        .map(|i| groups[*i].duration())
        .fold(0.0, f64::max);
    let primary = *complete
        .iter()
        .max_by_key(|i| preference(&groups[**i], longest))
        .unwrap();
    for i in complete {
        if i != primary {
            plan.moves.push(edition_move(
                Action::Split,
                &groups[i],
                "another edition of the book".into(),
            ));
        }
    }
    plan
}

/// Where a position in a file that is leaving the book lands in the files
/// that stay: the same moment of a copy, or the place the transcripts say
/// the same words are spoken.
pub fn locate(
    files: &[EditionFile],
    plan: &Plan,
    file_id: Uuid,
    seconds: f64,
    staying: &HashSet<Uuid>,
) -> Option<(Uuid, f64)> {
    if let Some(keeper) = plan.copies.get(&file_id)
        && staying.contains(keeper)
    {
        return Some((*keeper, seconds));
    }
    let source = files.iter().find(|f| f.id == file_id)?;
    let near: Vec<(u64, f64)> = source
        .text
        .iter()
        .filter(|(_, t)| (t - seconds).abs() <= LOCATE_WINDOW_SECONDS)
        .copied()
        .collect();
    let mut index: HashMap<u64, (Uuid, f64)> = HashMap::new();
    for f in files.iter().filter(|f| staying.contains(&f.id)) {
        for (h, t) in &f.text {
            index.entry(*h).or_insert((f.id, *t));
        }
    }
    let mut by_file: HashMap<Uuid, Vec<f64>> = HashMap::new();
    for (h, t) in &near {
        if let Some((target, at)) = index.get(h) {
            by_file.entry(*target).or_default().push(at + (seconds - t));
        }
    }
    let (target, mut times) = by_file.into_iter().max_by_key(|(_, t)| t.len())?;
    times.sort_by(f64::total_cmp);
    Some((target, times[times.len() / 2].max(0.0)))
}
