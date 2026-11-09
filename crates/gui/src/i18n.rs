//! Translation table for every string the GUI shows.
//!
//! [`strings!`] generates the [`Key`] enum, so a misspelt key is a compile
//! error and adding one without both languages does not compile. The test
//! module proves every key has text in both languages and that placeholders
//! (`{0}`, `{1}`, ...) match.
//!
//! The language is thread-local state: all GUI text is built on the UI
//! thread, and tests can switch it without affecting each other. The worker
//! threads never produce user-facing text (technical errors from the decoder
//! stay in English, like the log).

use std::cell::Cell;
use std::fmt::Display;

use ccoonverter_core::settings::UiLanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    English,
    Korean,
}

thread_local! {
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::English) };
}

/// The language all [`t`]/[`tf`] calls on this thread use.
pub fn set_language(lang: Lang) {
    CURRENT.with(|c| c.set(lang));
}

pub fn language() -> Lang {
    CURRENT.with(|c| c.get())
}

/// The language for a user setting; `Auto` follows the system locale.
pub fn resolve(choice: UiLanguage, locale: Option<&str>) -> Lang {
    match choice {
        UiLanguage::English => Lang::English,
        UiLanguage::Korean => Lang::Korean,
        UiLanguage::Auto => match locale {
            Some(l) if l.trim().to_ascii_lowercase().starts_with("ko") => Lang::Korean,
            _ => Lang::English,
        },
    }
}

/// Current system locale (for example `ko-KR`), when the OS reports one.
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

/// Apply `choice`, detecting the system language for `Auto`.
pub fn apply(choice: UiLanguage) {
    set_language(resolve(choice, system_locale().as_deref()));
}

macro_rules! strings {
    ($($key:ident => $en:literal, $ko:literal;)*) => {
        /// Every translatable string of the GUI.
        #[allow(clippy::enum_variant_names)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Key { $($key,)* }

        impl Key {
            /// All keys, for tests.
            #[cfg_attr(not(test), allow(dead_code))]
            pub const ALL: &'static [Key] = &[$(Key::$key,)*];

            fn text(self, lang: Lang) -> &'static str {
                match (self, lang) {
                    $((Key::$key, Lang::English) => $en,)*
                    $((Key::$key, Lang::Korean) => $ko,)*
                }
            }
        }
    };
}

/// The text of `key` in the current language.
pub fn t(key: Key) -> &'static str {
    key.text(language())
}

/// The text of `key` with `{0}`, `{1}`, ... replaced by `args`.
pub fn tf(key: Key, args: &[&dyn Display]) -> String {
    let mut out = t(key).to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), &a.to_string());
    }
    out
}

