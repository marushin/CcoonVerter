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
    ShowPreview => "Show preview", "미리보기 표시";
    HidePreview => "Hide preview", "미리보기 숨기기";
    Browse => "Browse", "찾아보기";
    SourceFolder => "RAW folder", "RAW 폴더";
    OutputFolderOptional => "Save to (optional)", "저장 폴더 (선택)";
    OutputHint => "Empty: a Converted_JPG folder inside the RAW folder", "비워 두면 RAW 폴더 안의 Converted_JPG에 저장";
    IfOutputExists => "If a file exists", "이미 있으면";
    ConflictSuffix => "Add suffix", "접미사 붙이기";
    ConflictSkip => "Skip", "건너뛰기";
    ConflictOverwrite => "Overwrite", "덮어쓰기";
    DroppedSelected => "{0} dropped file(s) selected", "끌어다 놓은 파일 {0}개 선택됨";
    Clear => "Clear", "해제";
    BrowseSourceTitle => "Select source folder", "원본 폴더 선택";
    BrowseOutputTitle => "Select output folder", "출력 폴더 선택";

    // Settings panel
    ModeRaw => "RAW develop", "RAW 현상";
    ModeCamera => "Camera JPEG", "카메라 JPEG";
    ModeRawNote => "Develops each RAW with the adjustments below.", "RAW마다 아래 조정값으로 현상합니다.";
    ModeCameraNote => "Takes the JPEG the camera saved inside each RAW. Fastest, and the RAW adjustments do not apply.", "카메라가 RAW 안에 저장한 JPEG을 꺼냅니다. 가장 빠르며 RAW 조정은 적용되지 않습니다.";
    SectionStyle => "Style", "스타일";
    SectionOutput => "Output", "출력";
    SectionPresets => "Presets", "프리셋";
    SectionApp => "App", "앱";
    Advanced => "Advanced", "고급";
    AdvancedActive => "Brightness {0}", "밝기 {0}";
    ExifKeep => "Keep EXIF shooting data", "촬영 정보(EXIF) 유지";
    KeepFileDate => "Keep source file date", "원본 파일 날짜 유지";
    ResizeLabel => "Long edge (px)", "긴 변 (px)";
    ResizeHint => "Original size", "원본 크기";
    QualityLabel => "Quality", "품질";
    Reset => "Reset", "초기화";
    ToneCurve => "Tone Curve", "톤 커브";
    WhiteBalanceLabel => "White balance", "화이트 밸런스";
    ExposureLabel => "Exposure (EV)", "노출 (EV)";
    BrightnessLabel => "Brightness", "밝기";
    BrightnessNote => "Set by an older preset.", "이전 프리셋의 값입니다.";
    RgbBalanceLabel => "RGB balance", "RGB 균형";
    LevelsLabel => "Levels", "레벨";
    LevelsBlack => "Black point", "블랙 포인트";
    LevelsWhite => "White point", "화이트 포인트";
    SelectPreset => "Choose a preset...", "프리셋 선택...";
    Save => "Save", "저장";
    Delete => "Delete", "삭제";

    // Output options
    OutputButton => "Format:  {0}", "형식:  {0}";
    CameraJpegOutput => "JPEG (camera)", "JPEG (카메라 원본)";
    OutputOptionsTitle => "Output options", "출력 옵션";
    FormatLabel => "File format", "파일 형식";
    ColorSpaceLabel => "Color space", "색 공간";
    NameTemplateLabel => "File name", "파일 이름";
    NameExample => "Example: {0}", "예: {0}";
    NameTokens => "Tokens: {name} {date} {time} {seq} {camera} {ext}", "사용 가능: {name} {date} {time} {seq} {camera} {ext}";
    Close => "Close", "닫기";
    CameraJpegOnly => "Camera JPEG mode always writes the camera's own JPEG. File format and color space are not used; the file name still applies.", "카메라 JPEG 추출은 항상 카메라가 만든 JPEG 그대로 저장합니다. 파일 형식과 색 공간은 적용되지 않고, 파일 이름만 적용됩니다.";
    NoteJpeg => "Smallest files, 8-bit. Quality slider applies.", "파일이 가장 작습니다. 8비트이며 품질 슬라이더가 적용됩니다.";
    NotePng => "8-bit, lossless. Larger files; the quality slider is not used.", "8비트 무손실입니다. 파일이 크고 품질 슬라이더는 쓰지 않습니다.";
    NoteTiff => "16 bits per channel, uncompressed. For further editing; very large files.", "채널당 16비트 비압축입니다. 추가 보정용이며 파일이 매우 큽니다.";
    NoteWebp => "Lossy, 8-bit, smaller than JPEG. Quality slider applies. Up to 16383 pixels per side.", "손실 압축 8비트이며 JPEG보다 작습니다. 품질 슬라이더가 적용됩니다. 한 변 최대 16383픽셀입니다.";
    NoteSrgb => "Right for screens and the web.", "화면과 웹에 알맞습니다.";
    NoteAdobe => "For printing workflows. The profile is AdobeRGB-compatible. rawler clips colors to sRGB before this step, so it gains less than a true wide-gamut pipeline.", "인쇄 작업용입니다. 프로파일은 AdobeRGB 호환입니다. rawler가 이 단계 전에 색을 sRGB로 잘라 내므로, 실제 광색역 처리만큼 색이 넓어지지는 않습니다.";
    TipOutputOptions => "Choose the file format, color space and file name of the converted files.", "변환 결과의 파일 형식, 색 공간, 파일 이름을 고릅니다.";
    TipFormat => "File type written for every photo. Camera JPEG mode is always JPEG.", "사진마다 저장할 파일 형식입니다. 카메라 JPEG 추출은 항상 JPEG입니다.";
    TipColorSpace => "Color space of the output, with an ICC profile embedded in the file.", "결과의 색 공간입니다. 파일에 ICC 프로파일을 함께 넣습니다.";
    TipNameTemplate => "Names the output files. {date} uses the capture date, or the file date when there is none. {seq} counts the batch in file order. The extension follows the format.", "결과 파일 이름입니다. {date}는 촬영일을 쓰고, 없으면 파일 날짜를 씁니다. {seq}는 파일 순서대로 매깁니다. 확장자는 파일 형식을 따릅니다.";
    TemplateEmpty => "The file name is empty.", "파일 이름이 비어 있습니다.";
    TemplateTooLong => "The file name is too long.", "파일 이름이 너무 깁니다.";
    TemplatePathSeparator => "A file name cannot contain / or \\.", "파일 이름에는 / 나 \\ 를 쓸 수 없습니다.";
    TemplateTraversal => "A file name cannot contain two dots in a row.", "파일 이름에 점을 두 개 연속으로 쓸 수 없습니다.";
    TemplateBrace => "A { or } has no partner.", "{ 또는 } 의 짝이 맞지 않습니다.";
    TemplateUnknownToken => "Unknown token {{0}}.", "알 수 없는 항목입니다: {{0}}";

    // Files that need attention after a batch
    IssuesTitle => "{0} file(s) need attention", "확인이 필요한 파일 {0}개";
    FailUnsupportedCamera => "RAW development does not support this camera ({0}) yet.", "이 카메라({0})는 아직 RAW 현상을 지원하지 않습니다.";
    FailUnsupportedCameraUnnamed => "RAW development does not support this camera yet.", "이 카메라는 아직 RAW 현상을 지원하지 않습니다.";
    FailUnsupportedCompression => "This RAW compression cannot be developed yet (for example Nikon High Efficiency).", "이 RAW 압축 방식은 아직 현상할 수 없습니다(예: Nikon High Efficiency).";
    FailUnreadable => "The file is damaged or is not a RAW file this app can read.", "파일이 손상되었거나 이 앱이 읽을 수 없는 RAW 파일입니다.";
    FailNoEmbeddedJpeg => "No camera JPEG was found in this file.", "이 파일에서 카메라 JPEG을 찾지 못했습니다.";
    FailDisk => "Could not read or write the file: {0}", "파일을 읽거나 쓰지 못했습니다: {0}";
    FailEncode => "The output could not be written: {0}", "결과 파일을 만들지 못했습니다: {0}";
    FailInternal => "Unexpected error: {0}", "예기치 못한 오류: {0}";
    FailCameraJpegHint => "Switch to Camera JPEG mode to get the camera's JPEG from this file.", "카메라 JPEG 모드로 바꾸면 이 파일에서 카메라 JPEG을 꺼낼 수 있습니다.";
    NoteFellBack => "No camera JPEG in this file, so the RAW was developed instead.", "이 파일에 카메라 JPEG이 없어 대신 RAW를 현상했습니다.";

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
    StartButton => "Let's Get Cooking!", "맛있게 구워볼까요!";
    Converting => "Cooking...", "맛있게 굽는 중...";
    CancelButton => "Cancel", "취소";
    Ready => "Ready when you are", "준비됐어요";
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
    Download => "Download", "다운로드";
    Later => "Later", "나중에";

    // Native dialogs
    TitleError => "Error", "오류";
    TitleConfirm => "Confirm", "확인";
    TitleQuit => "Quit", "종료";
    TitleCancel => "Cancel", "변환 취소";
    SavePresetFailed => "Could not save presets:\n{0}", "프리셋을 저장하지 못했습니다:\n{0}";
    QuitWhileConverting => "A conversion is still running.\nCancel it and quit?", "아직 변환 중입니다.\n취소하고 종료할까요?";
    ConfirmCancelConversion => "Are you sure you want to cancel the conversion?", "변환을 취소할까요?";
    Cancelling => "Cancelling...", "취소하는 중...";
    ResetDone => "RAW adjustments have been reset to defaults.", "RAW 조정값을 기본값으로 되돌렸습니다.";
    PresetSaved => "Preset '{0}' saved.", "프리셋 '{0}'을(를) 저장했습니다.";
    ConfirmDeletePreset => "Delete preset '{0}'?", "프리셋 '{0}'을(를) 삭제할까요?";
    PresetDeleted => "Preset '{0}' deleted.", "프리셋 '{0}'을(를) 삭제했습니다.";
    CouldNotStart => "Could not start conversion:\n{0}", "변환을 시작하지 못했습니다:\n{0}";
    InvalidSourceFolder => "Please select a valid source folder.", "올바른 원본 폴더를 선택하세요.";
    NoRawFound => "No RAW files found in the selected folder.", "선택한 폴더에 RAW 파일이 없습니다.";
    FileSystemError => "File system error: {0}", "파일 시스템 오류: {0}";
    UnexpectedError => "An unexpected error occurred:\n{0}", "예기치 못한 오류가 발생했습니다:\n{0}";
    ResultDoneTitle => "All cooked!", "다 구웠어요!";
    ResultPartialTitle => "Some files could not be converted", "일부 파일을 변환하지 못했습니다";
    ResultCancelledTitle => "Conversion cancelled", "변환을 취소했습니다";
    ResultErrorTitle => "The conversion stopped", "변환이 중단되었습니다";
    ResultConverted => "Converted", "완료";
    ResultSkipped => "Skipped", "건너뜀";
    ResultFailed => "Failed", "실패";
    ResultElapsed => "Time taken: {0}", "걸린 시간: {0}";
    ResultSavedTo => "Saved to: {0}", "저장 위치: {0}";
    ResultLogHint => "Details are in the log: {0}", "자세한 내용은 로그에 있습니다: {0}";
    ResultOpenFolder => "Open folder", "폴더 열기";
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
    PreviewHeader => "Preview", "미리보기";
    DropRawFolderHere => "Drop your RAW folder here", "RAW 폴더를 여기에 놓으세요";
    EmptyHint => "Or pick one with Browse above. Single RAW files work too.", "위의 찾아보기로 골라도 되고, RAW 파일을 바로 놓아도 됩니다.";
    ZoomFit => "Fit", "맞춤";
    ZoomActual => "100%", "100%";
    ZoomLoading => "Developing at full resolution...", "원본 해상도로 현상하는 중...";
    ZoomFailed => "100% view not available: {0}", "100% 보기를 열 수 없습니다: {0}";
    Compare => "Compare", "비교";
    Histogram => "Histogram", "히스토그램";
    CompareDeveloped => "Developed", "현상 결과";
    TipZoomFit => "Show the whole photo.", "사진 전체를 표시합니다.";
    TipZoomActual => "Real pixels. Drag to move around; double-click the photo to go back.", "실제 픽셀로 봅니다. 드래그로 이동하고, 사진을 더블클릭하면 돌아갑니다.";
    TipCompareNeedsCamera => "This file has no camera JPEG to compare with.", "이 파일에는 비교할 카메라 JPEG이 없습니다.";
    TipHistogram => "Shows or hides the histogram.", "히스토그램을 표시하거나 숨깁니다.";
    CameraJpegTag => "Camera JPEG", "카메라 JPEG";
    CompareTip => "Left: the camera's JPEG. Right: your development. Drag the divider.", "왼쪽은 카메라 JPEG, 오른쪽은 현상 결과입니다. 경계선을 드래그해 보세요.";
    NoRawHere => "No RAW files here.", "RAW 파일이 없습니다.";
    NoRawInFolder => "No RAW files in this folder.", "이 폴더에 RAW 파일이 없습니다.";
    LookingForRaw => "Looking for RAW files...", "RAW 파일을 찾는 중...";
    RawCount => "{0} RAW file(s)", "RAW 파일 {0}개";
    PreviewDeveloping => "Developing...", "현상 중...";
    PreviewCantDevelop => "Can't develop this file ({0}). Camera JPEG shown.", "이 파일은 현상할 수 없습니다 ({0}). 카메라 JPEG을 표시합니다.";
    PreviewUnavailable => "Preview unavailable: {0}", "미리보기 불가: {0}";
    PausedWhileConverting => "Paused while converting.", "변환 중에는 잠시 멈춥니다.";
    LoadingPreview => "Loading preview...", "미리보기를 불러오는 중...";
    NoPreviewYet => "No preview yet.", "아직 미리보기가 없습니다.";
    PreviewPausedStatus => "Preview paused while converting.", "변환 중에는 미리보기를 멈춥니다.";
    NoEmbeddedDevelopStatus => "No embedded JPEG in this file; conversion will develop the RAW.", "카메라 JPEG이 없는 파일입니다. 변환할 때 RAW를 현상합니다.";

    // Tooltips
    TipSource => "Folder with your RAW files. Subfolders are included.", "RAW 파일이 있는 폴더입니다. 하위 폴더까지 함께 변환합니다.";
    TipBrowse => "Pick a folder.", "폴더를 고릅니다.";
    TipOutput => "Where the output goes. Leave empty to use Converted_JPG inside the source folder.", "결과를 저장할 폴더입니다. 비워 두면 원본 폴더 안의 Converted_JPG에 저장합니다.";
    TipConflict => "What to do when an output with the same name already exists.", "같은 이름의 결과 파일이 이미 있을 때 어떻게 할지 정합니다.";
    TipQualityUnused => "Camera JPEGs are saved as they are, so quality only applies when a long edge is set.", "카메라 JPEG은 그대로 저장하므로, 긴 변을 지정했을 때만 품질이 적용됩니다.";
    TipExif => "Copies shooting data into the output: camera, lens, exposure, date, GPS and rating where the file has them. Not all metadata may be copied identically.", "카메라, 렌즈, 노출, 촬영 일시, GPS, 별점을 파일에 있는 만큼 결과 파일에 기록합니다. 일부 정보는 원본과 똑같이 복사되지 않을 수 있습니다.";
    TipKeepDate => "Gives the output the same modified date as its RAW file.", "결과 파일의 수정한 날짜를 RAW 파일과 같게 맞춥니다.";
    TipResize => "Long edge of the output in pixels. Leave empty to keep the original size.", "결과 이미지의 긴 변 픽셀 수입니다. 비워 두면 원본 크기를 유지합니다.";
    TipQuality => "JPEG and WebP: higher means better quality and larger files. 90 is a good default.", "JPEG과 WebP에 적용됩니다. 높을수록 화질이 좋아지고 파일이 커집니다. 보통 90이면 충분합니다.";
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
    const SAME_IN_BOTH: &[Key] = &[
        Key::StyleHdr,
        Key::ZoomActual,
        Key::LanguageEnglish,
        Key::LanguageKorean,
    ];

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
