// Compiles and runs the macOS speech helper.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub const MAX_AUDIO_BYTES: usize = 8 * 1024 * 1024; // ~4 minutes of 16 kHz mono PCM
const MIN_AUDIO_BYTES: usize = 1000;
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(60);
/// Exit status the helper uses for a language SpeechTranscriber does not support.
const EXIT_UNSUPPORTED_LOCALE: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationCategory {
    Unavailable,
    UnsupportedLocale,
    Failed,
    Empty,
    TooLarge,
}

impl DictationCategory {
    pub fn code(&self) -> &'static str {
        match self {
            DictationCategory::Unavailable => "unavailable",
            DictationCategory::UnsupportedLocale => "unsupported_locale",
            DictationCategory::Failed => "failed",
            DictationCategory::Empty => "empty",
            DictationCategory::TooLarge => "too_large",
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct DictationError {
    pub category: DictationCategory,
    pub message: String,
}

impl DictationError {
    fn new(category: DictationCategory, message: &str) -> Self {
        DictationError {
            category,
            message: message.to_string(),
        }
    }
}

pub fn helper_path(bin_dir: &Path) -> PathBuf {
    bin_dir.join("briardr-transcribe")
}

/// Build the Swift helper if it's missing or older than its source.
pub async fn ensure_helper(bin_dir: &Path, source_file: &Path) -> Option<PathBuf> {
    let out = helper_path(bin_dir);
    if is_up_to_date(&out, source_file).await {
        return Some(out);
    }
    let build = Command::new("swiftc")
        .arg("-parse-as-library")
        .arg("-O")
        .arg("-o")
        .arg(&out)
        .arg(source_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(BUILD_TIMEOUT, build).await {
        Ok(Ok(status)) if status.success() => Some(out),
        _ => None,
    }
}

async fn is_up_to_date(out: &Path, source: &Path) -> bool {
    let (Ok(out_meta), Ok(src_meta)) = (
        tokio::fs::metadata(out).await,
        tokio::fs::metadata(source).await,
    ) else {
        return false;
    };
    match (out_meta.modified(), src_meta.modified()) {
        (Ok(out_time), Ok(src_time)) => out_time >= src_time,
        _ => false,
    }
}

/// A fresh private folder holding one recording. Removed when dropped, on every path.
struct TempAudioDir(PathBuf);

impl TempAudioDir {
    async fn create() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("briardr-dictation-{}", random_hex::<6>()));
        tokio::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .await?;
        Ok(TempAudioDir(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempAudioDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn random_hex<const N: usize>() -> String {
    let mut buf = [0u8; N];
    rand::fill(&mut buf);
    hex::encode(buf)
}

async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await
}

/// Accepts what the regex /^[A-Za-z]{2,3}([-_][A-Za-z0-9]{2,8})*$/ accepts.
fn valid_locale(locale: &str) -> bool {
    let mut parts = locale.split(['-', '_']);
    let Some(lang) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

pub async fn transcribe(
    helper: Option<&Path>,
    audio: &[u8],
    locale: Option<&str>,
) -> Result<String, DictationError> {
    let Some(helper) = helper else {
        return Err(DictationError::new(
            DictationCategory::Unavailable,
            "On-device dictation isn’t available on this Mac (needs macOS 26 and the Swift toolchain to build the helper).",
        ));
    };
    if audio.len() > MAX_AUDIO_BYTES {
        return Err(DictationError::new(
            DictationCategory::TooLarge,
            "That recording is too long.",
        ));
    }
    if audio.len() < MIN_AUDIO_BYTES {
        return Err(DictationError::new(
            DictationCategory::Empty,
            "No speech was recorded.",
        ));
    }

    let couldnt_transcribe = || {
        DictationError::new(
            DictationCategory::Failed,
            "The Mac couldn’t transcribe that recording.",
        )
    };
    let dir = TempAudioDir::create()
        .await
        .map_err(|_| couldnt_transcribe())?;
    let file = dir.path().join(format!("{}.wav", random_hex::<6>()));
    write_private(&file, audio)
        .await
        .map_err(|_| couldnt_transcribe())?;

    let mut command = Command::new(helper);
    command.arg(&file);
    if let Some(locale) = locale.filter(|l| valid_locale(l)) {
        command.arg(locale);
    }
    let run = command.stdin(Stdio::null()).kill_on_drop(true).output();
    let output = match tokio::time::timeout(TRANSCRIBE_TIMEOUT, run).await {
        Ok(Ok(output)) => output,
        _ => return Err(couldnt_transcribe()),
    };
    if output.status.code() == Some(EXIT_UNSUPPORTED_LOCALE) {
        return Err(DictationError::new(
            DictationCategory::UnsupportedLocale,
            "Dictation doesn’t support this language on the Mac.",
        ));
    }
    if !output.status.success() {
        return Err(couldnt_transcribe());
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err(DictationError::new(
            DictationCategory::Empty,
            "No speech was recognized.",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::os::unix::fs::PermissionsExt;

    /// Writes an executable `sh` helper whose body runs with the audio path as `$1`.
    fn fake_helper(scratch: &Scratch, body: &str) -> PathBuf {
        let path = scratch.path().join("briardr-transcribe");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn audio() -> Vec<u8> {
        vec![0u8; 1000]
    }

    #[test]
    fn helper_path_is_named_briardr_transcribe() {
        assert_eq!(
            helper_path(Path::new("/opt/bin")),
            PathBuf::from("/opt/bin/briardr-transcribe")
        );
    }

    #[test]
    fn category_codes_match_ts() {
        assert_eq!(DictationCategory::Unavailable.code(), "unavailable");
        assert_eq!(
            DictationCategory::UnsupportedLocale.code(),
            "unsupported_locale"
        );
        assert_eq!(DictationCategory::Failed.code(), "failed");
        assert_eq!(DictationCategory::Empty.code(), "empty");
        assert_eq!(DictationCategory::TooLarge.code(), "too_large");
    }

    #[test]
    fn locale_check_matches_ts_pattern() {
        for ok in [
            "en",
            "eng",
            "en-US",
            "en_US",
            "zh-Hant-TW",
            "de-CH-1996",
            "fil-PH",
        ] {
            assert!(valid_locale(ok), "{ok} should be accepted");
        }
        for bad in [
            "",
            "e",
            "english",
            "en-",
            "-en",
            "en--US",
            "en_-US",
            "en-U",
            "en-USABCDEF0",
            "en US",
            "1en",
            "en-US-",
            "en_US!",
            "en;rm",
            "en-US\n",
        ] {
            assert!(!valid_locale(bad), "{bad:?} should be rejected");
        }
    }

    #[tokio::test]
    async fn refuses_before_touching_disk_in_ts_order() {
        let missing = Path::new("/nonexistent/briardr-transcribe");
        let err = transcribe(None, &vec![0u8; 5000], None).await.unwrap_err();
        assert_eq!(err.category, DictationCategory::Unavailable);
        let err = transcribe(None, &vec![0u8; MAX_AUDIO_BYTES + 1], None)
            .await
            .unwrap_err();
        assert_eq!(err.category, DictationCategory::Unavailable);
        let err = transcribe(Some(missing), &vec![0u8; MAX_AUDIO_BYTES + 1], None)
            .await
            .unwrap_err();
        assert_eq!(err.category, DictationCategory::TooLarge);
        assert_eq!(err.message, "That recording is too long.");
        let err = transcribe(Some(missing), &[0u8; 10], None)
            .await
            .unwrap_err();
        assert_eq!(err.category, DictationCategory::Empty);
        assert_eq!(err.message, "No speech was recorded.");
    }

    #[tokio::test]
    async fn success_returns_trimmed_output() {
        let scratch = Scratch::new("dictation");
        let helper = fake_helper(&scratch, "printf '  Please run the integration tests \\n'");
        let text = transcribe(Some(&helper), &audio(), Some("en-US"))
            .await
            .unwrap();
        assert_eq!(text, "Please run the integration tests");
    }

    #[tokio::test]
    async fn exit_2_is_unsupported_locale() {
        let scratch = Scratch::new("dictation");
        let helper = fake_helper(&scratch, "exit 2");
        let err = transcribe(Some(&helper), &audio(), Some("xx"))
            .await
            .unwrap_err();
        assert_eq!(err.category, DictationCategory::UnsupportedLocale);
        assert_eq!(err.category.code(), "unsupported_locale");
        assert_eq!(
            err.message,
            "Dictation doesn’t support this language on the Mac."
        );
    }

    #[tokio::test]
    async fn other_failures_are_failed() {
        let scratch = Scratch::new("dictation");
        let helper = fake_helper(&scratch, "printf 'hi'; exit 1");
        let err = transcribe(Some(&helper), &audio(), None).await.unwrap_err();
        assert_eq!(err.category, DictationCategory::Failed);
        assert_eq!(err.message, "The Mac couldn’t transcribe that recording.");
        let missing = scratch.path().join("no-such-helper");
        let err = transcribe(Some(&missing), &audio(), None)
            .await
            .unwrap_err();
        assert_eq!(err.category, DictationCategory::Failed);
    }

    #[tokio::test]
    async fn blank_output_is_empty() {
        let scratch = Scratch::new("dictation");
        let helper = fake_helper(&scratch, "printf ' \\n\\n'");
        let err = transcribe(Some(&helper), &audio(), None).await.unwrap_err();
        assert_eq!(err.category, DictationCategory::Empty);
        assert_eq!(err.message, "No speech was recognized.");
    }

    #[tokio::test]
    async fn locale_is_passed_only_when_valid() {
        let scratch = Scratch::new("dictation");
        let rec = scratch.path().join("record");
        let helper = fake_helper(
            &scratch,
            &format!("printf '%s' \"$2\" > '{}'; printf ok", rec.display()),
        );
        let text = transcribe(Some(&helper), &audio(), Some("en-US"))
            .await
            .unwrap();
        assert_eq!(text, "ok");
        assert_eq!(std::fs::read_to_string(&rec).unwrap(), "en-US");

        let text = transcribe(Some(&helper), &audio(), Some("en US; rm"))
            .await
            .unwrap();
        assert_eq!(text, "ok");
        assert_eq!(std::fs::read_to_string(&rec).unwrap(), "");

        transcribe(Some(&helper), &audio(), None).await.unwrap();
        assert_eq!(std::fs::read_to_string(&rec).unwrap(), "");
    }

    #[tokio::test]
    async fn temp_dir_is_removed_after_success_and_failure() {
        let scratch = Scratch::new("dictation");
        let rec = scratch.path().join("record");
        let ok = fake_helper(
            &scratch,
            &format!(
                "dirname \"$1\" > '{}'; test -s \"$1\" && printf ok",
                rec.display()
            ),
        );
        assert_eq!(transcribe(Some(&ok), &audio(), None).await.unwrap(), "ok");
        let dir = PathBuf::from(std::fs::read_to_string(&rec).unwrap().trim());
        assert!(!dir.exists(), "temp dir should be removed after success");

        let failing = fake_helper(
            &scratch,
            &format!("dirname \"$1\" > '{}'; exit 1", rec.display()),
        );
        assert!(transcribe(Some(&failing), &audio(), None).await.is_err());
        let dir = PathBuf::from(std::fs::read_to_string(&rec).unwrap().trim());
        assert!(!dir.exists(), "temp dir should be removed after failure");
    }

    #[tokio::test]
    async fn audio_file_and_folder_are_private() {
        let dir = TempAudioDir::create().await.unwrap();
        let file = dir.path().join("audio.wav");
        write_private(&file, &audio()).await.unwrap();
        let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        let file_mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);
        let name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("briardr-dictation-"));
    }

    #[tokio::test]
    async fn ensure_helper_reuses_helper_newer_than_source() {
        let scratch = Scratch::new("dictation");
        let src = scratch.path().join("transcribe.swift");
        std::fs::write(&src, "// source").unwrap();
        let out = helper_path(scratch.path());
        std::fs::write(&out, "#!/bin/sh\n").unwrap();
        assert_eq!(ensure_helper(scratch.path(), &src).await, Some(out));
    }
}