strings! {
    // Directory paths panel
    DirectoryPaths => "DIRECTORY PATHS", "폴더 경로";
    ShowPreview => "SHOW PREVIEW", "미리보기 표시";
    HidePreview => "HIDE PREVIEW", "미리보기 숨기기";
    Browse => "BROWSE", "찾아보기";
    SourceFolder => "SOURCE FOLDER", "원본 폴더";
    OutputFolderOptional => "OUTPUT FOLDER (Optional)", "출력 폴더 (선택)";
    OutputHint => "Leave empty for default subfolder", "비워 두면 기본 하위 폴더에 저장";
    IfOutputExists => "If output exists:", "같은 파일이 있으면:";
    ConflictSuffix => "Add suffix", "접미사 붙이기";
    ConflictSkip => "Skip", "건너뛰기";
    ConflictOverwrite => "Overwrite", "덮어쓰기";
    DroppedSelected => "{0} dropped file(s) selected", "끌어다 놓은 파일 {0}개 선택됨";
    Clear => "CLEAR", "해제";
    BrowseSourceTitle => "Select source folder", "원본 폴더 선택";
    BrowseOutputTitle => "Select output folder", "출력 폴더 선택";

    // Settings panel
    Settings => "SETTINGS", "설정";
    ExtractEmbedded => "Extract Embedded JPEG (Fast)", "카메라 JPEG 추출 (빠름)";
    ExifPreservation => "EXIF PRESERVATION", "EXIF 보존";
    KeepFileDate => "Keep source file date", "원본 파일 날짜 유지";
    ResizeLabel => "Resize (Long Edge px):", "긴 변 크기 (px):";
    ResizeHint => "0 for original", "0이면 원본 크기";
    ResizeOriginal => "(0 = Original)", "(0 = 원본)";
    QualityLabel => "JPG Quality (1-100):", "JPG 품질 (1-100):";
    RawAdjustments => "RAW ADJUSTMENTS", "RAW 현상 조정";
    Reset => "RESET", "초기화";
    ToneCurve => "Tone Curve", "톤 커브";
    WhiteBalanceLabel => "White Balance:", "화이트 밸런스:";
    PictureStyleLabel => "Picture Style:", "픽처 스타일:";
    ExposureLabel => "Exposure (EV):", "노출 보정 (EV):";
    BrightnessLabel => "Brightness (0.0-2.0):", "밝기 (0.0-2.0):";
    RgbBalanceLabel => "RGB Channel Balance:", "RGB 채널 균형:";
    LevelsLabel => "Levels (Black/White):", "레벨 (블랙/화이트):";
    Presets => "PRESETS", "프리셋";
    SelectPreset => "Select Preset...", "프리셋 선택...";
    Save => "SAVE", "저장";
    Delete => "DELETE", "삭제";

    // White balance and picture style names
    WbCameraDefault => "Camera Default", "카메라 기본값";
    WbAuto => "Auto", "자동";
    StyleStandard => "Standard", "표준";
    StylePortrait => "Portrait", "인물";
    StyleLandscape => "Landscape", "풍경";
    StyleNeutral => "Neutral", "중립";
    StyleVivid => "Vivid", "선명하게";
    StyleMonochrome => "Monochrome", "모노크롬";
    StyleSepia => "Sepia", "세피아";
    StyleCinematic => "Cinematic", "시네마틱";
    StyleHdr => "HDR", "HDR";
    StyleSoft => "Soft", "부드럽게";
    StyleDramatic => "Dramatic", "드라마틱";
    StyleBlackAndWhite => "Black & White", "흑백";

    // Actions and progress
    StartButton => "Let's Get Cooking!", "변환 시작";
    Converting => "Converting...", "변환 중...";
    CancelButton => "CANCEL", "취소";
    Ready => "READY", "준비됨";
    ProgressLine => "Processing ({0}/{1}): {2} ({3}%)", "처리 중 ({0}/{1}): {2} ({3}%)";
    ProgressError => "Error", "오류";
    ProgressStarting => "Processing (0/{0}): ... (0%)", "처리 중 (0/{0}): ... (0%)";
    EtaUnknown => "ETA: --:--", "남은 시간: --:--";
    EtaValue => "ETA: {0}", "남은 시간: {0}";
    ElapsedSeconds => "{0}s", "{0}초";
    ElapsedMinutes => "{0}m {1}s", "{0}분 {1}초";

    // Footer
    CheckUpdates => "Check for updates on startup", "시작할 때 업데이트 확인";
    FooterCredit => "v{0}  ·  MIT  ·  RAW decoding: rawler (LGPL-2.1)", "v{0}  ·  MIT  ·  RAW 디코딩: rawler (LGPL-2.1)";
    LanguageSystem => "System", "시스템";
    LanguageEnglish => "English", "English";
    LanguageKorean => "한국어", "한국어";

    // Preset and update modals
    SavePresetTitle => "Save Preset", "프리셋 저장";
    EnterPresetName => "Enter preset name:", "프리셋 이름:";
    UpdateAvailable => "A New Version is Available!", "새 버전이 나왔습니다!";
    UpdateVersions => "Current: v{0}  →  Latest: v{1}", "현재: v{0}  →  최신: v{1}";
    Download => "DOWNLOAD", "다운로드";
    Later => "LATER", "나중에";

    // Native dialogs
    TitleError => "Error", "오류";
    TitleInfo => "Info", "알림";
    TitleSuccess => "Success", "완료";
    TitleConfirm => "Confirm", "확인";
    TitleQuit => "Quit", "종료";
    TitleCancel => "Cancel", "변환 취소";
    TitleCancelled => "Cancelled", "취소됨";
    TitleReset => "Reset", "초기화";
    TitleCompletedWithErrors => "Completed with Errors", "일부 파일 실패";
    SavePresetFailed => "Could not save presets:\n{0}", "프리셋을 저장하지 못했습니다:\n{0}";
    QuitWhileConverting => "A conversion is still running.\nCancel it and quit?", "아직 변환 중입니다.\n취소하고 종료할까요?";
    ConfirmCancelConversion => "Are you sure you want to cancel the conversion?", "변환을 취소할까요?";
    Cancelling => "Cancelling...", "취소하는 중...";
    ResetDone => "RAW adjustments have been reset to defaults.", "RAW 조정값을 기본값으로 되돌렸습니다.";
    PresetSaved => "Preset '{0}' saved.", "프리셋 '{0}'을(를) 저장했습니다.";
    ConfirmDeletePreset => "Delete preset '{0}'?", "프리셋 '{0}'을(를) 삭제할까요?";
    PresetDeleted => "Preset '{0}' deleted.", "프리셋 '{0}'을(를) 삭제했습니다.";
    CouldNotStart => "Could not start conversion:\n{0}", "변환을 시작하지 못했습니다:\n{0}";
    OpenOutputFolderQuestion => "{0}\n\nOpen output folder?", "{0}\n\n출력 폴더를 열까요?";
    InvalidSourceFolder => "Please select a valid source folder.", "올바른 원본 폴더를 선택하세요.";
    NoRawFound => "No RAW files found in the selected folder.", "선택한 폴더에 RAW 파일이 없습니다.";
    FileSystemError => "File system error: {0}", "파일 시스템 오류: {0}";
    UnexpectedError => "An unexpected error occurred:\n{0}", "예기치 못한 오류가 발생했습니다:\n{0}";
    ConversionCancelled => "Conversion cancelled.", "변환을 취소했습니다.";
    SkippedLine => "Skipped (output exists): {0}\n", "건너뜀 (이미 있음): {0}개\n";
    SuccessBody => "Successfully converted {0} file(s).\n{1}Time taken: {2}\nSaved to: {3}", "파일 {0}개를 변환했습니다.\n{1}걸린 시간: {2}\n저장 위치: {3}";
    CompletedWithErrorsBody => "Conversion complete with some errors:\nSuccessful: {0}\nFailed: {1}\n{2}Time taken: {3}\n\nCheck {4} for details.\nSaved to: {5}", "변환은 끝났지만 일부 파일에서 오류가 났습니다.\n성공: {0}개\n실패: {1}개\n{2}걸린 시간: {3}\n\n자세한 내용은 {4} 파일을 확인하세요.\n저장 위치: {5}";
    WindowOpenFailed => "Could not open the application window:\n{0}\n\n{1}", "프로그램 창을 열 수 없습니다:\n{0}\n\n{1}";
    RendererHintGlow => "This build draws with OpenGL 2.0+. Update the graphics driver, or (old GPU, remote desktop or VM) try a build with the wgpu renderer (`cargo build --release -p ccoonverter --features wgpu`).", "이 빌드는 OpenGL 2.0 이상으로 그립니다. 그래픽 드라이버를 업데이트하거나, 오래된 GPU, 원격 데스크톱, 가상 머신이라면 wgpu 렌더러 빌드를 사용해 보세요(`cargo build --release -p ccoonverter --features wgpu`).";
    RendererHintWgpu => "This build draws with wgpu (DirectX 12 / Vulkan / Metal). If no adapter is available (old GPU, remote desktop or VM), use the default OpenGL (glow) build.", "이 빌드는 wgpu(DirectX 12 / Vulkan / Metal)로 그립니다. 사용 가능한 어댑터가 없다면(오래된 GPU, 원격 데스크톱, 가상 머신) 기본 OpenGL(glow) 빌드를 사용하세요.";

    // Drag and drop
    DropBusy => "Conversion running: the drop was ignored.", "변환 중이라 끌어다 놓은 항목을 무시했습니다.";
    DropReady => "{0} RAW file(s) ready. Press the button to convert just these.", "RAW 파일 {0}개를 골랐습니다. 버튼을 누르면 이 파일만 변환합니다.";
    DropHoverTitle => "Drop a folder or RAW files", "폴더나 RAW 파일을 놓으세요";
    DropHoverHint => "A folder sets the source. Files convert just those files.", "폴더는 원본 폴더로 지정하고, 파일은 그 파일만 변환합니다.";
    DropFolderNotice => "Using folder {0}; ignored {1} other dropped item(s).", "{0} 폴더를 사용합니다. 함께 놓은 항목 {1}개는 무시했습니다.";
    DropNoRaw => "No RAW files in the drop ({0} file(s) ignored). Supported: {1}.", "RAW 파일이 없습니다 ({0}개 무시). 지원 형식: {1}.";
    DropSameDrive => "Dropped files must be on the same drive.", "끌어다 놓는 파일은 같은 드라이브에 있어야 합니다.";
    DropIgnoredFiles => "Ignored {0} non-RAW file(s).", "RAW가 아닌 파일 {0}개를 무시했습니다.";

    // Preview panel
    PreviewHeader => "PREVIEW", "미리보기";
    ShowBefore => "SHOW BEFORE", "카메라 JPEG 보기";
    ShowAfter => "SHOW AFTER", "현상 결과 보기";
    CameraJpeg => "Camera JPEG", "카메라 JPEG";
    CameraJpegTag => "CAMERA JPEG", "카메라 JPEG";
    CompareTip => "Compare with the camera's embedded JPEG", "카메라가 RAW에 넣어 둔 JPEG과 비교합니다";
    NoRawHere => "No RAW files here.", "RAW 파일이 없습니다.";
    NoRawInFolder => "No RAW files in this folder.", "이 폴더에 RAW 파일이 없습니다.";
    SelectOrDrop => "Select a folder or drop RAW files.", "폴더를 선택하거나 RAW 파일을 끌어다 놓으세요.";
    LookingForRaw => "Looking for RAW files...", "RAW 파일을 찾는 중...";
    PreviewUpdating => "Updating...", "적용 중...";
    RawCount => "{0} RAW file(s)", "RAW 파일 {0}개";
    PreviewDeveloping => "Developing...", "현상 중...";
    PreviewCantDevelop => "Can't develop this file ({0}). Camera JPEG shown.", "이 파일은 현상할 수 없습니다 ({0}). 카메라 JPEG을 표시합니다.";
    PreviewUnavailable => "Preview unavailable: {0}", "미리보기 불가: {0}";
    PausedWhileConverting => "Paused while converting.", "변환 중에는 잠시 멈춥니다.";
    NoEmbeddedJpeg => "No embedded JPEG in this file.", "이 파일에는 카메라 JPEG이 없습니다.";
    LoadingPreview => "Loading preview...", "미리보기를 불러오는 중...";
    NoPreviewYet => "No preview yet.", "아직 미리보기가 없습니다.";
    PreviewPausedStatus => "Preview paused while converting.", "변환 중에는 미리보기를 멈춥니다.";
    NoEmbeddedDevelopStatus => "No embedded JPEG in this file; conversion will develop the RAW.", "카메라 JPEG이 없는 파일입니다. 변환할 때 RAW를 현상합니다.";

    // Tooltips
    TipSource => "Folder with your RAW files. Subfolders are included.", "RAW 파일이 있는 폴더입니다. 하위 폴더까지 함께 변환합니다.";
    TipBrowse => "Pick a folder.", "폴더를 고릅니다.";
    TipOutput => "Where the JPGs go. Leave empty to use Converted_JPG inside the source folder.", "JPG를 저장할 폴더입니다. 비워 두면 원본 폴더 안의 Converted_JPG에 저장합니다.";
    TipConflict => "What to do when a JPG with the same name already exists.", "같은 이름의 JPG가 이미 있을 때 어떻게 할지 정합니다.";
    TipEmbedded => "Uses the JPEG the camera stored inside the RAW. Fastest; RAW adjustments do not apply.", "카메라가 RAW 안에 저장한 JPEG을 그대로 꺼냅니다. 가장 빠르지만 RAW 조정은 적용되지 않습니다.";
    TipExif => "Copies shooting data into the JPG: camera, lens, exposure, date, GPS and rating where the file has them. Not all metadata may be copied identically.", "카메라, 렌즈, 노출, 촬영 일시, GPS, 별점을 파일에 있는 만큼 JPG에 기록합니다. 일부 정보는 원본과 똑같이 복사되지 않을 수 있습니다.";
    TipKeepDate => "Gives the JPG the same modified date as its RAW file.", "JPG의 수정한 날짜를 RAW 파일과 같게 맞춥니다.";
    TipResize => "Long edge of the output in pixels. 0 keeps the full size.", "결과 이미지의 긴 변 픽셀 수입니다. 0이면 원본 크기를 유지합니다.";
    TipQuality => "Higher means better quality and larger files. 90 is a good default.", "높을수록 화질이 좋아지고 파일이 커집니다. 보통 90이면 충분합니다.";
    TipToneCurve => "Applies a gentle film-like contrast curve, close to a camera JPEG.", "카메라 JPEG에 가까운 부드러운 S자 대비 커브를 적용합니다.";
    TipReset => "Returns the RAW adjustments below to their defaults.", "아래 RAW 조정값을 기본값으로 되돌립니다.";
    TipWhiteBalance => "Camera Default uses the camera's setting. Auto estimates it from the picture.", "카메라 기본값은 촬영 시 설정을 쓰고, 자동은 사진에서 화이트 밸런스를 추정합니다.";
    TipStyle => "A color and contrast look applied after development.", "현상한 뒤 색감과 대비에 입히는 스타일입니다.";
    TipExposure => "Exposure compensation in stops. +1 doubles the light.", "노출 보정값(스톱)입니다. +1이면 빛이 두 배가 됩니다.";
    TipBrightness => "Linear brightness multiplier, 1.0 is neutral. 2.0 equals +1 EV.", "선형 밝기 배율입니다. 1.0이 기준이며 2.0은 +1 EV와 같습니다.";
    TipRgbBalance => "Scales the red, green and blue channels. 1.0 is neutral.", "빨강, 초록, 파랑 채널을 배율로 조정합니다. 1.0이 기준입니다.";
    TipLevels => "Black point and white point on the 8-bit scale. Values outside are clipped.", "8비트 기준 블랙 포인트와 화이트 포인트입니다. 범위 밖 값은 잘립니다.";
    TipPreset => "Applies a saved group of settings.", "저장해 둔 설정 묶음을 불러옵니다.";
    TipPresetSave => "Saves the current settings as a named preset.", "지금 설정을 이름을 붙여 저장합니다.";
    TipPresetDelete => "Deletes the selected preset.", "선택한 프리셋을 삭제합니다.";
    TipCheckUpdates => "Looks for a newer release on GitHub at startup, at most once a day.", "시작할 때 GitHub에서 새 버전을 하루 한 번까지만 확인합니다.";
    TipLanguage => "Interface language. System follows your operating system.", "화면 언어입니다. 시스템은 운영체제 언어 설정을 따릅니다.";
    TipPreviewToggle => "Shows or hides the preview.", "미리보기를 표시하거나 숨깁니다.";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut out: Vec<String> = s
            .match_indices('{')
            .filter_map(|(i, _)| {
                let rest = &s[i + 1..];
                let end = rest.find('}')?;
                rest[..end]
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    .then(|| rest[..end].to_string())
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Keys whose Korean text legitimately has no Hangul or equals the English.
    const SAME_IN_BOTH: &[Key] = &[Key::StyleHdr, Key::LanguageEnglish, Key::LanguageKorean];

    #[test]
    fn every_key_has_text_in_both_languages() {
        assert!(Key::ALL.len() > 100, "keys: {}", Key::ALL.len());
        for &key in Key::ALL {
            let en = key.text(Lang::English);
            let ko = key.text(Lang::Korean);
            assert!(!en.trim().is_empty(), "{key:?} has no English text");
            assert!(!ko.trim().is_empty(), "{key:?} has no Korean text");
            if !SAME_IN_BOTH.contains(&key) {
                assert_ne!(en, ko, "{key:?} is not translated");
                assert!(
                    ko.chars().any(|c| ('가'..='힣').contains(&c)),
                    "{key:?} has no Hangul: {ko}"
                );
            }
        }
    }

    #[test]
    fn placeholders_match_between_languages() {
        for &key in Key::ALL {
            assert_eq!(
                placeholders(key.text(Lang::English)),
                placeholders(key.text(Lang::Korean)),
                "{key:?}"
            );
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for &key in Key::ALL {
            assert!(seen.insert(format!("{key:?}")), "duplicate {key:?}");
        }
    }

    #[test]
    fn formatting_replaces_all_placeholders() {
        set_language(Lang::English);
        assert_eq!(
            tf(Key::ProgressLine, &[&1, &4, &"a.jpg", &25]),
            "Processing (1/4): a.jpg (25%)"
        );
        set_language(Lang::Korean);
        assert_eq!(
            tf(Key::ProgressLine, &[&1, &4, &"a.jpg", &25]),
            "처리 중 (1/4): a.jpg (25%)"
        );
        set_language(Lang::English);
    }

    #[test]
    fn locale_resolution() {
        assert_eq!(resolve(UiLanguage::Auto, Some("ko-KR")), Lang::Korean);
        assert_eq!(resolve(UiLanguage::Auto, Some("KO")), Lang::Korean);
        assert_eq!(resolve(UiLanguage::Auto, Some("en-US")), Lang::English);
        assert_eq!(resolve(UiLanguage::Auto, None), Lang::English);
        assert_eq!(resolve(UiLanguage::Korean, Some("en-US")), Lang::Korean);
        assert_eq!(resolve(UiLanguage::English, Some("ko-KR")), Lang::English);
    }
}
