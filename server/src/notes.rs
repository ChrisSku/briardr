// Per-stream notes with revisions and image attachments.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::{read_json, write_file_atomic, ConfigError};
use crate::uploads::{
    new_upload_dir, quote_path, receive_upload, uploads_root, UploadError, UploadKind,
};

pub const MAX_NOTE_BYTES: usize = 256 * 1024;
pub const MAX_NOTES: usize = 200;
/// `^[a-f0-9]{16}$`
pub const NOTE_ID_PATTERN: &str = r"^[a-f0-9]{16}$";
/// `^[a-f0-9]{24}\.png$`
pub const IMAGE_NAME_PATTERN: &str = r"^[a-f0-9]{24}\.png$";

/// Images nothing references are removed once they are older than this (the grace period).
const IMAGE_GRACE: Duration = Duration::from_secs(7 * 24 * 3600);
/// `<24 hex>.png`
const IMAGE_NAME_LEN: usize = 28;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub text: String,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveResult {
    Ok(Note),
    Conflict(Option<Note>),
    TooLarge,
    TooMany,
}

/// The paste result: the rewritten text plus how many images were copied and how many were missing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pasted {
    pub text: String,
    pub images: usize,
    pub missing: usize,
}

#[derive(Debug, Error)]
pub enum NotesError {
    #[error("notes I/O: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Upload(#[from] UploadError),
}

/// The current on-disk shape, `{streamId, notes}`, read back.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamFile {
    stream_id: String,
    notes: Vec<Note>,
}

/// The same shape, borrowed, for writing.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamOut<'a> {
    stream_id: &'a str,
    notes: &'a [Note],
}

/// The legacy single-note shape, `{streamId, text, revision, updatedAt}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyFile {
    stream_id: String,
    text: String,
    revision: u64,
    #[serde(default)]
    updated_at: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OnDisk {
    Stream(StreamFile),
    Legacy(LegacyFile),
}

#[derive(Debug)]
pub struct Notes {
    pub dir: PathBuf,
    /// Serializes read-modify-write of a stream file; the server is multi-threaded.
    write_lock: Mutex<()>,
}

impl Notes {
    pub fn new(dir: PathBuf) -> Self {
        Notes {
            dir,
            write_lock: Mutex::new(()),
        }
    }

    pub fn list(&self, stream_id: &str) -> Vec<Note> {
        let mut notes = self.read(stream_id);
        notes.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        notes
    }

    pub fn get(&self, stream_id: &str, id: &str) -> Option<Note> {
        self.read(stream_id).into_iter().find(|n| n.id == id)
    }

    /// `id` None creates a note with a fresh id; `base_revision` None counts as 0.
    pub fn save(
        &self,
        stream_id: &str,
        id: Option<&str>,
        text: &str,
        base_revision: Option<u64>,
    ) -> Result<SaveResult, NotesError> {
        if text.len() > MAX_NOTE_BYTES {
            return Ok(SaveResult::TooLarge);
        }
        let _guard = self.locked();
        let mut notes = self.read(stream_id);
        let id = id.map_or_else(random_hex::<8>, str::to_string);
        let base = base_revision.unwrap_or(0);
        let pos = notes.iter().position(|n| n.id == id);
        let (cur_revision, created_at) = match pos {
            Some(i) => (notes[i].revision, Some(notes[i].created_at)),
            None => (0, None),
        };
        if cur_revision != base {
            return Ok(SaveResult::Conflict(pos.map(|i| notes[i].clone())));
        }
        if pos.is_none() && notes.len() >= MAX_NOTES {
            return Ok(SaveResult::TooMany);
        }
        let now = now_ms();
        let note = Note {
            id,
            text: text.to_string(),
            revision: base + 1,
            created_at: created_at.unwrap_or(now),
            updated_at: now,
        };
        match pos {
            Some(i) => notes[i] = note.clone(),
            None => notes.push(note.clone()),
        }
        self.write(stream_id, &notes)?;
        Ok(SaveResult::Ok(note))
    }

    /// False when the note is absent; nothing is written in that case.
    pub fn remove(&self, stream_id: &str, id: &str) -> Result<bool, NotesError> {
        let _guard = self.locked();
        let mut notes = self.read(stream_id);
        let Some(i) = notes.iter().position(|n| n.id == id) else {
            return Ok(false);
        };
        notes.remove(i);
        self.write(stream_id, &notes)?;
        Ok(true)
    }

    /// Store a PNG; returns the generated file name.
    pub fn add_image(&self, stream_id: &str, png: &[u8]) -> Result<String, NotesError> {
        let dir = self.image_dir(stream_id);
        let upload = receive_upload(png, UploadKind::Image, "image", &dir)?;
        let name = format!("{}.png", random_hex::<12>());
        fs::rename(&upload, dir.join(&name))?;
        if let Some(folder) = upload.parent() {
            // Empty by now; remove_dir refuses anything else.
            let _ = fs::remove_dir(folder);
        }
        Ok(name)
    }

    pub fn image_path(&self, stream_id: &str, name: &str) -> Option<PathBuf> {
        if !is_image_name(name) {
            return None;
        }
        let path = self.image_dir(stream_id).join(name);
        path.is_file().then_some(path)
    }

    /// Rewrite image references in pasted text to paths of private copies in the uploads folder.
    pub fn for_terminal(&self, stream_id: &str, text: &str) -> Result<Pasted, NotesError> {
        self.for_terminal_in(stream_id, text, &uploads_root())
    }

    fn for_terminal_in(
        &self,
        stream_id: &str,
        text: &str,
        uploads: &Path,
    ) -> Result<Pasted, NotesError> {
        let images = self.image_dir(stream_id);
        let b = text.as_bytes();
        let mut seen: HashMap<String, Option<PathBuf>> = HashMap::new();
        let mut folder: Option<PathBuf> = None;
        let mut out = Vec::with_capacity(b.len());
        let mut last = 0;
        let mut i = 0;
        while i < b.len() {
            let Some((end, name)) = match_ref(b, i) else {
                i += 1;
                continue;
            };
            let name = String::from_utf8_lossy(name).into_owned();
            let copy = match seen.get(&name).cloned() {
                Some(done) => done,
                None => {
                    let src = images.join(&name);
                    let done = if src.is_file() {
                        let dir = match &folder {
                            Some(d) => d.clone(),
                            None => {
                                let d = new_upload_dir(uploads)?;
                                folder = Some(d.clone());
                                d
                            }
                        };
                        let copy = dir.join(&name);
                        fs::copy(&src, &copy)?;
                        Some(copy)
                    } else {
                        None
                    };
                    seen.insert(name, done.clone());
                    done
                }
            };
            // A missing image keeps its original text.
            if let Some(copy) = copy {
                out.extend_from_slice(&b[last..i]);
                out.extend_from_slice(quote_path(&copy).as_bytes());
                last = end;
            }
            i = end;
        }
        out.extend_from_slice(&b[last..]);
        let images_count = seen.values().filter(|c| c.is_some()).count();
        Ok(Pasted {
            text: String::from_utf8_lossy(&out).into_owned(),
            images: images_count,
            missing: seen.len() - images_count,
        })
    }

    fn locked(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn file(&self, stream_id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", stream_key(stream_id)))
    }

    fn image_dir(&self, stream_id: &str) -> PathBuf {
        self.dir.join("images").join(stream_key(stream_id))
    }

    /// Notes in stored order. A file for another stream, or an empty legacy note, reads as empty.
    fn read(&self, stream_id: &str) -> Vec<Note> {
        let parsed: Option<OnDisk> = read_json(&self.file(stream_id), None);
        match parsed {
            Some(OnDisk::Stream(f)) if f.stream_id == stream_id => f.notes,
            Some(OnDisk::Legacy(f)) if f.stream_id == stream_id && !f.text.is_empty() => {
                let at = f.updated_at.unwrap_or_else(now_ms);
                vec![Note {
                    id: legacy_id(stream_id),
                    text: f.text,
                    revision: f.revision,
                    created_at: at,
                    updated_at: at,
                }]
            }
            _ => Vec::new(),
        }
    }

    fn write(&self, stream_id: &str, notes: &[Note]) -> Result<(), NotesError> {
        // The notes folder is private (0700).
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let bytes = serde_json::to_vec(&StreamOut { stream_id, notes })?;
        write_file_atomic(&self.file(stream_id), &bytes, 0o600)?;
        self.prune_images(stream_id, notes);
        Ok(())
    }

    /// Drop images no note mentions, once they are past the grace period. Errors are ignored.
    fn prune_images(&self, stream_id: &str, notes: &[Note]) {
        let Ok(entries) = fs::read_dir(self.image_dir(stream_id)) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if notes
                .iter()
                .any(|n| n.text.contains(&format!("note:{name}")))
            {
                continue;
            }
            let path = entry.path();
            let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            if now.duration_since(modified).unwrap_or_default() > IMAGE_GRACE {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// `^[a-f0-9]{24}\.png$`, on bytes so a non-ASCII name cannot split a character.
fn is_image_name_bytes(b: &[u8]) -> bool {
    b.len() == IMAGE_NAME_LEN
        && b.ends_with(b".png")
        && b[..24]
            .iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_image_name(name: &str) -> bool {
    is_image_name_bytes(name.as_bytes())
}

fn name_at(b: &[u8], start: usize) -> Option<&[u8]> {
    let name = b.get(start..start + IMAGE_NAME_LEN)?;
    is_image_name_bytes(name).then_some(name)
}

/// Match `![alt](note:NAME)` or bare `note:NAME` at `i` (the IMAGE_REF pattern), returning
/// the end index and the name. The alt text may hold anything but `]` and a newline.
fn match_ref(b: &[u8], i: usize) -> Option<(usize, &[u8])> {
    if b[i..].starts_with(b"![") {
        if let Some(off) = b[i + 2..].iter().position(|&c| c == b']' || c == b'\n') {
            let close = i + 2 + off;
            if b[close] == b']' && b[close + 1..].starts_with(b"(note:") {
                let start = close + 1 + "(note:".len();
                if let Some(name) = name_at(b, start) {
                    if b.get(start + IMAGE_NAME_LEN) == Some(&b')') {
                        return Some((start + IMAGE_NAME_LEN + 1, name));
                    }
                }
            }
        }
    }
    if b[i..].starts_with(b"note:") {
        if let Some(name) = name_at(b, i + "note:".len()) {
            return Some((i + "note:".len() + IMAGE_NAME_LEN, name));
        }
    }
    None
}

fn random_hex<const N: usize>() -> String {
    let mut buf = [0u8; N];
    rand::fill(&mut buf);
    hex::encode(buf)
}

/// First 32 hex digits of SHA-256(streamId): the file and folder key.
fn stream_key(stream_id: &str) -> String {
    let mut key = hex::encode(Sha256::digest(stream_id.as_bytes()));
    key.truncate(32);
    key
}

/// The id a legacy single-note file is presented under: first 16 hex of SHA-256("note:" + streamId).
fn legacy_id(stream_id: &str) -> String {
    let mut id = hex::encode(Sha256::digest(format!("note:{stream_id}").as_bytes()));
    id.truncate(16);
    id
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::os::unix::fs::PermissionsExt;

    fn fake_png(width: u32, height: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        v.extend_from_slice(&[0, 0, 0, 13]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    fn set_old(path: &Path) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap();
    }

    fn saved(result: SaveResult) -> Note {
        match result {
            SaveResult::Ok(note) => note,
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn save_list_get() {
        let tmp = Scratch::new("notes-basic");
        let notes = Notes::new(tmp.join("notes"));
        let note = saved(notes.save("s1", None, "hello", None).unwrap());
        assert_eq!(note.id.len(), 16);
        assert_eq!(note.revision, 1);
        assert_eq!(note.text, "hello");
        assert_eq!(notes.get("s1", &note.id), Some(note.clone()));
        assert_eq!(notes.list("s1"), vec![note]);
        assert!(notes.list("other").is_empty());
    }

    #[test]
    fn update_and_conflicts() {
        let tmp = Scratch::new("notes-conflict");
        let notes = Notes::new(tmp.join("notes"));
        let first = saved(
            notes
                .save("s1", Some("0123456789abcdef"), "a", None)
                .unwrap(),
        );
        let second = saved(notes.save("s1", Some(&first.id), "b", Some(1)).unwrap());
        assert_eq!(second.revision, 2);
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(second.text, "b");

        // A stale base revision gets the current note back.
        assert_eq!(
            notes.save("s1", Some(&first.id), "c", Some(1)).unwrap(),
            SaveResult::Conflict(Some(second.clone()))
        );

        // A deleted note conflicts with its old revision, but can be recreated from 0.
        assert!(notes.remove("s1", &first.id).unwrap());
        assert_eq!(
            notes.save("s1", Some(&first.id), "d", Some(2)).unwrap(),
            SaveResult::Conflict(None)
        );
        assert_eq!(
            saved(notes.save("s1", Some(&first.id), "d", Some(0)).unwrap()).revision,
            1
        );
    }

    #[test]
    fn too_large_boundary() {
        let tmp = Scratch::new("notes-large");
        let notes = Notes::new(tmp.join("notes"));
        let at_limit = "a".repeat(MAX_NOTE_BYTES);
        assert!(matches!(
            notes.save("s1", None, &at_limit, None).unwrap(),
            SaveResult::Ok(_)
        ));
        let over = "a".repeat(MAX_NOTE_BYTES + 1);
        assert_eq!(
            notes.save("s1", None, &over, None).unwrap(),
            SaveResult::TooLarge
        );
    }

    #[test]
    fn too_many_allows_updates() {
        let tmp = Scratch::new("notes-many");
        let notes = Notes::new(tmp.join("notes"));
        let mut first = None;
        for n in 0..MAX_NOTES {
            let note = saved(notes.save("s1", None, &format!("n{n}"), None).unwrap());
            first.get_or_insert(note);
        }
        let first = first.unwrap();
        assert_eq!(
            notes.save("s1", None, "extra", None).unwrap(),
            SaveResult::TooMany
        );
        assert!(matches!(
            notes
                .save("s1", Some(&first.id), "changed", Some(1))
                .unwrap(),
            SaveResult::Ok(_)
        ));
    }

    #[test]
    fn remove_reports_presence() {
        let tmp = Scratch::new("notes-remove");
        let notes = Notes::new(tmp.join("notes"));
        let note = saved(notes.save("s1", None, "x", None).unwrap());
        assert!(!notes.remove("s1", "ffffffffffffffff").unwrap());
        assert!(notes.remove("s1", &note.id).unwrap());
        assert!(notes.list("s1").is_empty());
    }

    #[test]
    fn list_is_newest_first() {
        let tmp = Scratch::new("notes-order");
        let notes = Notes::new(tmp.join("notes"));
        let mk = |id: &str, created_at: u64| Note {
            id: id.to_string(),
            text: id.to_string(),
            revision: 1,
            created_at,
            updated_at: created_at,
        };
        notes
            .write(
                "s1",
                &[
                    mk("aaaaaaaaaaaaaaaa", 1),
                    mk("bbbbbbbbbbbbbbbb", 3),
                    mk("cccccccccccccccc", 2),
                ],
            )
            .unwrap();
        let ids: Vec<String> = notes.list("s1").into_iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            ["bbbbbbbbbbbbbbbb", "cccccccccccccccc", "aaaaaaaaaaaaaaaa"]
        );
    }

    #[test]
    fn legacy_file_reads_as_one_note() {
        let tmp = Scratch::new("notes-legacy");
        let notes = Notes::new(tmp.join("notes"));
        fs::create_dir_all(&notes.dir).unwrap();
        fs::write(
            notes.file("s1"),
            r#"{"streamId":"s1","text":"old","revision":4,"updatedAt":null}"#,
        )
        .unwrap();
        let listed = notes.list("s1");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, legacy_id("s1"));
        assert_eq!(listed[0].revision, 4);
        assert_eq!(listed[0].text, "old");
        assert!(listed[0].created_at > 0);

        fs::write(
            notes.file("s1"),
            r#"{"streamId":"s1","text":"","revision":4,"updatedAt":5}"#,
        )
        .unwrap();
        assert!(notes.list("s1").is_empty());
    }

    #[test]
    fn stream_id_mismatch_reads_empty() {
        let tmp = Scratch::new("notes-mismatch");
        let notes = Notes::new(tmp.join("notes"));
        fs::create_dir_all(&notes.dir).unwrap();
        fs::write(
            notes.file("s2"),
            r#"{"streamId":"s1","notes":[{"id":"0123456789abcdef","text":"x","revision":1,"createdAt":1,"updatedAt":1}]}"#,
        )
        .unwrap();
        assert!(notes.list("s2").is_empty());
    }

    #[test]
    fn add_image_and_paste() {
        let tmp = Scratch::new("notes-paste");
        let notes = Notes::new(tmp.join("notes"));
        let uploads = tmp.join("uploads");

        let name = notes.add_image("s1", &fake_png(3, 2)).unwrap();
        assert!(is_image_name(&name));
        assert!(notes.image_path("s1", &name).unwrap().is_file());
        assert!(notes.image_path("s2", &name).is_none());
        assert!(notes.image_path("s1", "../escape.png").is_none());
        assert!(matches!(
            notes.add_image("s1", b"not a png at all, just text........"),
            Err(NotesError::Upload(UploadError::NotPng))
        ));

        let missing = format!("{}.png", "0".repeat(24));
        let text = format!("see ![shot](note:{name}) and note:{name} then note:{missing} end");
        let pasted = notes.for_terminal_in("s1", &text, &uploads).unwrap();
        assert_eq!(pasted.images, 1);
        assert_eq!(pasted.missing, 1);
        assert!(!pasted.text.contains(&format!("note:{name}")));
        assert!(pasted.text.contains(&format!("note:{missing}")));
        let root = uploads.display().to_string();
        assert_eq!(pasted.text.matches(&root).count(), 2);
        assert_eq!(fs::read_dir(&uploads).unwrap().count(), 1);

        // Text with no references passes through and creates no folder.
        let plain = notes
            .for_terminal_in("s1", "just words", &tmp.join("none"))
            .unwrap();
        assert_eq!(
            plain,
            Pasted {
                text: "just words".into(),
                images: 0,
                missing: 0
            }
        );
        assert!(!tmp.join("none").exists());
    }

    #[test]
    fn alt_text_stops_at_bracket_or_newline() {
        let name = format!("{}.png", "a".repeat(24));
        let text = format!("![alt](note:{name})");
        assert_eq!(
            match_ref(text.as_bytes(), 0),
            Some((text.len(), name.as_bytes()))
        );
        let bracketed = format!("![a]b](note:{name})");
        assert_eq!(match_ref(bracketed.as_bytes(), 0), None);
        let bare = format!("x note:{name}");
        assert_eq!(
            match_ref(bare.as_bytes(), 2).map(|(end, _)| end),
            Some(bare.len())
        );
    }

    #[test]
    fn prune_drops_old_unreferenced_images() {
        let tmp = Scratch::new("notes-prune");
        let notes = Notes::new(tmp.join("notes"));
        let kept_old = notes.add_image("s1", &fake_png(1, 1)).unwrap();
        let dropped_old = notes.add_image("s1", &fake_png(1, 1)).unwrap();
        let fresh = notes.add_image("s1", &fake_png(1, 1)).unwrap();
        let dir = notes.image_dir("s1");
        set_old(&dir.join(&kept_old));
        set_old(&dir.join(&dropped_old));

        // Saving a note that references one old image prunes the other old, unreferenced one.
        notes
            .save("s1", None, &format!("![x](note:{kept_old})"), None)
            .unwrap();
        assert!(dir.join(&kept_old).is_file());
        assert!(!dir.join(&dropped_old).exists());
        assert!(dir.join(&fresh).is_file());
        assert_eq!(
            fs::metadata(dir.join(&fresh)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
